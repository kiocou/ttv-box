# LumiPlayer — Frontend → Backend Call Graph & Session Lifecycle (Targets ⑨ / ⑩)

> 来源：`lumiplayer-tauri.exe` (26 777 712 B) 原始字节挖掘。
> 关键提醒：`analysis/frontend_extract/` 已被 Rust 字符串表污染（`¸=EA` 标记、`update manifest ...` 等），**已排除，不使用**。

---

## §1 真实前端的定位方式与验证

### 1.1 扫描过程
1. 对 exe 全字节扫描嵌入 web 归档签名：`PK\x03\x04` (5 处)、`\x1f\x8b\x08` gzip (5 处)、zstd `\x28\xb5\x2f\xfd` (0 处)。
   - 全部 `PK`/`gzip` 命中经解析均为**误报**（方法号异常、解压失败、文件名是 x86 机器码）—— 即 Rust `.text` 段内的偶合字节，非真实 zip。
2. 改为扫描 Tauri 运行时标记，命中：`window.__TAURI__` ×1、`__TAURI_INTERNALS__` ×56、`invoke(` ×6、`addEventListener` ×10。
3. 在偏移 **21296514** 处找到真实 Tauri v2 JS API IIFE：
   `e.mocks=K,e.path=Y,...e.window=he,e}({});window.__TAURI__=__TAURI_IIFE__;`
4. 在偏移 **21248016** (长度 48531 B) 提取到 Tauri JS API 运行时包（含 `@tauri-apps/api` 全部插件绑定 + 内嵌的 capabilities/ACL 清单）；另提取 `tauri_api_a.js`(21157064)、`tauri_api_b.js`(22184744)。

### 1.2 真实 JS 验证（通过）
提取 blob 满足全部真实-JS 判据，且**不含**污染产物：
- 含 `window.__TAURI__=__TAURI_IIFE__`、`__TAURI_INTERNALS__` ×56、`addEventListener`、真实的 `invoke`。
- 含 invoke 包装器：`async function h(e,n={},t){return window.__TAURI_INTERNALS__.invoke(e,n,t)}`（@21257966）。
- 含 **159 个** `plugin:X|cmd` 命令标识符（见 §2）。
- **无** `L>EA` / `feiniu-lazy://` / `update manifest` 等 Rust 字符串表伪迹。

### 1.3 应用自身 UI 包**不可雕刻**（明确声明）
对 exe 全字节搜索业务命令的明文 `invoke("...")` 调用：**0 命中**。业务命令名（`media_search`/`kv_get`/`api_3`/`streamhub-start`/`refresh_token`…）仅以 **Rust 侧字面量**（SQL 串、命令注册名、capabilities 条目）形式出现，从不以 JS `invoke("cmd")` 字面量出现。
结论：LumiPlayer 的**应用层 UI 逻辑（调用业务命令的那部分 JS）被 Tauri v2 的压缩资源存储（asset store）嵌入，无法用字符串/正则扫描雕刻**；仅 Tauri 运行时 JS 与 capabilities 清单以明文形式可恢复。因此 §2 的命令图为**命令曲面级**（哪些命令存在且被授权），UI 按钮→命令的逐条绑定因 app 包不可得而只能推断。

---

## §2 UI → invoke 命令映射（按功能聚类）

所有前端调用统一经 `h(cmd,args)` → `__TAURI_INTERNALS__.invoke`，命令名格式：`plugin:<plugin>|<cmd>`（Tauri 插件）或裸命令名（Rust `#[tauri::command]`）。

### A. 已恢复的 Tauri/插件命令曲面（明文，159 条，来自 carved JS + capabilities ACL）
- **window** (plugin:window)：create / show / hide / close / destroy / center / maximize / minimize / unmaximize / unminimize / toggle_maximize / start_dragging / start_resize_dragging、inner/outer position&size、is_* 状态查询、set_position/size/title/decorations/fullscreen/always_on_top/skip_taskbar/shadow/badge/progress_bar/effects/cursor_*、scale_factor、available/current/primary_monitor、request_user_attention 等。
- **webview** (plugin:webview)：create_webview / create_webview_window / reparent / set_webview_*（position/size/zoom/focus/background_color/auto_resize）/ webview_show/hide/close/position/size / **clear_all_browsing_data**（登出清理用）。
- **menu** (plugin:menu)：new / append / prepend / insert / remove / popup / set_text/icon/checked/enabled/accelerator / set_as_app_menu|window_menu|windows_menu_for_nsapp|help_menu_for_nsapp。
- **tray** (plugin:tray)：new / get_by_id / remove_by_id / set_icon(_as_template|_with_as_template) / set_title / set_tooltip / set_visible / set_menu / set_show_menu_on_left_click / set_temp_dir_path。
- **app** (plugin:app)：name / version / tauri_version / identifier / bundle_type / default_window_icon / supports_multiple_windows / **set_app_theme** / set_dock_visibility / fetch_data_store_identifiers / remove_data_store / app_hide / app_show。
- **path** (plugin:path)：basename / dirname / extname / join / normalize / resolve / **resolve_directory** / is_absolute。
- **resources** (plugin:resources)：close。
- **event** (plugin:event)：emit / emit_to / listen / unlisten。
- **image** (plugin:image)：new / from_bytes / from_path / rgba / size。

