# LumiPlayer 逆向工程综合分析报告

> **目标**：`lumiplayer-tauri.exe`（Tauri **2.11.5** 桌面客户端）1:1 功能重建级逆向（非源码反编译）。
> **报告日期**：2026-08-18
> **方法**：静态取证（PE / fat JAR 字节码反汇编 `javap -c`、字符串提取、前端 carving、二进制扫描）+ 运行时实证（实跑 StreamHub JAR 抓 API / JWT 闭环）。
> **证据分级**：
> - 【确证】运行时实测 + 字节码反汇编双重印证
> - 【高】静态字节码 / 字符串直接恢复
> - 【中】基于上下文推断
> - 【待动态】需运行期 hook / 抓包确认

---

## 0. 摘要（TL;DR）

LumiPlayer 是**三轨架构**的桌面媒体客户端：Rust（Tauri）负责直连/播放与进程编排，StreamHub（内嵌 Spring Boot fat JAR）提供本地媒体中心，Lumi Cloud（`api_3`）提供远端托管。本会话完成：

- **StreamHub 运行时鉴权闭环实测通过**（JWT **HS512**、Argon2 密码、ADMIN 角色约束、签名校验生效）。
- **License 离线公钥静态缺失**——推翻旧"内嵌 RSA 公钥"断言（exe/JAR/前端扫描全部 0 命中）。
- 三轨架构、API 地图、云盘 Provider 签名线索、更新/插件机制、会话模型全部梳理归档。

---

## 1. 分析来源（Sources）

| # | 来源 | 形态 / 大小 | 用途 |
|---|---|---|---|
| S1 | `lumiplayer-tauri.exe` | PE32+, 26.7 MB | Rust 主程序、JVM 启动参数、Tauri 命令、云盘 sign、license IPC、前端 carving |
| S2 | `streamhub-local-api-0.1.0.jar` | Spring Boot fat JAR, 102 MB | StreamHub 后端全部类（448 `.class` / 98 依赖 jar） |
| S3 | `_up_\resources\streamhub-local-api\target\...jar` | 同 S2（发行内嵌副本） | 同上 |
| S4 | 安装目录 `_up_` 暂存区 | mpv / ffmpeg / shaders / plugins / VapourSynth | 运行时资源、`tauri-runtime-manifest.json`、`README.txt`、VapourSynth 插件 |
| S5 | 前端 JS（exe 内 carving） | 69 文件（偏移 21296514 起） | Tauri v2 JS API（IIFE，`__TAURI_INTERNALS__`×56） |
| S6 | `analysis/rsa_scan/*.json` | 扫描产物 | 公钥静态缺失证据 |
| S7 | `analysis/streamhub_run/` | 隔离运行目录 + 日志 | StreamHub 实跑闭环 |

**工具链**：系统 Temurin 21（`jdk-21.0.11+10`，`javap -c` 反汇编需把 `BOOT-INF/lib/` 98 个依赖 jar 并入 classpath）；`cfr-0.152.jar` 为 4.5 KB 残桩不可用，改 `javap`；Python 扫描脚本（`scan_rsa*.py`、`scan_crypto.py`、`scan_frontend_keys.py`）。

---

## 2. 总体架构（三轨模型）【高】

```
┌─────────────────────────────────────────────────────────────┐
│                       LumiPlayer (Tauri 2)                    │
│  ┌──────────────────┐     进程编排      ┌──────────────────┐  │
│  │  Rust 主进程      │ ─── java -jar ─▶ │  StreamHub        │  │
│  │  直连/播放/代理   │ ◀── 探活/暴露 ─── │  Spring Boot      │  │
│  │  Tauri 命令层     │                  │  :动态端口 本地    │  │
│  └────────┬─────────┘                  └──────────────────┘  │
│           │ 云端信令/元数据                                          │
│           ▼                                                       │
│  ┌──────────────────┐                                            │
│  │  Lumi Cloud       │  api_3 (117.72.12.20:9651) + :9321 元数据  │
│  │  远端托管         │                                            │
│  └──────────────────┘                                            │
└─────────────────────────────────────────────────────────────┘
```

- **Rust 轨**：直连播放、云盘 Provider 路由、StreamHub 生命周期管理、license/完整性监视、前端 UI 宿主。
- **StreamHub 轨**：本地媒体中心（媒体库 / 刮削 / 缓存 / 账号 / JWT），由 Rust 以 `java -jar` 子进程拉起。
- **Lumi Cloud 轨**：远端媒体托管与账号体系（`api_3`），与 StreamHub 本地账号是**两套独立鉴权**。

