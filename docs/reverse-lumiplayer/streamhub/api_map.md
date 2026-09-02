# StreamHub HTTP API 地图（端口 18400，base `/api`）

> 端点路径来自 controller `.class` 常量池扫描；方法名（authoritative）来自 `javap -p`；
> HTTP 动词（GET/POST/PUT/DELETE）按 Spring REST 约定**推断**，部分未直接提取注解，已标注。
> 鉴权：除 `/api/system/health` 与 `/api/auth/**` 外，均需 Bearer JWT（Access Token）。

---

## 1. 账号 `/api/auth`（AuthController → AuthService）
| 方法(推断) | 路径 | 入参 DTO | 返回 | 说明 |
|---|---|---|---|---|
| POST | `/api/auth/send-register-code` | SendRegisterCodeRequest | MessageResponse | 发送注册验证码（邮件）|
| POST | `/api/auth/register` | RegisterRequest(username,email,password,verificationCode) | AuthTokensResponse(accessToken,expiresIn,user) | 注册并签发会话 |
| POST | `/api/auth/login` | LoginRequest(account,password) | AuthTokensResponse | 登录（失败5次锁15分）|
| POST | `/api/auth/refresh` | (cookie: streamhub_refresh_token) | AuthTokensResponse | 刷新访问令牌（轮换）|
| POST | `/api/auth/logout` | (cookie) | MessageResponse | 吊销刷新令牌 |
| GET | `/api/auth/me` | (Bearer) | AuthUserDto | 当前用户 |
| POST | `/api/auth/forgot-password` | ForgotPasswordRequest | MessageResponse | 发重置邮件 |
| POST | `/api/auth/reset-password` | ResetPasswordRequest | MessageResponse | 重置密码 |

## 2. 首页 `/api/home`（HomeController → LibraryService）
| 方法 | 路径 | 返回 | 说明 |
|---|---|---|---|
| GET | `/api/home` | HomeResponse(spotlight,recent[],categories{}) | 首页聚光灯+最近+分类 |

## 3. 媒体库 `/api/library`（LibraryController → LibraryService）
| 方法 | 路径 | 返回 | 说明 |
|---|---|---|---|
| GET | `/api/library/movies/{id}` | MediaDetailDto | 影片详情 |
| GET | `/api/library/shows/{id}` | MediaDetailDto | 剧集详情（含 seasons/episodes/versions）|
| GET | `/api/library/categories/{category}` | MediaPageResponse | 按分类分页 |
| GET | `/api/library/recent` | List<MediaCardDto> | 最近添加 |
| GET | `/api/library/browse` | LibraryBrowseResponse | 浏览（多筛选参数）|
| POST | `/api/library/{type}/{id}/rematch` | RematchRequest | 重新匹配 TMDB |
| GET | `/api/library/{type}/{id}/rematch/search` | List<RematchCandidateDto> | 搜索匹配候选 |

> `MediaDetailDto.versions`: `List<MediaFileVersionDto>` —— 同一媒体可有多个文件版本（不同清晰度）。

## 4. 媒体源 `/api/sources`（MediaSourceController → MediaSourceService）
| 方法 | 路径 | 入参 | 返回 | 说明 |
|---|---|---|---|---|
| GET | `/api/sources` | — | List<MediaSourceResponse> | 列出全部源 |
| POST | `/api/sources` | MediaSourceRequest(name,url,username,password,rootPath,scanIntervalMinutes,enableScheduledSync) | MediaSourceResponse | 新增 |
| PUT | `/api/sources/{id}` | MediaSourceRequest | MediaSourceResponse | 修改 |
| DELETE | `/api/sources/{id}` | — | — | 删除 |
| POST | `/api/sources/{id}/test` | — | ActionResponse | 连通测试 |
| POST | `/api/sources/{id}/sync` | — | ActionResponse | 触发同步扫描 |
| POST | `/api/sources/{id}/ai-rescrape` | — | ActionResponse | 触发 AI 重刮 |

## 5. 刮削修复 `/api/scrape`（ScrapeRepairController → SyncOrchestratorService）
| 方法 | 路径 | 入参 | 返回 | 说明 |
|---|---|---|---|---|
| POST | `/api/scrape/repair` | RepairFailedScrapesRequest | RepairFailedScrapesResponse | 重刮失败项 |

## 6. 设置 `/api/settings`（SettingsController）
| 方法 | 路径 | 说明 |
|---|---|---|
| GET/PUT | `/api/settings/tmdb` | TMDB 设置（apiKey/baseUrl/imageBaseUrl）|
| POST | `/api/settings/tmdb/test` | 连通测试 |
| GET/PUT | `/api/settings/ai` | AI 设置（scraper/agent/clue 三组 baseUrl+apiKey+model+timeout+promptVersion）|
| POST | `/api/settings/ai/agent/test` | Agent AI 连通测试 |
| POST | `/api/settings/ai/agent/clue/test` | Clue AI 连通测试 |
| GET/PUT | `/api/settings/mdblist` | MdbList 评分设置 |
| GET/PUT | `/api/settings/player` | 播放器偏好 |
| POST | `/api/settings/ratings/test` | MdbList 评分连通 |
| POST | `/api/settings/ratings/chain/test` | 评分链测试 |
| POST | `/api/settings/ratings/backfill` | 回填外部评分 |
| POST | `/api/settings/ratings/mdblist/backfill` | 回填 MdbList 评分 |
| POST | `/api/settings/rag/rebuild` | 重建 RAG 向量索引 |

