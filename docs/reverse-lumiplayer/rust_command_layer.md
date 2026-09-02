# Rust 命令层与真实业务逻辑（逆向重建 · 第二阶段核心）

> 来源：从 `lumiplayer-tauri.exe` 原始字节中，用「锚点 + 字段契约」法恢复的 serde 数据模型、
> 错误/日志串、加密算法名、解析路由上下文。这些是**真实业务逻辑**，不是 UI 文案。
> 注意：Rust 二进制 strip 了符号，无源码；以下为静态恢复，动态验证见末尾。

---

## 1. 播放解析路由（resolveProvider 分发树）

二进制 `cache-hit` 上下文揭示了完整的源类型分发逻辑：
```
resolveProvider / resolveContext
  ├─ directProvider      (直链源：光鸭/115/阿里/夸克/天翼 等网盘直链)
  ├─ cloudProvider       (云盘经解析后端 api/3)
  ├─ media-server        (Emby / Jellyfin / Plex)
  ├─ local               (本机文件 / WebDAV)
  └─ remote              (远端流)
路由还区分：
  · "routed through the system-proxy path"  ← 走系统代理
  · "direct request failed" / "system proxy request failed"  ← 代理回退直连
```

**播放信息协商（PlaybackInfo）字段**（`cache-hit` 上下文）：
`guid, title, itemNo, playable, mediaType, seasonNumber, episodeNumber, rawUrl, originalUrl,
poster, backdrop, overview, duration, httpHeaders, redirect, outcome, cache-hit,
playbackInfoMs, redirectMs, plexMs, resolvedSource, thumbnailExtraction`。

→ 还原的播放流程：
```
前端 POST "play info" {guid, mediaType, seasonNumber, episodeNumber, ...}
  → Rust resolveProvider() 判定源类型
  → 若 media-server: 向 Emby/Jellyfin/Plex 发 PlaybackInfo 请求
       (带 X-Emby-Token / embyRequiredHttpHeader; 计时 playbackInfoMs/redirectMs/plexMs)
  → 若 cloud: 调 api/3 或各网盘 API 拿直链
  → 代理策略: 媒体流量可 bypass 系统代理 (mediaProxyMode/by{ass,b}ass)
  → 返回 playable URL (m3u8 / direct / redirect) + httpHeaders
```

## 2. 媒体 / TV 层级数据契约（serde 字段）

```
original_title, mediaType, overview, description, vote_average,
watched, watched_tsts (观看时间戳),
seasonNumber, season_number, number_of_seasons, local_number_of_seasons,
numberOfEpisodes, number_of_episodes, local_number_of_episodes, localNumberOfEpisodes,
parent_guid, grand_guid, parentTitle, ancestor_guid, ancestor_name, single_child_guid,
video_guid, mediaGuid, media_guid, fileName,
genres, backdrops, posters, background, genre_items, is_watched, logos, genre,
duration, runtime, release_date, air_date,
provider (示例值 feiniu), tv_title, parent_title
```
这是 TV 剧集层级：`episode → season(parent_guid) → show(grand_guid/ancestor_guid)`，
`single_child_guid` 用于单集单文件场景。对应命令 `media_search/page/count/upsert` 与 StreamHub 实体。

## 3. mpv HDR / 色调映射渲染配置（serde 字段 + 取值）

```
target-colorspace-hint, target-colorspace-hint-mode, target-colorspace-hint-strict,
tone-mapping, autogamut-mapping-mode (auto-gamut-mapping-mode),
hdr-compute-peak, hdr-contrast-recovery, hdr-contrast-smoothness,
dither-depth, dither, fbo-format, d3d11-output-format
取值样本: rgba16hf, rgb10_a2, perceptual
```
→ 重建时这些映射到 mpv 的 `--tone-mapping`, `--target-colorspace-hint`, `--dither`,
`--fbo-format`, `--d3d11-output-format` 等。HDR 失败时二进制有专用错误：
`HDR_APPLY_FAILED`, `property INTERPOLATION_CONFLICT`, `HDR video-reconfig`。

## 4. 播放控制命令（命令名 + 业务动作）

