# StreamHub 后端架构（com.streamhub.localapi）

> 逆向来源：`lumiplayer-tauri.exe` 同包/同发行内嵌的 Spring Boot fat JAR（包名 `com.streamhub.localapi`，应用名 `streamhub-local-api`）。
> 恢复手段：`javap -p` 提取全部类签名 + `.class` 常量池扫描 + `application.yml` 配置提取。
> 结论：**LumiPlayer 不是一个单纯的 Tauri 壳，它背后跑着一个完整的本地媒体服务后端 StreamHub。**

---

## 0. 一句话定位

StreamHub 是一个**本地运行的媒体中心后端**（类似 Jellyfin/Emby 的"私有化"版本），职责覆盖：
- 媒体库管理（扫描 WebDAV / 本地文件，TMDB 刮削元数据）
- 流媒体代理与转封装（HLS 生成、外挂/内封字幕抽取、音视频轨选择）
- 账号体系（JWT + 刷新令牌，可邮件注册）
- AI 推荐 Agent（RAG 向量检索 + LLM 推理 + 重排，可选）

它与 Rust/Tauri 前端（mpv 播放器）的关系是**互补双轨**：
- **Rust 直达轨**：`resolveProvider` 直连 Emby/Jellyfin/Plex 与网盘（115/阿里/百度/夸克），不经过 StreamHub。
- **StreamHub 媒体库轨**：桌面端启动/连接本地 StreamHub，浏览自有媒体库、走 HLS 代理播放、用 AI 推荐。

---

## 1. 运行时画像（来自 application.yml）

| 项 | 值 | 含义 |
|---|---|---|
| 监听地址 | `0.0.0.0:18400` | 本地 API，桌面端与之通信 |
| 应用名 | `streamhub-local-api` | |
| 数据库 | **SQLite** `jdbc:sqlite:./data/streamhub.db` | Hikari 连接池 size=1（SQLite 单写）|
| ORM | **MyBatis-Plus** | `mapper/` 包即 DAO 层（非 JPA）|
| 系统代理 | `use-system-proxy: true` | 出站 TMDB/网盘抓取走系统代理 |
| 缓存目录 | `./cache`（images / subtitles） | TMDB 图片与字幕缓存 |
| 调度轮询 | `scheduler.poll-ms: 60000` | 媒体源定时同步 |
| 邮件 | `smtp.qq.com:587` STARTTLS | 注册码/重置密码默认走 QQ 邮箱，可改 |
| 私有配置 | `application-private.yml`（gitignored） | 覆盖 TMDB key / JWT secret / 邮件 |
| 虚拟线程 | `spring.threads.virtual.enabled: true` | JDK21 虚拟线程 |
| 异步超时 | `mvc.async.request-timeout: 10m` | 流式/代理长连接 |

---

## 2. 安全模型（SecurityConfig + AuthService）

- **认证方式**：`AuthTokenFilter`（JWT Bearer）挂在 `UsernamePasswordAuthenticationFilter` 之前；CSRF 关闭，CORS 开启。
- **令牌**：
  - Access Token = **JWT**（**HS512**，密钥取自 `STREAMHUB_JWT_SECRET` 环境变量，空则首次启动自动生成并落库 `AppSettingEntity` 的 `APP_SETTING_JWT_SECRET`），**有效期 15 分钟**。算法经运行时实测确认为 HS512（token 头 `eyJhbGciOiJIUzUxMiJ9` = `{"alg":"HS512"}`），旧 HS256 推测作废。
  - Refresh Token = **随机串，SHA-256 哈希后入库**（`RefreshTokenEntity`），cookie 名 `streamhub_refresh_token`，**有效期 30 天**，支持**轮换**（字段 `replacedByTokenId` / `revokedAt`）。
  - 用户 `tokenVersion` 字段：改密后自增，可一键吊销全部会话。
- **账号**：
  - 角色 `ROLE_USER` / `ROLE_ADMIN`；状态 `PENDING` / `ACTIVE` / `DISABLED`。
  - 注册需邮件验证码（`EMAIL_PURPOSE_REGISTER`，10 分钟有效，60 秒重发限流）。
  - 登录失败 5 次锁 15 分钟（`failedLoginCount` / `lockedUntil`）。
  - `hasAnyUser()`：首个注册用户自动成为管理员（典型单机私有部署）。
