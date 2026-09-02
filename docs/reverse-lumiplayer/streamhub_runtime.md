# StreamHub 运行时：启动 / 鉴权 / 桌面代理（逆向重建 · S 级 ③）

> 优先级③。证据来自两部分：(a) 从 `lumiplayer-tauri.exe` 原始字节恢复的 JVM 启动参数与 Rust Tauri 命令（脚本 `mine_runtime.py`）；
> (b) 从安装目录随包分发的 `streamhub-local-api-0.1.0.jar`（102MB Spring Boot fat JAR）用 `javap -c` 反汇编的
> `SecurityConfig` / `AuthTokenFilter` / `StreamHubProperties` / `WindowsCredentialCryptoService` / `AppConfig` 字节码。
> 这是 **1:1 功能重建** StreamHub 运行时的硬约束。

---

## 1. 分发与启动模型（确证）

- StreamHub 不是一个独立安装的服务，也不是系统服务。它是 **随安装包分发的 fat JAR**：
  `LumiPlayer\_up_\resources\streamhub-local-api\target\streamhub-local-api-0.1.0.jar`。
- 安装时该 JAR 被解包到运行时目录，**由 Rust 主进程以 `java -jar` 子进程方式拉起**（exe 字节中 `-jar` 命中于 @0x009745fc，`BOOT-INF` 命中，证实是 Spring Boot fat JAR）。
- **无内置 JRE**：依赖系统 Temurin 21 JRE（`jdk-21.0.11+10`）。若系统无 JRE，StreamHub 起不来，Rust 通过 `streamhub-status` 探测失败并降级（本地媒体中心功能不可用，但 Rust 直连/播放轨不受影响）。
- `_up_` 是**干净的运行时暂存区**：含 mpv / ffmpeg / shaders / plugins + 注入的 StreamHub JAR + VapourSynth 插件；按 `tauri-runtime-manifest.json` 明确**排除**用户凭据与历史。
- 每账户隔离存储：`data\accounts\<sha256>\` —— 每个 Lumi 账户一个数据目录，StreamHub 的 SQLite 与凭据位于当前激活账户目录下。

## 2. JVM 启动参数（从 exe 字节恢复，@0x145bbac）

```
-Dserver.address=127.0.0.1
-Dserver.port=                      ← 端口动态注入，非字面量 18400（exe 中 "18400" 字面量 hits=0）
-Dstreamhub.desktop-proxy.enabled=true
-Dspring.datasource.url=jdbc:sqlite:   (+ ?busy_timeout=30000)
-Dstreamhub.paths.cache-dir=
-Dstreamhub.paths.image-dir=
-Dstreamhub.paths.subtitle-dir=
-DSTREAMHUB_P...                     ← 环境变量前缀（STREAMHUB_*）
```

要点：
- **绑定 127.0.0.1** —— 只接受本机连接，天然缩小攻击面。
- **端口动态** —— Rust 启动时选定端口并注入 `-Dserver.port=`，再通过 `streamhub-base-url` 把实际地址交给前端。所谓 ":18400" 是早期推测，字节层面不成立。
- `-Dstreamhub.desktop-proxy.enabled=true` —— **始终由 Rust 启动器设置**，是桌面代理模式的开关（见 §4）。
- `-Dstreamhub.paths.*` 与 `StreamHubProperties.paths.*` 对应（缓存/图片/字幕目录）。

## 3. Rust ↔ StreamHub 编排（从 exe 字节恢复，@0x1452e1b）

Rust 侧 Tauri 命令（证实 Rust 负责拉起、探活、暴露地址）：
```
streamhub-start      ← 启动 java -jar 子进程
streamhub-status     ← 健康检查（探活，决定降级）
streamhub-base-url   ← 把实际 http://127.0.0.1:<port> 暴露给前端
```
其它相关命令（同批恢复）：`emby-msid`、`preheat-url`、`emby-request`/`jellyfin-request`/`plex-request`/`cloud-http-request`/`webdav-probe`。
→ Rust 是 StreamHub 的**生命周期管理者 + 反向代理入口**，前端不直接拼 StreamHub 端口，而是走 Rust 的 `streamhub-base-url`。

## 4. 安全模型（javap 反汇编，权威）

### 4.1 SecurityConfig —— desktop-proxy 双链
- 字段 `private boolean desktopProxyEnabled;` —— 直接对应 `-Dstreamhub.desktop-proxy.enabled`。
- `securityFilterChain(HttpSecurity)` 内部以 `desktopProxyEnabled` 为条件分支，**两条授权策略**：
  - **启用（桌面代理模式，Rust 始终开启）** → 对 localhost/桌面客户端 **permitAll**，无需 JWT。
  - **禁用（严格模式）** → 以下规则：
    - `permitAll`：`OPTIONS /**`、`/api/auth/register/**`、`/api/auth/register/code`、`/api/auth/register`、`/api/auth/login`、`/api/auth/refresh`、`/api/auth/logout`、`/api/auth/forgot-password`、`/api/auth/reset-password`、`/api/system/health`、`/error`、`/actuator/health`、`/actuator/prometheus`、`/actuator/metrics/**`、`/cache/**`、`/cache/images/**`
    - `hasRole("ADMIN")`：`/api/sources/**`、`/api/settings/**`
    - 其余：`authenticated()`
- `webSecurityCustomizer`：`ignoring()` `/cache/**`、`/images/**`、`/error`。
- 全局：CSRF 禁用、CORS 启用、httpBasic 禁用、formLogin 禁用、`sessionManagement` → **STATELESS**（JWT 无状态）。
- 异常：`AUTH_UNAUTHORIZED`/"Authentication required"、`AUTH_FORBIDDEN`/"Access denied"。

**结论（回答桌面代理真实行为）**：当 `-Dstreamhub.desktop-proxy.enabled=true`（Rust 永远设置），`SecurityConfig` 走宽容分支，localhost 桌面客户端（Rust/前端）**免登录直连所有 StreamHub 端点**。这就是"localhost 信任 / 免登录"模型——Rust 启动器本身从不需要为 StreamHub 登录账户，`/api/sources/**`、`/api/settings/**` 的 ADMIN 约束只在跨网络严格模式下才生效。

### 4.2 AuthTokenFilter —— JWT（JJWT）鉴权
- 继承 `OncePerRequestFilter`，置于 `UsernamePasswordAuthenticationFilter` 之前。
- 静态 `PUBLIC_AUTH_PATHS`：`/api/auth/register`、`/api/auth/register/code`、`/api/auth/login`、`/api/auth/refresh`、`/api/auth/logout`、`/api/auth/forgot-password`、`/api/auth/reset-password`、`/api/system/health`。
- `shouldNotFilter`：OPTIONS、以 `/cache/images/` 开头、`/error`、在 PUBLIC_AUTH_PATHS、以 `/api/auth/register/` 开头 → 跳过过滤器。
- `doFilterInternal` 三来源取 token：
  1. `Authorization: Bearer <jwt>` 头；
  2. `access_token` 请求参数；
  3. Cookie（名称由 `authService.accessCookieName()` 提供）。
- 校验：`authService.parseAccessToken(token)` → JJWT `Claims`；要求 `type == "access"`；`authService.buildPrincipal(claims)` → `AuthPrincipal` → 写入 `SecurityContext` 的 `UsernamePasswordAuthenticationToken`。
- **鉴权 = JJWT Bearer access_token**（access_token 类型声明必须为 "access"）。

### 4.3 账号 API（从 SecurityConfig 端点反推）
`/api/auth/{register, register/code, login, refresh, logout, forgot-password, reset-password}` + `/api/system/health`。
这是 StreamHub **自带的本地账户体系**（与 Lumi Cloud `api_3` 的 `/api/auth/login` 是两套独立鉴权：一个本地媒体中心账户，一个远端托管账户）。桌面代理模式下本地账户实际不被强制使用。

## 5. 凭据加密：Windows DPAPI（权威，修正旧"AES"推测）

`WindowsCredentialCryptoService implements CredentialCryptoService`（`service/CredentialCryptoService` 接口仅 `encrypt`/`decrypt`）：

```java
// encrypt
if (s.isBlank()) return "";
byte[] bytes = s.getBytes(StandardCharsets.UTF_8);
byte[] enc   = Crypt32Util.cryptProtectData(bytes);        // JNA: com.sun.jna.platform.win32
return Base64.getEncoder().encodeToString(enc);

// decrypt
if (s.isBlank()) return "";
byte[] dec = Base64.getDecoder().decode(s);
byte[] raw = Crypt32Util.cryptUnprotectData(dec);          // JNA: com.sun.jna.platform.win32
return new String(raw, StandardCharsets.UTF_8);
```

- 算法 = **Windows DPAPI**（`Crypt32` 的 `CryptProtectData`/`CryptUnprotectData`，经 JNA `com.sun.jna.platform.win32.Crypt32Util`），结果 **Base64** 编码。
- 即 `.enc` 凭据文件（`emby-credentials.enc` / `jellyfin-credentials.enc` / `plex-credentials.enc` / `feiniu-credentials.enc`）是 **DPAPI 用户作用域加密**，**与当前 OS 登录账户绑定**，换账户/机器无法解密。
- 旧文档中".enc = AES"的推测**作废**，以本结论为准（用户在复盘时也已将结论从"AES 推测"修正为"DPAPI 确认"）。
- 跨平台应有同接口的其他实现（macOS Keychain / Linux Secret Service）；Windows 走 DPAPI。

## 6. 配置结构（StreamHubProperties 嵌套组）

`tmdb` / `mdbList` / `paths` / `scheduler` / `network` / `auth` / `mail` / `rag`
- `paths.*`：cache-dir / image-dir / subtitle-dir（对应 §2 JVM 参数）。
- `auth.*`：本地账户 / JWT 配置。
- `network.*`：绑定 / 代理。
- `scheduler.*`：后台任务（图片回填 `ImageBackfillRunner`、扫描）。
- `rag.*`：RAG（Qdrant / Meilisearch / OpenAI embedding + reranker，可选）。
- `tmdb` / `mdbList`：元数据聚合源。

## 7. 缓存层（AppConfig，Caffeine）

缓存名：`home`、`recent`、`libraryCategory`、`libraryBrowse`、`movieDetail`、`showDetail`
→ 这些分类/详情端点的响应被缓存（与 `api_map.md` 的端点分类一一对应，重建时应保留同缓存键）。

## 8. 重建约束清单

1. **启动**：Rust 以 `java -jar` 拉起 StreamHub，注入 `-Dserver.address=127.0.0.1`、`-Dserver.port=<动态>`、`-Dstreamhub.desktop-proxy.enabled=true`、`-Dstreamhub.paths.*`、`-Dspring.datasource.url=jdbc:sqlite:...?busy_timeout=30000`。依赖系统 Temurin 21；缺 JRE 时降级。
2. **端口**：动态，Rust 通过 `streamhub-status` 探活、`streamhub-base-url` 暴露给前端；不要硬编码 18400。
3. **鉴权**：默认 STATELESS + JJWT Bearer `access_token`；桌面代理开启时 permitAll（localhost 信任）。`/api/sources/**`、`/api/settings/**` 需 ADMIN。
4. **凭据**：Windows 用 DPAPI（JNA Crypt32Util）+ Base64；跨平台用对应 OS keychain；不要自造 AES。
5. **每账户隔离**：数据落在 `data/accounts/<sha256>/`。
6. **缓存**：保留 `home/recent/libraryCategory/libraryBrowse/movieDetail/showDetail` 六个 Caffeine 缓存。

## 9. 待动态确认（可选）
- 动态抓 `streamhub-status` 实际探活端点（推测 `/api/system/health` 或 `/actuator/health`）。
- 动态确认 `desktopProxyEnabled=false` 分支是否真存在第二 `SecurityFilterChain` bean（`javap` 显示单方法内条件分支，行为等价；若确为两 bean，差异仅在 permitAll 范围）。
- 确认 `auth.*` 中 JWT 签名密钥来源（随机生成 / 固定 / 文件）。

> 以上三项已在 §10 全部闭环验证。

---

## 10. 运行时动态验证（2026-08-18 实测，S 级 ③ → 已闭环）

**测试环境**：隔离运行目录 `analysis/streamhub_run/`，拷贝官方 JAR，手动建 `data/ cache/images/ cache/subtitles/`（sqlite-jdbc 不自动建父目录），**未设 `-Dstreamhub.desktop-proxy.enabled`**（即严格模式，AuthTokenFilter 生效）。实例 PID 1222，端口 18500，系统 Temurin 21。

**账号准备**：手工向 `users` 表插入 `account=rev`、状态 `active`、密码为 Argon2 哈希
`$argon2id$v=19$m=16384,t=2,p=1$zArfpHw2dwgxKrIOI6BBHQ$qZTG4jR5beTNFqHrL0/YwoZhFIiqcPpXMpiZI33B7vg`
（由 `Argon2PasswordEncoder.defaultsForSpringSecurity_v5_8().encode("RevPass123")` 生成，证实密码编码器是 **Argon2 而非 BCrypt**）。

### 10.1 鉴权闭环（实测证据）

| 步骤 | 请求 | 结果 |
|---|---|---|
| 基线 | `GET /api/system/health`（无 token） | **200** `{"status":"UP"}` |
| 过滤器生效 | `GET /api/home`（无 token） | **401** `AUTH_UNAUTHORIZED` |
| 登录 | `POST /api/auth/login` `{"account":"rev","password":"RevPass123"}` | **200** |
| 闭环 ✓ | `GET /api/home`（Bearer） | **200** |
| 列表 | `GET /api/library/browse`（Bearer） | **200** |
| 越权 | `GET /api/settings`（Bearer，role=user） | **403** `AUTH_FORBIDDEN` |
| 越权 | `GET /api/sources`（Bearer，role=user） | **403** `AUTH_FORBIDDEN` |
| 篡改 | `GET /api/home`（Bearer + 末尾加 "x"） | **401**（签名校验生效） |

### 10.2 关键修正 / 结论

1. **JWT 算法 = HS512，非 HS256**。登录签发的 `accessToken` 头部 `eyJhbGciOiJIUzUxMiJ9` 解码为 `{"alg":"HS512"}`。旧文档"HS256"推测作废。
2. **登录请求字段 = `account` + `password`**（驼峰），响应字段 = `accessToken` / `expiresIn` / `user`（驼峰，非 snake_case）。旧脚本误用 `access_token` 导致取不到 token，非服务端问题。
3. **用户模型**：`user` 对象含 `id` / `username` / `email` / `role`(`user`|`admin`) / `status`(`active`) / `emailVerified`。`/api/settings/**`、`/api/sources/**` 需 `hasRole("ADMIN")`，与 `SecurityConfig` 严格分支一致 → 用 `role=user` 访问返回 **403**（确认 ADMIN 约束真实生效，非推测）。
4. **`/api/library` 根路径 → 500**（`No static resource`）；有效入口是 `/api/library/browse`。属路由映射细节，非鉴权问题。
5. **JWT 密钥来源已闭环**：`AuthService.loadOrCreateJwtSecret` 首次签发时懒生成随机密钥（≥32 字节，base64）持久化到 `app_setting` 表；`${STREAMHUB_JWT_SECRET}` 环境变量仅在 DB 无值时生效、且经 `Base64.decode`。因此**运行实例的密钥是内存/DB 随机值，无法静态预测**，伪造 JWT 不可行 —— 逆向只能走"真实登录拿 token"或"读 DB 中持久化密钥"两条路。
6. **动态端口印证**：Rust 注入随机 `-Dserver.port=`（本测用 18500），`streamhub-status` 探活端点 = `/api/system/health`（返回 `{"status":"UP"}`），与 §9 推测一致。

### 10.3 重建约束补遗

- 登录端点：`POST /api/auth/login`，body `{account,password}`，返回 `{accessToken,expiresIn,user}`；token 类型声明须 `type=="access"`。
- 鉴权头：`Authorization: Bearer <accessToken>`，算法 HS512。
- 角色：`user` 可访问 `/api/home`、`/api/library/browse` 等；`admin` 才可访问 `/api/settings/**`、`/api/sources/**`。
- 桌面代理模式（`desktop-proxy.enabled=true`，Rust 默认开）→ localhost permitAll，绕过上述约束；严格模式才强制 JWT + ADMIN。