从 `hdr-compute-peak` 上下文恢复的命令名（前端可调）：
```
set_playback_speed, set-video-quality, set-video-preset, set-audio-preset,
set-subtitle-style, set-presentation, set-advanced-interpolation, set-frame-interp,
get_audio_tracks, get_subtitle_tracks, get_subtitle_tra[cks],
toggle-always-on-top, always-on-top-on/off/disable/enable, toggle-fullscreen,
maximize-toggle, topbar-maximize,
x-authorization, proxy-authorization, proxy-authenticate,
web-player-drag-window, libmpv-embedded / tauri-libmpv-emb
```
窗口嵌入相关错误：`embedded player WRY host not available`, `embedded player WRY host not found`,
`SetWindowPos hide/show failed`, `invalid top-level`, `parked`。

## 5. 加密与凭据（.enc）

- `.enc` 文件：`emby-credentials.enc / jellyfin-credentials.enc / plex-credentials.enc / feiniu-credentials.enc`
- 二进制内 AES 上下文为 **OpenSSH PEM 风格**（`AES-256-CBC`, `DEK-Info`, `aes128/192/256-ctr/cbc`）——
  但这是 **ssh2 / 库代码路径**，非 .enc 本体算法。`.enc` 实际解密在 **StreamHub JAR 的 `CredentialCryptoService`**（Java 侧），Rust 侧只负责存储与透传 `LUMI_SESSION_AUTH__`。
- 媒体服务器连接配置字段：`embyServerUrl, embyAccessToken, privateGatewayMode, proxyMode, mediaProxyMode, auth_mode`。
- 光鸭盘：`direct-cloud-auth.json` **明文**；`cloud-accounts.json` 明文索引。

### 5.1 媒体服务器鉴权头混淆（运行时 XOR/位移解码）
二进制中 Emby/Jellyfin 鉴权头名以混淆形式存储，运行时解码。原始窗口呈现 `[H..H..H3..H3O..H9]` 标记结构，
解码后对应明文头集合（已从 `X-Emby-Token` 上下文确认）：
```
x-emby-authorization, X-Emby-Token, x-emby-token,
playsessionid, starttimeticks, deviceid,
api_key, accesstoken, x-nd-authorization, x-nd-client-unique-id
```
配套请求头：`Accept: application/json`, `X-Trim-Client-Version`, `userAgent`。
Rust 侧逻辑：用混淆串表构造媒体服务器鉴权头 → 调 `/media/range/...`、`PlaybackInfo` 等端点。
重建时可直接用明文头名（无需复刻混淆），但头集合本身必须一致。

### 5.2 RIFE / MEMC 帧插值管线
二进制内：`MEMC_RIFE_DML.vpy`, `MEMC_MVT_LQ.vpy`, `scripts/`, `LUMI_LIBMPV_PATH`, `libmpv-2.dll`, `mpv-1.dll`
→ 帧率提升/光流补帧走 **vpy 脚本 + libmpv**；`libmpv-2.dll` 实际加载，`mpv-1.dll` 备用。
`rife`/`AI`/`RIFE`/`MVTools` 标记确认。重建时帧插值模块应独立实现（vpy + mpv `vf` 链）。

## 6. 网络层逻辑

- UA：`LumiPlayer-Standard`
- 代理回退：`system proxy request failed: <e>; direct request failed: <e>`
- 空响应：`empty HTTP response`
- 代理环境变量识别：`HTTP_PROXY/http_proxy/HTTPS_PROXY/https_proxy/ALL_PROXY/all_proxy/NO_PROXY/no_proxy`
  （来自 reqwest 库，但 app 有自定义 bypass：`lumi_bypass_proxy_emby`, `media_bypass_proxy`）
- 字节范围：`Content-Range`, `416 Range Not Satisfiable`, `206 Partial Content`（断点续传/分段）

## 7. 外部播放器枚举值

`namespacepotplayervlc` + `exempc-hcmpc-be mpcvlcVLCPotPlayer` → 枚举值：
`potplayer, vlc, mpc-hc, mpc-be, mpc, vlc`（`externalPlayer` 配置项，复用 `00-tauri-electron-compat.js` 探测结果）。

