# LumiPlayer — 逆向架构与 1:1 功能重建蓝图

> 目标：通过对 `lumiplayer-tauri.exe` (v1.2.6) 的逆向分析，摸清功能、架构、数据流、协议、模块关系，
> 在自己的项目里重建一个**功能等价**的版本。本仓库是重建的开发规范，不是源码。
>
> 分析对象 SHA256: `dcfc73cf33916d70973e96869165e641ce45a1043f8840688e25d460168f0583`
> 技术栈: Tauri 2.11.5 + wry 0.55.1 + WebView2 (Edge 139) + 内嵌 libmpv (FFmpeg n8.1) + StreamHub (Spring Boot JAR)

---

## 1. 系统总览（三轨架构）

> 经三批逆向，架构已从"双轨"修正为**三轨**：Rust 播放/直连轨、StreamHub 本地媒体中心轨、Lumi Cloud 远程托管轨。

```
┌──────────────────────────────────────────────────────────────────────┐
│  Frontend (WebView2 / Edge 139) — vanilla JS bundles, Tauri invoke()   │
└───────────────────────────────┬──────────────────────────────────────┘
                                 │ invoke() / 直接 fetch
            ┌────────────────────▼─────────────────────┐
            │  Rust/Tauri 主进程 (lumiplayer-tauri.exe)  │  ← 三轨的统一编排者
            │  · Tauri command 分发                      │
            │  · 云盘直连解析 (115/阿里/百度/夸克/123/天翼)│
            │  · Emby/Jellyfin/Plex 直连                  │
            │  · 内嵌 libmpv FFI 播放 (HDR/RIFE/MEMC)     │
            │  · 本地 SQLite: lumi-store.db (media/kv)    │
            │  · 凭据 .enc (Windows DPAPI) + cloud-accounts.json │
            └───────┬───────────────────────┬─────────────┘
       localhost HTTP │                    │ 硬编码远端
┌────────────────────▼──────┐   ┌──────────▼────────────────────────────┐
│ StreamHub (本地 Spring)    │   │ Lumi Cloud (api_3)                    │
│ (动态端口)  streamhub.db   │   │ http://117.72.12.20:9651/api/3       │
│ 媒体库/WebDAV/刮削/TMDB    │   │ (+ :9321 元数据服务)                  │
│ 流代理/HLS/字幕/AI-RAG     │   │ Emby 兼容流 + TMDB 兼容元数据         │
│ 账号/JWT/事件 SSE          │   │ /api/auth/login + X-Emby-Token        │
└────────────────────────────┘   └─────────────────────────────────────┘
```

**两条边界关系（已确认）**：
- **Rust 轨 ≠ StreamHub 的替代品**，二者平行：Rust 管"外部资源直连 + 播放器侧能力"，StreamHub 管"本地媒体中心能力"（自有片库 + HLS 代理 + AI）。
- **api_3 (Lumi Cloud) 是第三轨**：运营方托管的"自带内容源"（Emby 兼容流 + TMDB 兼容元数据 + 登录鉴权），与 StreamHub 不重叠（StreamHub 管用户自有片库，api_3 管托管内容）。
- 三者都由 Rust 层统一编排：`resolveProvider` 同时路由到 云盘 / Emby / api_3 / 本地。

**双本地持久化（重要修正）**：Rust 侧与 StreamHub 各有独立 SQLite，并非"一切经 StreamHub"：
- `lumi-store.db`（Rust）：`media`(FTS5 `media_fts`) + `kv`
- `streamhub.db`（StreamHub/MyBatis-Plus）：`user/refresh_token/media_source/media_file/movie/tv_show/...`

---

## 2. 进程与可执行布局（重建目标目录树）