## 7. 流媒体 `/api/stream`（StreamController → StreamProxyService）—— 播放核心
| 方法 | 路径 | 返回 | 说明 |
|---|---|---|---|
| GET | `/api/stream/{mediaFileId}` | StreamingResponseBody | 代理视频流（支持 Range/断点续传）|
| HEAD | `/api/stream/{mediaFileId}` | Void | 流元信息（contentLength/range）|
| GET | `/api/stream/{mediaFileId}/tracks` | MediaTrackInfoResponse | **`useHlsPlayback`+默认音/字幕轨+外挂字幕类型+轨列表** |
| GET | `/api/stream/{mediaFileId}/redirect` | Void(302) | 重定向到上游直链 |
| GET | `/api/stream/{mediaFileId}/playable` | PlaybackTargetResponse(url) | 返回可播放 URL |
| GET | `/api/stream/show/{showId}/playlist` | Resource(m3u8) | 剧集播放列表重写 |
| GET | `/api/stream/hls/{mediaFileId}/{audioStreamIndex}` | Resource(m3u8) | HLS master 播放列表 |
| GET | `/api/stream/hls/{mediaFileId}/{path}` | Resource | HLS 分片/资源 |
| GET | `/api/stream/{mediaFileId}/subtitle` | StreamingResponseBody | 外挂字幕 |
| GET | `/api/stream/{mediaFileId}/subtitle/{streamIndex}` | Resource | 内封字幕抽取 |

> mpv 实际消费的是 `/playable` 或 `/hls/...` 返回的 URL —— StreamHub 做"源协议→HTTP/HLS"的归一化，屏蔽 WebDAV/上游差异。

## 8. 观看进度 `/api/history`（HistoryController → LibraryService）
| 方法 | 路径 | 入参 | 说明 |
|---|---|---|---|
| POST | `/api/history/progress` | ProgressRequest(mediaId,mediaType,progressSeconds,mediaFileId) | 保存观看进度 |

## 9. AI 推荐 `/api/agent`（AgentController → RecommendationAgentService）
| 方法 | 路径 | 入参 | 返回 | 说明 |
|---|---|---|---|---|
| POST | `/api/agent/recommend` | AgentRecommendRequest(query,limit) | AgentRecommendResponse(summary,plan,groups,diagnostics) | 推荐 |
| GET | `/api/agent/recommend/stream` | AgentRecommendRequest | SSE(text/event-stream) | 流式推荐 |
| GET | `/api/agent/sidebar-shortcuts` | — | SidebarShortcutResponse | 侧栏快捷推荐 |
| POST | `/api/agent/shortcut/recommend` | ShortcutRecommendRequest | ShortcutRecommendResponse | 快捷推荐 |
| POST | `/api/agent/feedback` | AgentFeedbackRequest | AgentFeedbackResponse | 反馈（用于偏好学习）|

## 10. 事件订阅 `/api/events`（EventController → SyncEventService）
| 方法 | 路径 | 返回 | 说明 |
|---|---|---|---|
| GET | `/api/events/subscribe` | SseEmitter | 同步进度/系统事件推送 |

## 11. TMDB 发现 `/api/discover/tmdb`（TmdbDiscoveryController）
| 方法 | 路径 | 返回 | 说明 |
|---|---|---|---|
| GET | `/api/discover/tmdb/lists` | TmdbDiscoveryCatalogDto | 发现榜单目录 |
| GET | `/api/discover/tmdb/lists/{listKey}/items` | TmdbDiscoveryItemsDto | 榜单条目 |

## 12. 系统 `/api/system`（SystemController）
| 方法 | 路径 | 返回 | 说明 |
|---|---|---|---|
| GET | `/api/system/health` | Map | 健康检查（permitAll）|
| GET | `/api/system/thread-pools` | ThreadPoolMetricsResponse | 线程池指标（scrape/scan 执行器）|

---

## 端点 → Service → Entity 调用链（速查）
- 登录/注册 → `AuthService` → user / refresh_token / email_verification
- 媒体库 → `LibraryService` → movie / tv_show / tv_episode / media_file / watch_history
- 媒体源 → `MediaSourceService` → media_source（+ `WebDavService` 取流）
- 播放 → `StreamProxyService` → media_file（解析 remotePath）→ proxy → mpv
- 刮削 → `TmdbService` / `LlmBatchInferenceService` → movie / tv_show / media_file(scrapeStatus)
- 推荐 → `RecommendationAgentService` → agent_feedback / agent_preference_signal（+ 可选 Qdrant/Meili）
- 设置 → `AppSettingsService` → app_setting