## 8. 着色器 / AI 管线（RIFE/MEMC）

二进制内：`MEMC_RIFE_DML`, `MEMC_MVT_LQ`, `vpyscripts`, `LUMI_LIBMPV_PATH`, `libmpv-2.dll`, `mpv-1.dll`
→ 帧插值/RIFE 超分走 vpy 脚本 + libmpv 路径；`libmpv-2.dll` 是实际加载的 mpv（`mpv-1.dll` 备用）。

## 9. 残留待解（动态分析才能定）

| 项 | 静态结论 | 动态验证手段 |
|----|---------|-------------|
| `.enc` 解密密钥派生 | 在 StreamHub `CredentialCryptoService`（Java） | 逆向 JAR / 运行时 dump 解密后的凭据 |
| 各 provider 直链算法 | 命令名 + OAuth 端点已知，算法内部未知 | 抓包 + hook `resolveProvider` |
| 完整自定义命令枚举 | ~50 已恢复，估计 60-90 | 运行时 hook `__TAURI_INTERNALS__.invoke` |
| 播放链路端到端 | 路由树已还原 | 真实播放 + Fiddler 抓包 |
| 插件/更新系统 | 未见明确协议串 | 运行时网络监控 |

## 10. 代理处理（真实网络逻辑）

- 库：`reqwest-0.12.5/src/proxy.rs` + Windows 注册表 `Software\Microsoft\Windows\CurrentVersion\Internet Settings\ProxyEnable\ProxyServer`。
- 环境变量识别：`HTTP_PROXY/http_proxy/HTTPS_PROXY/https_proxy/ALL_PROXY/all_proxy/NO_PROXY/no_proxy`（CGI 下 `HTTP_PROXY` 被忽略）。
- 自定义 bypass：`lumi_bypass_proxy_emby`, `media_bypass_proxy` —— 媒体流量不走系统代理。
- 回退：`system proxy request failed: <e>; direct request failed: <e>`，UA `LumiPlayer-Standard`。

## 11. 重建优先级建议
1. **先复刻数据契约**（§2/§3 字段表）—— 前端↔Rust IPC 的 JSON schema 是 1:1 重建的硬约束。
2. **复刻 resolveProvider 路由树**（§1）—— 这是业务逻辑主干。
3. **复刻 mpv HDR 配置映射**（§3）—— 画质等价的关键。
4. **凭据层**：统一用 OS keychain 替代 .enc 明文混合方案。

## 12. 云盘 Provider 端点地图 + 本地数据契约（第三阶段深化）

> 来源：本阶段对 `lumiplayer-tauri.exe` 原始字节做「命令名 → 上下文窗口」挖掘，直接恢复出各 provider 的真实 HTTP 端点、本地 SQLite 表结构与字段、以及会话/凭据模型。这是 1:1 功能重建的硬约束。

### 12.1 本地数据契约（SQLite：`lumi-store.db`）
- PRAGMA：`journal_mode=WAL`、`synchronous=NORMAL`、`foreign_keys=ON`
- 表 `media`：`id, account_id, library_id, kind, title, sort_key, year, art_url, payload(JSON), updated_at`
  - 索引：`idx_media_page ON media(account_id, library_id, kind, sort_key, id)`（分页=索引范围扫描，无排序）
  - 全文检索：`media_fts` 为 `fts5(title, id UNINDEXED, tokenize='unicode61')`
  - 关键 SQL：
    - `media_search`：`SELECT j FROM media WHERE id IN (SELECT id FROM media_fts WHERE media_fts MATCH ?1) ORDER BY sort_key ASC LIMIT ?2`
    - `media_page`：`SELECT ... FROM media ORDER BY sort_key , id LIMIT ? OFFSET ?`（参数：`libraryId, kind, offset, limit, desc`）
    - `media_count`：`SELECT COUNT(*) FROM media`
    - `media_upsert`：`INSERT INTO media(id,account_id,library_id,kind,title,sort_key,year,art_url,payload,updated_at) VALUES(?1..?10)` + 同步 `media_fts`
    - `media_clear`：`DELETE FROM media_fts WHERE id IN (SELECT id FROM media WHERE account_id=?1); DELETE FROM media WHERE account_id=?1`