- **桌面模式**：`desktopProxyEnabled`（`streamhub.desktop` 相关）标志，桌面端走本地代理鉴权，便于本地进程免登录调用。
- **凭据加密（已证实）**：媒体源密码等敏感字段经 `CredentialCryptoService` 加密存储；Windows 实现 `WindowsCredentialCryptoService` 用 **JNA `Crypt32Util.cryptProtectData/cryptUnprotectData`（Windows DPAPI）+ Base64**。即 `.enc` 凭据仅在加密它的 Windows 用户账户下可解密，属 OS 级账户绑定保护，非自定义 AES。

---

## 3. 技术栈

- Spring Boot 3.x + Spring Security 6 + Spring MVC
- MyBatis-Plus（DAO），SQLite（JDBC 驱动 `org.sqlite.JDBC`）
- JWT：`io.jsonwebtoken`（JJWT）
- WebDAV 客户端：`Sardine`（`SardineWebDavService`）
- 缓存：Caffeine（`forgotPasswordAttempts` 等限流）
- RAG（可选，默认关闭）：Qdrant / Meilisearch 向量库 + OpenAI 兼容 Embedding（`text-embedding-3-small`）+ Reranker
- AI 模型（可配置多组）：scraper（刮削补全标题）、agent（推荐）、agent-clue（线索推理），均走 OpenAI 兼容 `/v1` 接口

---

## 4. 数据模型（Entity → SQLite 表，共 13 张）

| 实体 | 表 | 关键点 |
|---|---|---|
| `UserEntity` | user | username/email/passwordHash/role/status/emailVerified/tokenVersion/failedLoginCount/lockedUntil |
| `RefreshTokenEntity` | refresh_token | tokenHash/deviceName/userAgent/ipAddress/expiresAt/revokedAt/replacedByTokenId |
| `EmailVerificationEntity` | email_verification | tokenHash/codeHash/purpose/expiresAt/usedAt/attemptCount |
| `PasswordResetEntity` | password_reset | tokenHash/expiresAt/usedAt |
| `AppSettingEntity` | app_setting | settingKey/settingValue（含 JWT secret、TMDB/AI 配置缓存）|
| `MediaSourceEntity` | media_source | **name/type/url/username/encryptedPassword/rootPath/scanIntervalMinutes/enabled/enableScheduledSync/connectionStatus/lastSyncTime** |
| `MediaFileEntity` | media_file | sourceId/mediaRefId/remotePath/fileName/fileSize/etag/lastModified/**subtitlePaths**/scrapeStatus(scrapeQuality/reasonCodes/attemptCount/failureCode) |
| `MovieEntity` | movie | tmdbId/title/year/ratings(IMDB/Trakt/RT)/genresJson/castJson/directorsJson/collectionItemsJson/recommendationsJson… |
| `TvShowEntity` | tv_show | 同 Movie + totalSeasons |
| `TvEpisodeEntity` | tv_episode | showId/seasonNumber/episodeNumber/title/durationSeconds |
| `WatchHistoryEntity` | watch_history | userId/mediaId/mediaType/mediaFileId/progressSeconds/lastWatchedAt |
| `AgentFeedbackEntity` | agent_feedback | queryText/feedbackType/mediaType/tmdbId/evidenceJson/gateStatus/routeSourcesJson |
| `AgentPreferenceSignalEntity` | agent_preference_signal | signalType/targetType/targetValue/polarity/weight/confidence/decayHalfLifeDays |

> 注：表名由 MyBatis-Plus 默认（实体名下划线化）推断，实际以运行时 `./data/streamhub.db` 为准。文件位于 StreamHub 工作目录（随安装路径，典型在 `LumiPlayer` 数据目录下 `data/`）。

---

## 5. 核心子系统

### 5.1 流媒体代理 `StreamProxyService`（播放链路核心）
- `openVideo(mediaFileId, range)` → `ProxiedStream`：从媒体源（WebDAV/本地/上游）拉流并按 Range 代理输出（支持断点续传、HEAD）。
- `resolvePlayableUri(mediaFileId)` / `playable` 端点：返回可直接播放的 URL（直链或 HLS）。
- `ensureHlsPlaylist(mediaFileId, audioStreamIndex)`：生成 HLS master/分片（自适应码率播放）。
- `ensureEmbeddedSubtitle(mediaFileId, streamIndex)` / `openSubtitle`：抽取内封字幕为独立资源。
- `getTrackInfo(mediaFileId)` → `MediaTrackInfoResponse`：**`useHlsPlayback` + 默认音轨/字幕轨索引 + 外挂字幕类型 + 音轨/字幕轨列表**（与 Rust 侧 `get_video_preview_play_info` 的协商逻辑对应）。
- 播放列表重写（`rewritePlaylist*`）：为多集剧集拼接 m3u8，注入带 access token 的媒体 URL。