### B. 业务命令集（来自 `ipc_commands_detailed.json` + exe 内 Rust 字面量交叉验证，capabilities 未显式列但 Rust 侧已注册）
- **media_library**：`media_search` / `media_page` / `media_count` / `media_upsert` / `media_clear` / `media_fts`（lumi-store.db FTS5）。
- **kv_state**：`kv_get` / `kv_set` / `kv_all` / `kv_delete`（lumi-store.db `kv` 表；会话令牌落此处）。
- **playback**：`get_video_preview_play_info` / `get_subtitle_info`。
- **cloud_direct_link / account_auth**：`direct_cloud_login_create` / `get_file_list115` / `batch_rename115` / `get_download_url` / `get_res_download_url` / `download_info` / `refresh_token`（状态机：missing/expired/hasToken/canRefresh/running）。
- **backend**：`api_3`（Lumi Cloud Emby 兼容代理，117.72.12.20:9651/api/3，混淆头 X-Emby-Token/api_key/deviceid…）。
- **fs_system / config_theme**：`resolve_directory` / `set_app_theme`。
- **streamhub 编排**：`streamhub-start` / `streamhub-status` / `streamhub-base-url`（Rust 拉起/探活/暴露 127.0.0.1:<动态端口>）。
- **媒体源直连**：`emby-request` / `jellyfin-request` / `plex-request` / `cloud-http-request` / `preheat-url` / `emby-msid` / `ffmpeg-probe` / `webdav-probe`。

---

## §3 会话生命周期（登录 / 刷新 / 登出，双鉴权体系）

### 3.1 三套鉴权并存
| 体系 | 端点 | 令牌形态 | 前端/谁持有 |
|---|---|---|---|
| **Lumi Cloud `api_3`** | `/api/auth/login` + `/api/user/login`（117.72.12.20:9651） | `access_token` + `device_id` + `UserId` | Rust 经 `api_3` 代理转发；令牌入 kv |
| **StreamHub 本地** | `/api/auth/{login,refresh,logout}`（127.0.0.1:<port>） | JJWT `Bearer access_token`（`type=="access"`） | StreamHub 自管；桌面代理模式免登录 |
| **云盘直链**（115/阿里/夸克/123/天翼/Emby/Jellyfin/Plex/feiniu） | 各厂商 OAuth/refresh | `access_token`+`refresh_token`+过期时间 | Rust 命令；`.enc` DPAPI 加密 |

### 3.2 登录
- **api_3**：前端（经 Rust `api_3` 代理）POST `/api/auth/login`（或 `/api/user/login`），参数含 `device_id`/`deviceID`；返回 `access_token`+`UserId`+`device_id`。Rust 代理对 `login`/`account` 端点有白名单拦截（`unsupported_auth_endpoint`/`invalid_account_endpoint`）。
- **StreamHub**：桌面代理模式（`-Dstreamhub.desktop-proxy.enabled=true`，Rust 永远设置）→ `SecurityConfig` 走 permitAll 分支，**localhost 免登录**直连所有端点。严格模式才需 `/api/auth/login` JWT。
- **云盘直链**：`direct_cloud_login_create` 换 `refreshToken`/`accessToken`/过期时间。

### 3.3 令牌存储（关键）
- `__LUMI_SESSION_AUTH__` = **Rust 侧 kv 键**（`lumi-store.db` 的 `kv` 表），与 `auth_mode`、`emby-credentials.enc`/`jellyfin-credentials.enc`/`plex-credentials.enc` 同组出现（exe 内数据结构实锤）。它持有当前激活的 **Lumi Cloud 会话令牌 blob**（`access_token`+`device_id`+`UserId`），经 `kv_set` 写入、`kv_get` 读取。
- `cloud-accounts.json`：账户索引（与 kv 并存）。
- `.enc` 凭据文件：**Windows DPAPI**（`Crypt32Util` + Base64）加密，与 OS 账户绑定，存储 Emby/Jellyfin/Plex/feiniu 凭据。
- **WebView2 存储**：因云调用经 Rust 代理而非 webview 直连，会话令牌**不落 WebView2 cookie**；仅 `plugin:webview|clear_all_browsing_data` 作为登出时的浏览数据清理手段。

### 3.4 刷新
- **云盘直链**：`refresh_token` 命令按状态机刷新（`access_token_expires_at`/`refreshTokenExpiresAt` 驱动），复用厂商 token/refresh 端点。
- **StreamHub**：`/api/auth/refresh`（JWT 无状态，SecurityConfig 列入 PUBLIC_AUTH_PATHS）；桌面代理模式不强制。
- **api_3**：`access_token` 刷新机制**未在静态证据中确认**（api_3 文档 §6 标 pending）；推测由 Rust 在 `__LUMI_SESSION_AUTH__` 内维护过期并复用本地 refresh 逻辑。

### 3.5 登出
- api_3：前端触发 `kv_delete("__LUMI_SESSION_AUTH__")` 清除会话令牌（必要时配合 `clear_all_browsing_data`）。
- StreamHub：`/api/auth/logout`（存在但未在桌面代理模式强制）。
- 云盘：失效/删除对应 `.enc` 与 kv 中的 provider token。

### 3.6 `__LUMI_SESSION_AUTH__` 定义
Rust `lumi-store.db` 的 `kv` 主键，值为已登录 Lumi Cloud 账户的会话令牌结构（access_token / device_id / user_id / 可能含过期与 authorToken/session_token）。它是连接「前端 UI → Rust `api_3` 代理 → 远端 117.72.12.20」会话状态的总开关。