- 表 `kv`：`key TEXT PRIMARY KEY, value, updated_at`
  - `kv_get`：`SELECT value FROM kv WHERE key=?1`
  - `kv_set`：`INSERT INTO kv(key,value,updated_at) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at`
  - `kv_all`：`SELECT key, value FROM kv`

### 12.2 云盘 Provider 直连接点（Rust 直达，不经过 StreamHub）
| Provider | 关键端点 | 用途 |
|---|---|---|
| 阿里云盘 (aliyun) | `api.aliyundrive.com/v2/file/get_video_preview_play_info`、`openapi.alipan.com/adrive/v1.0/openFile/getDownloadUrl`、`api.aliyundrive.com/token/refresh`、`auth.aliyundrive.com/v2/account/token`、`user.aliyundrive.com/v2/user/get` | 预览播放信息 / 下载 URL / token 刷新 |
| 115 | `aps.115.com/natsort/files.php`（文件列表）、`webapi.115.com/files/batch_rename115`、`webapi.115.com/files/video`、`115.com/api/video/m3u8?definition=0`、`aps.115.com/nd.bizuserres.s/v1/get_res_download_url` | 列表/重命名/视频m3u8/签名URL |
| 百度网盘 (baidu) | `pan.baidu.com/rest/2.0/xpan/multimedia`、`pan.baidu.com/api/filemeta`、`pan.baidu.com/disk/main`、`pan.baidu.com/api/filemanager`(`/file/rename`) | 媒体元信息/重命名 |
| 夸克 (quark) | `drive-pc.quark.cn/1/clouddrive/file/rename`、`drive-pc.quark.cn/1/clouddrive/file/v2/play`、`pan.quark.cn` | 重命名/播放 |
| 123 云盘 | `yun.123pan.cn/.../file/download_info`（返回 `FileDownloadUrl`） | 下载信息 |
| 天翼云 (tianyi) | `cloud.189.cn/api/open/file/batchGetFileDownloadUrl.action`、`cloud.189.cn/api/ope...` | 批量下载 URL |

- 清晰度档位（`get_download_url`）：`slow, normal, high, super, 2k, 4k`；封装支持：`fmp4_av, m3u8, dolby_vision`。
- 鉴权上下文字段（从 `refresh_token`/登录流恢复）：`refreshToken/accessToken/accessTokenExpiresAt/refreshTokenExpiresAt`、`driveId/resourceDriveId/backupDriveId/rootId`（阿里）、`pickcode`（115）、`nick_name/user_name`、`provider/waiting`。
- 账号状态机：`missing / expired / hasToken / canRefresh / expiresAt`、`running / initialized`、`tampered / ok`、`fingerprint / features`。

### 12.3 自有托管后端 `api/3`（Emby 兼容）
- 端点：`http://117.72.12.20:9651/api/3`（硬编码，命令 `api_3` 代理）。
- 协议：Emby 兼容（`/Items/`、`/PlaybackInfo`、`api_key`、`UserId`、`X-Emby-Token`）。
- 混淆请求头（字节中以 XOR/位移存储，标记 `H3`/`H3O`/`H9`）：`x-emby-token`、`api_key`、`playsessionid`、`starttimeticks`、`deviceid`、`lumi_ret`。
- 含义：LumiPlayer 运营一个远端 Emby 兼容媒体服务，作为"自带内容源"；`api_3` 是客户端与之通信的桥。

### 12.4 会话与凭据模型
- 会话键：`__LUMI_SESSION_AUTH__`（内存/进程级会话令牌）。
- 凭据文件（`.enc`，OS/账户绑定加密，Windows 即 DPAPI，与 StreamHub `CredentialCryptoService` 同源思路）：`emby-credentials.enc`、`jellyfin-credentials.enc`、`plex-credentials.enc`、`feiniu-credentials.enc`。
- 账户聚合：`cloud-accounts.json`（多 provider 账户的统一存储）。
- 授权/许可：`license.dat`、`.secure-timestamp`、`license-backup`。
- Emby 恢复逻辑：`[emby-recovery] entered first`、`missing-token-context`、`playback-info outcome=cache-hit|renderer-confirmed-source|fast-start-raw-url`。