### 5.2 媒体源与刮削
- `MediaSourceService`：媒体源 CRUD、连通测试、触发同步、触发 AI 重刮。
- `WebDavService`：列举 WebDAV 视频资源、打开流、查找字幕（目前明确的 source type 之一）。
- `TmdbService`：TMDB 搜索最佳匹配 / 拉取元数据 / 拉取季度剧集；支持 HTTP 缓存。
- `scrape` 包：LLM 批量推理（`LlmBatchInferenceService`）、各类缓存（ParseCache/SeasonCache/MatchCache/NegativeCache）、单飞去重（`MetadataSingleFlightService`）、重试策略（`ScrapeRetryPolicy`）、评分指标（`ScrapeMetricsRecorder`）。
- `SyncOrchestratorService` / `ScheduledSyncTask`：队列化同步、AI 重刮、失败修复（`repairFailedScrapes`）。

### 5.3 AI 推荐 Agent（`recommendation` 包）
- `RecommendationAgentService.recommend(query, limit, Consumer<AgentStreamEventDto>)`：SSE 流式返回推荐，带 `RetrievalPlan`（检索规划）、分组、诊断。
- 证据融合（`EvidenceFusion`）、证据门控（`EvidenceGate`）、硬约束（`HardConstraints`）、线索解析（`ClueResolutionResolver`）、候选池（`CandidatePool`/`CandidateSlot`）、用户画像（`AgentUserProfileService`/`AgentPreferenceProfile`）。
- 后端可选：Qdrant/Meilisearch 向量检索 + Embedding + Reranker；偏好信号按 `decayHalfLifeDays` 衰减学习。

### 5.4 账号 / 设置 / 系统
- `AuthService`：`sendRegisterCode`/`register`/`login`/`refresh`/`logout`/`me`/`forgotPassword`/`resetPassword`。
- `AppSettingsService`：解析 TMDB / MdbList / AI（scraper/agent/clue 三组 baseUrl+apiKey+model+timeout+promptVersion）/ 播放器偏好。
- `ImageStorageService`：TMDB 图片本地缓存。
- `SyncEventService`：SSE 事件总线（同步进度等推送前端）。
- `SystemController`：`/health`、`/thread-pools` 指标（Prometheus 暴露）。

---

## 6. 与 Rust 前端的边界（关键结论）

| 能力 | 谁负责 |
|---|---|
| Emby/Jellyfin/Plex 直连播放 | **Rust**（`resolveProvider` → media-server）|
| 网盘（115/阿里/百度/夸克）直连 | **Rust**（cloud provider commands）|
| 本地/WebDAV 文件媒体库 + 元数据刮削 | **StreamHub** |
| HLS 代理 / 字幕抽取 / 轨选择 | **StreamHub**（mpv 播放 StreamHub 出流的 URL）|
| 账号 / 观看进度 / AI 推荐 | **StreamHub** |
| mpv 实际解码渲染（HDR/MEMC/RIFE） | **Rust + libmpv** |

> StreamHub 不必处理 Emby/网盘 —— 那是 Rust 直达轨。StreamHub 是"自有片库 + 智能服务"轨。两者由同一桌面端统一呈现。

---

## 7. 待动态确认项
1. StreamHub 是由 `lumiplayer-tauri.exe` 以子进程方式启动，还是独立安装的服务？（查安装目录有无 `streamhub*.jar` 与启动脚本）
2. `streamhub.desktop` 标志的精确语义（免登录信任 vs 固定桌面令牌）。
3. `MediaSourceEntity.type` 的全部取值（目前确认有 WebDAV；是否含 local/smb/emby 需运行时或 scrape 包确认）。
4. `./data/streamhub.db` 实际建表 SQL（MyBatis-Plus 自动建表 `sql.init.mode: always`，可能含 `schema.sql`）。