---

## 3. 二进制与运行时取证

### 3.1 PE 结构与字符串【高】
- 完整 PE 解析、ASCII/Unicode 字符串提取（`strings_ascii.txt` 2.6 MB、`strings_unicode.txt` 31 KB）。
- 恢复 Rust Tauri 命令名（`tauri_commands.json`、`ipc_commands_detailed.json`）。

### 3.2 Rust ↔ StreamHub 编排（从 exe 字节 @0x145bbac / @0x1452e1b）【确证】
**JVM 启动参数（Rust 注入）**：
```
-Dserver.address=127.0.0.1
-Dserver.port=                       ← 动态注入，非字面 18400（"18400" 命中 0）
-Dstreamhub.desktop-proxy.enabled=true
-Dspring.datasource.url=jdbc:sqlite:...?busy_timeout=30000
-Dstreamhub.paths.cache-dir=
-Dstreamhub.paths.image-dir=
-Dstreamhub.paths.subtitle-dir=
STREAMHUB_*                          ← 环境变量前缀
```
**Rust Tauri 命令**（生命周期管理）：
```
streamhub-start      拉起 java -jar 子进程
streamhub-status     健康检查（探活 → 决定降级）
streamhub-base-url   暴露实际 http://127.0.0.1:<port> 给前端
```
其它：`emby-msid`、`preheat-url`、`emby-request`/`jellyfin-request`/`plex-request`/`cloud-http-request`/`webdav-probe`。