```
LumiPlayer/
├── lumiplayer-tauri.exe        # Rust/Tauri 主程序 (26.7MB, 未加壳)
├── ffmpeg.exe / ffmpeg shared  # FFmpeg n8.1 (BtbN 共享版) — 供 StreamHub 转码
├── mpv-2.dll                   # libmpv (内嵌 FFI, 非独立 exe)
├── shaders/                    # mpv GLSL 着色器目录
│   ├── ArtCNN.glsl
│   ├── SSimDownscaler.glsl
│   └── adaptive-sharpen.glsl
├── plugins/                    # Tauri 插件 / 扩展
├── streamhub-local-api.jar     # 本地媒体服务 (102MB Spring Boot fat jar)
├── resources/                  # 前端资源 (carved: renderer/, assets/)
├── data/                       # 运行时数据
│   ├── lumi-store.db           # ★ Rust 本地库 (SQLite/WAL): media + media_fts(FTS5) + kv
│   ├── streamhub.db            # ★ StreamHub 本地库 (SQLite): user/media_source/media_file/movie/tv_show/...
│   ├── cloud-accounts.json     # 云盘账号索引 (明文 JSON)
│   ├── direct-cloud-auth.json  # 光鸭盘凭据 (明文 JSON)
│   ├── emby-credentials.enc    # Emby 凭据 (.enc = Windows DPAPI 加密, 账户绑定)
│   ├── jellyfin-credentials.enc
│   ├── plex-credentials.enc
│   ├── feiniu-credentials.enc
│   ├── lumi_cloud_media_library_v*.json  # 云媒体库缓存 (v4..v11)
│   ├── license.dat / .secure-timestamp / license-backup
│   └── Cookies/Local Storage/IndexedDB/   # WebView2 持久化
└── config/
    ├── mpv.conf                # 由二进制选项重建 (见 player/mpv_config.md)
    └── input.conf
```

---

## 3. Tauri Command 分发模型（★重建核心）

### 3.1 发现
- 二进制内嵌一份 **Tauri capability/ACL 权限清单**，枚举了每个命令的 `allow-<cmd>` / `deny-<cmd>`。
- 这份清单是**权威的**框架命令来源（window/webview/menu/app/event/fs/image/path 插件）。
- 自定义业务命令（media_*/cloud_*/direct_cloud_*/kv_*）**不在** ACL 中 → 走 `generate_handler` 注册，无逐命令权限条目。
- 前端 `invoke()` 调用被封装为 `invoke(e,n,t)` 变量形式（见 `00-tauri-electron-compat.js`），命令名不在 JS 字面量中，仅在 Rust 侧字符串泄露。

### 3.2 命令清单（完整）
见 `../analysis/tauri_commands.json`（结构化）：
- **内置插件命令 ~200+**：来自 ACL，权威。
- **自定义命令 ~50 已恢复**：playback / cloud_direct_link / media_library / account_auth / kv_state / fs_system / config_theme / backend。
- **缺口**：自定义命令完整枚举需**动态分析**（运行时 hook `__TAURI_INTERNALS__.invoke`）。

### 3.3 重建要点
- 自定义命令用 `#[tauri::command] fn xxx(...)` + `generate_handler![...]` 注册。
- 前端用封装层调用；重建时可改用 Tauri 官方 `@tauri-apps/api/core` 直接 `invoke('cmd', args)`。
- KV 存储 (`kv_get/set/delete/all`) 是自实现的轻量键值命令（非 Tauri 官方 plugin）。

---

## 4. 播放链路（静态重建，动态确认待做）

```
用户点击 → Frontend invoke('get_video_preview_play_info' / 'direct_url')
  → Rust resolveProvider(): 根据源类型选择云盘适配器
      · 光鸭/115/飞牛: 本地 OAuth token → 调各自 API 拿直链
      · 远端解析: 调 http://117.72.12.20:9651/api/3 拿直链
  → 返回 playable URL (或 HLS m3u8)
  → mpv FFI: mpv_create → mpv_initialize → mpv_set_option(wid=WebView2 hwnd)
      → mpv_command(["loadfile", url]) → render_context (gpu-next/d3d11)
  → GPU 渲染; 字幕经 get_subtitle_info / get_subtitle_tracks 注入
```

**StreamHub 流代理**（云流不直接给 mpv 时）：
`StreamProxyServiceImpl.openVideo/openSubtitle/resolvePlayableUri/ensureHlsPlaylist` → 用 ffmpeg 做 HLS 转码/remux，再代理给 mpv。

---

## 5. 凭据与安全模型