### 12.5 播放器后端与帧插值
- 后端标记：`libmpvBackendActive`、`nativeMpv`、`nativeMpvEmbedded`、`nativeFallbackDisabled` —— 表明播放走 libmpv，且存在"原生 mpv"与"内嵌 WRY(WebView) 回退"双路径。
- 帧插值/超分脚本：`MEMC_RIFE_DML.vpy`、`MEMC_MVT_LQ.vpy`，置于 `scripts/`，经 `LUMI_LIBMPV_PATH` 指定的 `libmpv-2.dll`（备用 `mpv-1.dll`）。
- 外部播放器交接：`external-player-handoff`、`external-player`、`web-player-start`、`web-player-state-changed`。

### 12.6 播放列表状态机（命令 `playlist_info` / `playlist_index`）
- `playlist_info`：`currentIndex, len, playlistPreviousPath, playlistPreviousOptions, embyMediaSourceResolved, playlistSelection, playlistPreviousIndex`。
- `playlist_index`：`index, previous_index, len, title, updated_playlist, startSeconds, requestedPlaylistIndex, playlistRollback, recoveredPrevious`。
- 流程：`[web-player-load-playlist-index]` → `state.playlist_info written currentIndex=` → `[switch]` → `embyMediaSourceResolved` → `playlistSelection`；失败有 `playlistRollback`/`recoveredPrevious` 兜底。

## 13. resolveProvider 状态机（完整调用链 · S 级 ①）

> 来源：`lumiplayer-tauri.exe` 字节中 `resolveProvider`/`resolveContext` 上下文串（@0x01411de0 / @0x01419d68 / @0x0145c750 等）。
> **命令符号**为 `resolveProvider` / `resolveContext`（Rust 内部符号，camelCase；Tauri v2 默认保留函数名作 IPC 命令名。确切下划线/驼峰形式以 `generate_handler!` 注册为准，内部符号已确证为 `resolveProvider`）。

### 13.1 触发与输入（resolveContext）
前端发起播放解析，传入上下文对象 `resolveContext`，字段实测含：
`baseUrl, serverUrl, accessToken, itemGuid, mediaType, seasonNumber, episodeNumber`，以及播放目标 URI（如 `feiniu-lazy://playlistItem` 自定义 scheme，标识来自播放列表项）。
解析产出播放描述符，字段集（字节确证）：
`guid, title, itemNo, playable, mediaType, seasonNumber, episodeNumber, rawUrl, originalUrl, poster, backdrop, overview, duration, httpHeaders, redirect, outcome, cache-hit, playbackInfoMs, redirectMs, plexMs, resolvedSource, thumbnailExtraction`。

### 13.2 源类型路由（分支）
| 分支 | 含义 | 真实协议去向 |
|---|---|---|
| `directProvider` | 网盘直链 | Rust 直调各 provider API（见 §12.2）拿直链 |
| `cloudProvider` | 云盘经解析后端 | `api/3`（Emby 兼容）或 provider 解析服务 |
| `media-server` | Emby / Jellyfin / Plex | 媒体服务器 PlaybackInfo |
| `local` | 本机文件 / WebDAV | 本地直读 |
| `remote` | 远端流 | Lumi Cloud `api_3` |

### 13.3 outcome 终态枚举（实测日志格式串）
`outcome=` 完整取值（字节确证）：
`cache-hit | renderer-confirmed-source | fast-start-raw-url | ok | complete | fallback | error | cancelled | deadline | begin`
- `cache-hit`：命中本地缓存（lumi-store.db kv / 历史解析），直接返回。
- `renderer-confirmed-source`：渲染层已确认可用源（Plex/Emby 已验证直链）。
- `fast-start-raw-url`：直接以 `rawUrl` 起播，跳过 PlaybackInfo 协商。
- `ok`：完整解析成功（默认终态）。
- `complete`：阻塞式解析完成（日志 `blocking-start budgetMs=45000`，45s 预算内完成）。
- `fallback`：主路径失败回退备用源/代理。
- `error` / `cancelled` / `deadline`：失败 / 用户取消 / 超时。