### 3.3 StreamHub 启动模型【确证】
- 非系统服务，随安装包分发 fat JAR，由 Rust 子进程拉起。
- **无内置 JRE**，依赖系统 Temurin 21；缺 JRE → StreamHub 起不来 → Rust 降级（本地媒体中心不可用，Rust 直连/播放轨不受影响）。
- 每账户隔离：`data\accounts\<sha256>\`。
- `_up_` 是干净运行时暂存区（mpv/ffmpeg/shaders/plugins + StreamHub JAR + VapourSynth），`tauri-runtime-manifest.json` 明确**排除**凭据/历史。

### 3.4 StreamHub 实跑验证（本会话，隔离目录）【确证】
- 实例：端口 **18500**，严格模式（`desktop-proxy` 关闭），PID 1222，系统 Temurin 21。
- 手工建 `data/ cache/images/ cache/subtitles/`（sqlite-jdbc 不自动建父目录）。
- `/api/system/health` → `{"status":"UP"}`（探活端点确证）。
- 插入 Argon2 用户 `rev`/`RevPass123`，登录 200，闭环见 §5.2。

---

## 4. StreamHub 逆向（Spring Boot）

### 4.1 技术栈【高】
- Spring Boot **3.4.4**；`Main-Class: org.springframework.boot.loader.launch.JarLauncher`，`Start-Class: com.streamhub.localapi.StreamHubLocalApiApplication`。
- JJWT（`io.jsonwebtoken`）、Spring Security、SQLite（`sqlite-jdbc 3.49.1`，相对路径 `./data/streamhub.db?busy_timeout=30000`）、MyBatis-Plus、Caffeine、虚拟线程。
- `spring.sql.init.mode: always`；`management.endpoints.web.exposure.include: health,metrics,prometheus`。

### 4.2 安全模型【确证】
**SecurityConfig（desktop-proxy 双链）**：
- `desktopProxyEnabled`（对应 `-Dstreamhub.desktop-proxy.enabled`）：
  - **开启（Rust 默认）** → localhost / 桌面客户端 **permitAll**（免登录直连所有端点）。
  - **关闭（严格）** → permitAll：`OPTIONS /**`、`/api/auth/**`（register/login/refresh/logout/forgot-password/reset-password/register/code）、`/api/system/health`、`/error`、`/actuator/health|prometheus`、`/actuator/metrics/**`、`/cache/**`、`/cache/images/**`；`hasRole("ADMIN")`：`/api/sources/**`、`/api/settings/**`；其余 `authenticated()`。
- `webSecurityCustomizer` 忽略 `/cache/**`、`/images/**`、`/error`。
- 全局：CSRF 关、CORS 开、httpBasic 关、formLogin 关、`sessionManagement` → **STATELESS**。
- 异常：`AUTH_UNAUTHORIZED`/"Authentication required"、`AUTH_FORBIDDEN`/"Access denied"。

**AuthTokenFilter（JJWT）**：
- `OncePerRequestFilter`，置于 `UsernamePasswordAuthenticationFilter` 之前。
- token 三来源：`Authorization: Bearer` 头 → `access_token` 请求参数 → Cookie（`authService.accessCookieName()`）。
- `authService.parseAccessToken` 用 JJWT `verifyWith(secretKey)` 校验，要求 claim `type == "access"`；`buildPrincipal` 直接从 claim 构造 `AuthPrincipal`（**无 DB 查用户**）。

**JWT 密钥派生**【确证】：
- `AuthService.loadOrCreateJwtSecret` + `java.util.Base64` + `hmacShaKeyFor` + `Keys`。
- `${STREAMHUB_JWT_SECRET}` 是**环境变量**（非 `-D`），首次签发时懒生成随机密钥（≥32 字节，base64）持久化到 `app_setting` 表。
- **运行实例密钥是内存/DB 随机值 → 静态不可预测，伪造 JWT 不可行**。

**凭据加密 = Windows DPAPI**【确证，修正旧"AES"推测】：
- `WindowsCredentialCryptoService implements CredentialCryptoService`：`Crypt32Util.cryptProtectData/cryptUnprotectData`（JNA `com.sun.jna.platform.win32`）+ Base64。
- `.enc` 凭据（`emby/jellyfin/plex/feiniu-credentials.enc`）与当前 OS 登录账户绑定，换账户/机器无法解密。

### 4.3 数据库 Schema（15 表）【高】
`media_source`、`movie`、`tv_show`、`tv_episode`、`media_file`、`scrape_cache`、`watch_history`、`agent_*`(多张)、`app_setting`、`users`、`refresh_tokens`、`email_verifications`、`password_resets`。
- SQLite `./data/streamhub.db`，`spring.sql.init.mode: always`，**无 Flyway、无外键**。
- 本地库区分：**lumi-store.db**（Rust：`media`+`media_fts`+`kv`）vs **streamhub.db**（StreamHub）。

### 4.4 缓存层（Caffeine，6 个）【高】
`home`、`recent`、`libraryCategory`、`libraryBrowse`、`movieDetail`、`showDetail`。

### 4.5 配置结构（StreamHubProperties）【高】
`tmdb` / `mdbList` / `paths` / `scheduler` / `network` / `auth` / `rag` / `mail`。
- `auth.*`：`access-token-minutes: 15`、`refresh-token-days: 30`。
- `rag.*`：RAG 配置齐全（Qdrant / Meilisearch / OpenAI embedding + reranker）但**全部 `${...:false}` 且代码无对应客户端依赖 → 未接线**。

---

## 5. API 与鉴权

### 5.1 StreamHub API 地图（24+ 真实路径）【高】
`/api/auth/{login,register,register/code,refresh,logout,forgot-password,reset-password}`、`/api/system/health`、`/api/home`、`/api/library`、`/api/library/browse`、`/api/settings/**`、`/api/sources/**`、`/api/stream/media-files/{id}`、`/actuator/{health,metrics,prometheus}`、`/cache/**`、`/cache/images/**`。

### 5.2 鉴权闭环实测【确证】
| 请求 | 结果 |
|---|---|
| `GET /api/system/health`（无 token） | **200** `{"status":"UP"}` |
| `GET /api/home`（无 token） | **401** `AUTH_UNAUTHORIZED` |
| `POST /api/auth/login` `{"account":"rev","password":"RevPass123"}` | **200** |
| `GET /api/home`（Bearer） | **200** |
| `GET /api/library/browse`（Bearer） | **200** |
| `GET /api/settings`（Bearer, role=user） | **403** `AUTH_FORBIDDEN` |
| `GET /api/sources`（Bearer, role=user） | **403** `AUTH_FORBIDDEN` |
| `GET /api/home`（Bearer + 篡改） | **401**（签名校验生效） |

**关键修正 / 结论**：
1. **JWT 算法 = HS512**（token 头 `eyJhbGciOiJIUzUxMiJ9` = `{"alg":"HS512"}`），非旧文档 HS256。
2. 登录请求字段 `account`+`password`；响应驼峰 `accessToken`/`expiresIn`/`user`。
3. 密码编码器 **Argon2**（`$argon2id$v=19$m=16384,t=2,p=1$...`），非 BCrypt。
4. 用户模型：`id`/`username`/`email`/`role`(`user`\|`admin`)/`status`(`active`)/`emailVerified`。
5. `/api/settings/**`、`/api/sources/**` 需 `ADMIN`；普通用户 403 印证约束真实生效。
6. `/api/library` 根路径 → 500（无 handler）；有效入口 `/api/library/browse`。

### 5.3 Lumi Cloud `api_3`（远端托管）【高】
- 主：`http://117.72.12.20:9651/api/3`（`/api/auth/login`、`/Items/`、`/PlaybackInfo`、`/tv/`、`/external_ids`、`imdbId`）。
- 二级元数据：`http://117.72.12.20:9321/<40-hex hash>`（返回 `episode_number`/`episode_title`/`index`，**非转码/源站**）。
- **混淆算法破解**：XOR 字符串混淆（手写 obfstr），片段 = 代码内嵌立即数 XOR `.rdata` 密文，密钥部分内嵌（如 `"kenH"`）。还原头：`X-Emby-Token`/`access_token`/`api_key`/`playsessionid`/`starttimeticks`/`deviceid`/`lumi_ret(ry)`/`sign`/`UserId`；Emby CDN 域 `emby.media`/`.emby.me`/`by.media`。
- 登录双端点 `/api/auth/login` + `/api/user/login`；Rust `api_3` 代理对 login/account 端点特殊拦截（`unsupported_auth_endpoint`/`invalid_account_endpoint`）；登录带 `device_id`。
- Emby `/Items/{guid}/PlaybackInfo` → `MediaSources[].DirectStreamUrl`；Plex `/library/metadata/{ratingKey}?X-Plex-Token=` → `DirectStreamUrl`。

---

## 6. 云盘 Provider 算法与状态机

### 6.1 Provider sign 线索（静态恢复）【高】
| Provider | 签名 / 鉴权线索 |
|---|---|
| 115 | 硬编码 **RSA-2048 公钥** + `x-nd-authorization` |
| Feiniu | `API_KEY` + `AUTH_SALT` + `nonce`/`timestamp`/`sign`（HMAC） |
| 百度 | `shaOne`（SHA-1） |
| 天翼 | `appId` + `appKey` + `datetimeStamp` |
| 夸克 | `client_id=532` |
| 123 | `client_id=aMe-8VSlkrbQXpUR` |
| 阿里 | `X-Canary` |

- 端点从 exe 恢复（阿里/115/百度/夸克/123/天翼）。
- **Rust crypto crate 指纹**：`ring-0.17.14`（RSA PKCS1v15/PSS、AES-GCM、EC ECDSA、ed25519）、`rustls-0.23.41`、`tokio-rustls`；签名逻辑位于 `providersrc/direct_cloud.rs`。

### 6.2 resolveProvider 状态机【高】
- **outcome 枚举**：`cache-hit` | `renderer-confirmed-source` | `fast-start-raw-url` | `ok` | `complete` | `fallback` | `error` | `cancelled` | `deadline` | `begin`。
- **分支**：`directProvider` / `cloudProvider` / `media-server` / `local` / `remote`。
- **两条路由路径**：system-proxy media path vs local direct proxy。

---

## 7. 前端与会话

### 7.1 前端 carving【高】
- 真实前端 JS 位于 exe 偏移 **21296514**（Tauri v2 JS API IIFE，`__TAURI_INTERNALS__`×56）。
- 业务 UI 包命令名仅以明文存在于 Rust 字面量（纯文本 `invoke` 0 命中）。

### 7.2 会话模型（三套鉴权并存）【高】
- `api_3` 远端鉴权 / StreamHub `permitAll`（桌面代理） / 云盘 DPAPI 凭据。
- 令牌落地 `__LUMI_SESSION_AUTH__`（`lumi-store.db` 的 `kv` 表）；登出 = `kv_delete` + `clear_all_browsing_data`。
- 本地 SQLite 表 `media`/`kv` 完整 SQL 已从 exe 字符串恢复；会话模型 `__LUMI_SESSION_AUTH__` + `*.enc` 凭据 + `cloud-accounts.json`。

---

## 8. 更新 / 插件 / License

### 8.1 更新机制（自定义，非 Tauri 内置）【高】
- 版本检查：`https://download.chenfn.fun:441/lumiplayer/version.json`，UA `LumiPlayer-Updater/1.1`。
- **SHA-256 校验强制**：报错 `update package sha256 is required/invalid`、`update package SHA-256 verification failed; partial file was removed`。
- 安全护栏：`source/path/filename/url not allowed`、`redirected to an untrusted source`、HTTP 错误、过大、大小不匹配。
- `_up_` 暂存模型：`tauri-runtime-manifest.json` 构建期仅拷贝播放运行时（mpv/ffmpeg/shaders/plugins/resources）；`README.txt` 溯源（BtbN FFmpeg n8.1、shinchiro libmpv 2026-06-07、qBittorrent v5.2.3）；`includeResources:false`、`clean:false`；排除 cache/logs/cookies/credentials/history/locks。

### 8.2 插件系统（仅 VapourSynth）【高】
- 嵌入式 Python **3.12**（`python312.dll` 等）+ `libvapoursynth.dll` + `vs-plugins/`（mvtools、akarin、vsort + DirectML/onnxruntime + `models/rife_v2/rife_v4.25_lite.onnx`）。
- `RUNTIME_MANIFEST.json`（path/size/sha256 完整性清单）；env `VSSCRIPT_PATH`/`VAPOURSYNTH_EXTRA_PLUGIN_PATH`；config `vapoursynth.toml`。
- 经 mpv 加载（`@lumi-interp:vapoursynth=file=[...]`），filters：`vapoursynth-mvtools`、`vapoursynth-vsort-rife`、`performanceFallbackVapourSynth`、`gtx1050VapourSynth`；`MEMC_RIFE_DML.vpy` 帧插值。

### 8.3 License 验证【高 + 关键修正】
**存储 / 文件**：`license.dat`、`.secure-timestamp`、`.license-backup`，伴随 `.enc` 凭据、`cloud-accounts.json`、`data/accounts/`。
**混合模型**：
- 离线形态：`license.dat` + `.secure-timestamp`（可信时间戳）+ `.license-backup`。
- 在线激活：IPC `account-activate`/`cloud-account-ac…`/`cloud-direct-status`/`cloud-account-activate`；服务端返回 `expire_at`/`expires_at`/`vip_expire_at`/`pro_expire_at`。
- 层级：`is_pro`/`is_vip`/`is_member`/`is_permanent`(forever)/`is_privilege`；过期字段 `pro_expire_at`/`vip_expire_at`/`expire_at`。
- 反篡改 IPC：`native-guard-integrity`、`pro-integrity-*`、`pro-verify-*`、`device-fingerprint`、`pro-secure-save/load-tokens`；状态标志 `licensecleared`/`activated`。

> ### ⚠️ 关键修正（2026-08-18，推翻旧"内嵌 RSA 公钥"断言）【确证】
> 静态扫描 `lumiplayer-tauri.exe` + StreamHub JAR + 69 个前端 JS，覆盖 PEM / DER / base64-DER / 裸 RSA 模数 / EC(P-256) 点 / Ed25519 公钥 —— **全部 0 命中**（`analysis/rsa_scan/rsa_keys.json` / `rsa_keys_enhanced.json` / `rsa_keys_general.json` / `license_candidates.json` / `frontend_keys.json` 皆为 `[]`）。
>
> **结论**：离线校验公钥**不在静态二进制**。最可能是：(a) 由 Lumi Cloud 服务端在激活/校验时下发；(b) 运行时派生/混淆（如机器码 + 常量经 KDF，或 Rust `ring` 内存构造，无明文落盘）；(c) 所谓"离线"实为限时复核（首次在线激活后本地缓存，定期回服务端复核），并非纯本地 RSA 验签。
>
> **剩余缺口（必须 runtime hook）**：Frida/debugger 在 `license-verify`（机器码拼 `"license-"+"e-verify"`）执行时捕获实际校验密钥字节；抓取真实 `license.dat`+`.secure-timestamp` 做格式/签名算法逆向；确认完整性监视器在篡改时的动作（停用 vs 遥测）。

---

## 9. 关键数据字段汇总

### 9.1 License 状态字段
`status`: `is_pro` / `is_vip` / `is_member` / `is_permanent`(forever) / `is_privilege`
`expire`: `pro_expire_at` / `vip_expire_at` / `expire_at`
服务端返回：`expire_at` / `expires_at` / `vip_expire_at` / `pro_expire_at`

### 9.2 JWT（StreamHub）
- 算法：**HS512**；claim 必须 `type == "access"`。
- 有效期：`access-token-minutes: 15`，`refresh-token-days: 30`。
- 登录请求：`{account, password}`；响应：`{accessToken, expiresIn, user}`。
- 用户对象：`{id, username, email, role(user|admin), status(active), emailVerified}`。

### 9.3 Tauri 命令（Rust ↔ 系统）
生命周期：`streamhub-start` / `streamhub-status` / `streamhub-base-url`
播放/云盘：`emby-msid` / `preheat-url` / `emby-request` / `jellyfin-request` / `plex-request` / `cloud-http-request` / `webdav-probe`
授权/完整性：`license-verify` / `account-activate` / `cloud-account-activate` / `native-guard-integrity` / `pro-integrity-*` / `pro-verify-*` / `device-fingerprint` / `pro-secure-save/load-tokens`
其它：`fetch_channel_data_command` / `updated_playlist` / `set_playback_speed` / `live_transcoding` / `check_permissions`

### 9.4 云盘 Provider 端点线索
115（RSA-2048 + `x-nd-authorization`）、Feiniu（`API_KEY`+`AUTH_SALT`+nonce/timestamp/sign）、百度（`shaOne`）、天翼（`appId`+`appKey`+`datetimeStamp`）、夸克（`client_id=532`）、123（`client_id=aMe-8VSlkrbQXpUR`）、阿里（`X-Canary`）。

### 9.5 本地 DB 表（StreamHub，15 张）
`media_source` `movie` `tv_show` `tv_episode` `media_file` `scrape_cache` `watch_history` `agent_*` `app_setting` `users` `refresh_tokens` `email_verifications` `password_resets`
（Rust 侧另用 `lumi-store.db`：`media`+`media_fts`+`kv`）

---

## 10. 结论与置信度

### 已闭环（【确证】）
- 三轨架构与 Rust↔StreamHub 编排（JVM 参数、Tauri 命令、动态端口、探活端点 `/api/system/health`）。
- StreamHub 安全模型（desktop-proxy 双链、AuthTokenFilter、JWT HS512、DPAPI 凭据、Argon2 密码、ADMIN 约束）。
- 鉴权闭环实测（登录 → Bearer → 200 / 403 / 401 全路径）。
- 更新机制、VapourSynth 插件、api_3 混淆还原、云盘 Provider sign 线索、resolveProvider 状态机、前端 carving、会话三套鉴权。
- License 离线公钥**静态缺失**（推翻旧断言）。

### 待动态确认（【待动态】）
1. 各 Provider `sign` 生成公式（`direct_cloud.rs` 运行时 hook）。
2. `license.dat` 精确 schema / RSA 模数（`license-verify` 运行时 hook）。
3. RAG 若未来启用需接线验证。
4. `version.json` 签名机制、更新器重启/替换 exe 流程、`_up_` 是否可热补丁。
5. 完整性监视器在检测到篡改时的实际动作。

---

## 11. 交付物清单

| 文件 | 内容 |
|---|---|
| `LumiPlayer-Rebuild/architecture.md` | 三轨架构总览（含 HS512 修正） |
| `LumiPlayer-Rebuild/streamhub_runtime.md` | StreamHub 启动/鉴权/桌面代理 + §10 运行时闭环实测 |
| `LumiPlayer-Rebuild/update_license.md` | 更新/插件/License + §3.1 公钥静态缺失修正 |
| `LumiPlayer-Rebuild/rust_command_layer.md` | Rust 命令层 + resolveProvider 状态机（§13） |
| `LumiPlayer-Rebuild/lumi_cloud_api3.md` | api_3 端点 / 混淆 / 登录双端点 |
| `LumiPlayer-Rebuild/streamhub/architecture.md` | StreamHub 安全模型 / 数据库 / 子系统 |
| `LumiPlayer-Rebuild/streamhub/api_map.md` | StreamHub HTTP API 地图 |
| `LumiPlayer-Rebuild/streamhub/db_schema_real.md` | streamhub.db 15 表 schema |
| `LumiPlayer-Rebuild/streamhub/subsystems.md` | 流代理 / RAG / 媒体协商 |
| `LumiPlayer-Rebuild/protocol/cloud_provider_algorithms.md` | 云盘 Provider 算法线索 |
| `LumiPlayer-Rebuild/frontend/call_graph.md` | 前端 carving / 会话模型 |
| `analysis/rsa_scan/*.json` | 公钥静态缺失证据（全部 `[]`） |
| `analysis/streamhub_run/` | StreamHub 实跑目录 + 日志 + 闭环脚本 |

> **运行实例备注**：StreamHub 验证实例仍在跑（PID 1222，端口 18500）。停止：`taskkill /PID 1222 /F`。