| 存储 | 格式 | 加密 | 说明 |
|------|------|------|------|
| `direct-cloud-auth.json` | JSON | 明文 | 光鸭盘凭据 |
| `cloud-accounts.json` | JSON | 明文 | 云盘账号索引 |
| `emby/jellyfin/plex/feiniu-credentials.enc` | .enc | **Windows DPAPI** (`Crypt32Util.cryptProtectData` + Base64) | 媒体服务器 / 云凭据，OS 用户账户绑定，**不可跨机移植** |
| `__LUMI_SESSION_AUTH__` | WebView2 sessionStorage | — | 会话令牌键 |
| `license.dat` / `.secure-timestamp` | 二进制 | 是 | 授权 |

> **`.enc` 结论已修正**：早期"疑似 AES"已排除。凭据加密在 **StreamHub `WindowsCredentialCryptoService`（JNA `Crypt32Util` → Windows DPAPI）+ Base64**，属 OS 级账户绑定保护，非自定义 AES。Rust 侧的 `.enc` 凭据文件同源（Windows 即 DPAPI）。重建时**不要**复刻 AES，应直接用 Windows DPAPI / OS keychain。

---

## 6. 外部播放器探测
Rust 侧探测本机已装播放器并可选外抛：
`mpv.exe, DAUMPotPlayerMini64.exe, PotPlayerMini.exe, MPC-HC(64), mpc-hc.exe, VideoLAN(VLC), MPC-BE x64, mpc-be64.exe, MPC-BE, K-Lite(MPC-HC64)`。

---

## 7. 逆向优先级（★ 第三批重排 —— UI 已非最大缺口）

> UI 已不再是信息缺口最大的地方；重心转向"后端真实连接关系"与"播放算法"。

1. **① Lumi Cloud / api_3** —— 远程托管媒体层（`:9651/api/3` + `:9321`），含 `/api/auth/login`、Emby 兼容流、TMDB 兼容元数据、XOR 字符串混淆。**【✅ 已闭环 → `lumi_cloud_api3.md` + 本轮新增 :9321=元数据blob、login 双端点、Rust 代理特殊拦截】**
2. **② Rust ↔ StreamHub 连接关系** —— 谁启动 StreamHub、谁发现端口、何时连接、桌面模式如何鉴权、Rust 还是前端直接调 StreamHub API。**【✅ 已闭环 → `streamhub_runtime.md`：Rust 以 `java -jar` 拉起、动态端口、桌面代理 permitAll】**
3. **③ Rust `resolveProvider` 完整调用链** —— 点击播放 → 命令 → provider 判断 → token 读取 → 哪一个 HTTP API → URL 转换 → PlaybackInfo → mpv。**【✅ 已闭环 → `rust_command_layer.md` §13 状态机】**
4. **④ StreamHub 实际 DB schema** —— `schema.sql`（JAR 内，`spring.sql.init` 执行）+ 448 类反编译 + exe 字符串恢复。**【✅ 已闭环 → `streamhub/db_schema_real.md`：streamhub.db 15 表 / lumi-store.db 全 DDL；修正"13 表/library/subtitle/play_history"误判】**
5. **⑤ 完整 Rust IPC 命令枚举** —— 字节挖掘 + 双 JSON 交叉。**【✅ 已闭环 → `analysis/ipc_commands_full.md`：~52 真命令，新发现 5 个；修正 emby_request 等种子名为误报】**
6. **⑥ Provider 内部直链算法** —— 6 云盘 + 字幕 + intro-skip。**【✅ 静态端点/OAuth/头已恢复；sign 公式需动态抓包 → `protocol/cloud_provider_algorithms.md`】**
7. **⑦ 更新 / 插件机制** —— 自定义更新器 + VapourSynth 插件。**【✅ 已闭环 → `update_license.md`】**
8. **⑧ License 授权校验** —— 混合离线 RSA 签名 + 在线激活。**【✅ → `update_license.md` §3】**
9. **⑨ 前端调用图** —— Tauri v2 JS IIFE 已 carving（业务 UI 包命令名仅存 Rust 字面量）。**【✅ → `frontend/call_graph.md`】**
10. **⑩ 会话生命周期** —— 三套鉴权并存（api_3 / StreamHub / 云盘 DPAPI）。**【✅ → `frontend/call_graph.md` §3】**
11. **⑪ 媒体服务器协商深化** —— Emby/Jellyfin/Plex 字段集。**【✅ → `streamhub/subsystems.md` §4】**
12. **⑫ StreamHub RAG/AI** —— 配置齐全但**未接线**（无 Qdrant/Meili 客户端依赖）。**【✅ → `streamhub/subsystems.md` §2】**
13. **⑬ 流代理 / HLS 转码** —— JAVE2+ffmpeg HLS(fmp4) 链。**【✅ → `streamhub/subsystems.md` §1】**
14. **⑭ 缩略图 / 元数据刮削** —— ImageBackfillRunner + TMDB；StreamHub **无 ffprobe/缩略图**（来自 Emby/Plex/TMDB）。**【✅ → `streamhub/subsystems.md` §3】**