### 13.4 各分支真实协议
**Emby / Jellyfin（media-server）**
- 请求：`GET /Items/{itemGuid}/PlaybackInfo`，参数 `api_key, UserId, EnableDirectPlay/EnableDirectStream/EnableTranscoding, MediaSourceId, PlaySessionId`。
- 取 `MediaSources[].DirectStreamUrl`（及源信息）；`RequiredHttpHeaders` → 存入 `embyRequiredHttpHeaders`（随源返回的附加鉴权头，如 `X-Emby-Token`）。
- 计时：`playbackInfoMs`（PlaybackInfo 耗时）、`redirectMs`（302 重定向耗时）、`plexMs`（Plex 分支耗时）。
- 日志格式：`[startup] <provider>: <plexMs> ms / Emby PlaybackInfo <playbackInfoMs> ms / 302 <redirectMs> ms`。
- `forceEmbyPlaybackInfo`：强制发 PlaybackInfo（即使可 fast-start）。
- `emby-resolve` 子日志：`item=.. ms=.. msIsGuess=.. proxy=.. forced=.. changed=..`（含代理决策与是否猜测）。
- `privateGatewayMode` / `fastStart` / `embyUserId` / `embyDeviceId`：Emby 私有网关、快起、用户/设备标识。

**Plex（media-server）**
- 请求：`GET /library/metadata/{ratingKey}?X-Plex-Token=<token>` → 解析 `MediaContainer` → `MediaMetadataPart`。
- 取 `DirectStreamUrl` 作播放 URL。`X-Plex-Token` 来自 Plex 凭据；`ratingKey`/`_plexRatingKey` 为媒体键。

**网盘直链（directProvider / cloudProvider）**
- 走 §12.2 各 provider 端点拿直链（m3u8 / direct / redirect）。
- 日志片段：`[stream] 115 ... .m3u8`（115 云盘 m3u8 流式）、`cloudProvider` 分支标记。
- `httpHeaders` / `redirect`：直链所需请求头与重定向目标。

### 13.5 代理 / 路由路径（实测两条）
- `remote Emby playback routed through the system-proxy media path` —— 远端 Emby 走**系统代理**媒体路径（mediaProxyMode 启用）。
- `media-server playback routed through the local direct proxy` —— 媒体服务器走**本地直连代理**（StreamHub 本地流代理 `:port/stream` 或 Rust 本地代理）。
- 对应 §10 的 `lumi_bypass_proxy_emby` / `media_bypass_proxy`：媒体流量可 bypass 系统代理直连。
- 回退：`system proxy request failed: <e>; direct request failed: <e>`（§6/§10）。

### 13.6 输出与下游
解析结果（`rawUrl`/`httpHeaders`/`resolvedSource`/`outcome`）回传前端 → 触发 `playlist_info`/`playlist_index`（§12.6）写 `state.playlist_info.currentIndex` → `libmpv` 起播（`[tauri-player] ... FILE_LOADED`）。
`thumbnailExtraction`：ffprobe 抽帧（`-ss` + `-vf scale=640:-2 -q:v 4`），失败记 `thumbnail extraction failed`。

### 13.7 播放描述符关键字段表
| 字段 | 含义 |
|---|---|
| `rawUrl` / `originalUrl` | 最终播放 URL / 原始 URL |
| `httpHeaders` / `redirect` | 请求头 / 重定向目标 |
| `resolvedSource` | 实际源类型（directProvider/cloudProvider/media-server/...）|
| `outcome` | 解析终态（§13.3）|
| `playbackInfoMs` / `redirectMs` / `plexMs` | 各阶段耗时（性能埋点）|
| `embyRequiredHttpHeaders` | Emby 附加鉴权头（随源返回）|
| `mediaSourceResolved` | 媒体源是否已解析 |
| `thumbnailExtraction` | 缩略图抽取结果 |
| `forceEmbyPlaybackInfo` / `fastStart` / `privateGatewayMode` | Emby 行为开关 |