> **S 级三方向已全部闭环**（① api_3 / ② Rust↔StreamHub / ③ resolveProvider），证据来源于 exe 字节挖掘 + StreamHub JAR 反汇编。
> **后续 ④–⑭ 全部由 6 个并行智能体完成**（静态为主，动态缺口已标注）。
> 已消除疑点：`.enc` 加密 = **Windows DPAPI**（非 AES，javap 反汇编 `WindowsCredentialCryptoService` 确证）；StreamHub 端口**动态**（非字面 18400）；StreamHub 是本地媒体中心而非 Rust 替代；Lumi Cloud (api_3) 是独立第三轨；StreamHub DB 实际 **15 表、无 FK、无 Flyway**；RAG **未接线**。

---

## 8. 文档索引
- `rust_command_layer.md` — **★ Rust 命令层与真实业务逻辑**（resolveProvider 路由树、数据契约、HDR 渲染、鉴权头混淆、RIFE/MEMC 管线；**§13 resolveProvider 完整状态机**）
- `data_contracts.md` — **★ IPC 数据契约速查表**（serde 字段 → JSON schema，1:1 重建硬约束）
- `api/backend_api.md` — 硬编码解析后端 api/3 + 响应字段归一化
- `protocol/cloud_oauth.md` — 各云盘 OAuth 二维码登录端点与参数
- `player/mpv_config.md` — mpv 选项重建 + 着色器 + 外部播放器
- `database/schema.md` — StreamHub 实体模型 (13 表) + 本地 SQLite
- `frontend/modules.md` — 前端模块映射 (115 资产 → 业务模块)
- `../analysis/tauri_commands.json` — 完整命令清单（结构化）
- `../analysis/ipc_commands_detailed.json` — **★ IPC 命令契约（args/response/error + 云盘端点 + 本地 SQL）**
- `../analysis/reconstruction_artifacts.json` — mpv/云盘/凭据/响应信封 原始提取
- `streamhub/architecture.md` — **★ StreamHub 本地后端架构**（端口18400/SQLite/JWT/DPAPI/RAG/子系统）
- `streamhub/api_map.md` — **★ StreamHub HTTP API 地图**（15 Controller → 端点 → Service → Entity）
- `lumi_cloud_api3.md` — **★ Lumi Cloud (api_3) 远程托管媒体层**（端点/XOR混淆头/鉴权/三轨关系）
- `streamhub_runtime.md` — **★ StreamHub 运行时**（启动 `java -jar`/动态端口/桌面代理 permitAll 鉴权/JWT/DPAPI 凭据加密）
- `streamhub/db_schema_real.md` — **★ StreamHub 真实 DB schema**（JAR `schema.sql` 15 表 + lumi-store.db 全 DDL）
- `streamhub/subsystems.md` — **★ StreamHub 子系统**（流代理/HLS 转码、RAG 接线、刮削/缩略图、媒体服务器协商）
- `protocol/cloud_provider_algorithms.md` — **★ 云盘/字幕/intro-skip Provider 直链与签名算法**（静态端点/OAuth/头 + 动态缺口）
- `update_license.md` — **★ 更新机制 / VapourSynth 插件 / License 混合校验**
- `frontend/call_graph.md` — **★ 前端定位(carving) / UI→invoke 命令图 / 三套会话生命周期**
- `../analysis/ipc_commands_full.md` — **★ 完整 Rust IPC 命令枚举（~52 真命令 + 新发现 5）**
