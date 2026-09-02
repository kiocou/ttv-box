# TTV Box 元数据刮削

媒体入库后可通过 `library_scrape` 批量匹配元数据，写回本地 SQLite 的 `media` 表：

- `TVMaze`：默认启用，无需密钥，提供剧名、首播年份、评分、海报和简介。
- `TMDB`：设置 `TTV_TMDB_READ_TOKEN`（推荐）或 `TTV_TMDB_API_KEY` 后启用，优先匹配电影和剧集，并提供海报、背景图、原名和简介。
- `JAV (JavBus/JavDB/Avmoo/JavLibrary/Jav321/sehuatang)`：默认启用，无需密钥。只有文件名带有**强 18+ 信号**（如 `IPX-633`、`ipx633`、`FC2-PPV-1234567`、纯数字 `012345-678`）或已被分类器标为成人的条目才会改走成人元数据源，不再交给 TVMaze/TMDB。普通影视即使文件名碰巧像番号，也不会进入深夜档。
- `18+ 元数据`：扫描时会识别来源 JSON 中的 `adult` / `isAdult` / `contentRating` 字段及常见分级标记，写回 `adult`、`contentRating`、`genres`；启用 TMDB 时会同时允许成人结果检索。未匹配到公共条目的资源仍会保留 18+ 标签，不会丢失真实播放地址。

桌面端流程已经自动接入：

1. 扫描本地目录后自动刮削。
2. 光鸭云盘文件夹导入后自动刮削。
3. StreamHub 媒体库同步后自动刮削。
4. 媒体库工具栏的“刮削元数据”按钮可重跑批量匹配。

TMDB 密钥只从进程环境读取，不写入配置文件、SQLite 或前端 IPC 返回值。

影视库扫描会自动开启成人结果检索参数，但只对带有明确 18+ 标记的媒体跳过 TVMaze，避免把成人资源误匹配成普通剧集。手动编辑详情页也可以勾选“标记为 18+ 内容”，该标记会保存到媒体记录的 `payload` 中。

Windows PowerShell 示例：

```powershell
$env:TTV_TMDB_API_KEY = "你的 TMDB API key"
npm run tauri:dev
```

## JAV 元数据（18+ 隔离刮削）

番号识别与多源刮削移植自 JavBoss（参考实现位于 `.tmp-javboss/`），实现在 `src-tauri/src/adult/`：

- `code.rs`：从文件名提取番号候选。支持有码（`ABP-356`、`ipx633`、`ABP-888C`）、无码（`Heyzo-0945`、`Tokyo Hot n0646`、`051626-001`）与 FC2（`FC2-PPV-1234567`、`FC2-1234567`）命名。
- `javbus.rs`：主源，解析 JavBus HTML，得到标题、发行日期、时长、标签、演员、系列、制作商、发行商、导演与封面 URL。带 `age=verified` Cookie、Chrome UA 与 Referer，识别反爬 driver-verify 跳转。
- `javdb.rs`：第二源，搜索后跟进精确番号详情页，补演员、片商、评分与封面；JavBus 被 driver-verify 拦住时最常用。
- `avmoo.rs`：结构化 JSON 备源（CSRF token + 会话 Cookie，30 分钟会话缓存，最多重试 3 次）。
- `javlibrary.rs`：中文站备源，强于导演 / 制作商 / 发行商。
- `jav321.rs`：无码友好备源（Caribbean / 1pondo / Heyzo 等 JavBus 常 404 的番号）。
- `sehuatang.rs`：末位备源，走用户主要资源站 sehuatang.net（98堂）的 Discuz 游客搜索。网络层用 **OS 自带 curl.exe 子进程**（每 lookup 一个临时 cookie jar）：该站前置 Cloudflare,实测对 rustls 指纹、乃至 hyper+native-tls(Schannel) 的 h2/头部栈都回 "Just a moment" 403,只有 curl.exe（Schannel + HTTP/1.1）能过（2026-08-29 A/B 实测）；解析层仍用 scraper 对落盘 HTML 做结构化解析。gate 处理：首请求命中 JS gate 页（内嵌 `var safeid='…'`）则把 `Cookie: _safe=<safeid>` 以 Netscape 格式追加进 jar 重放（无需执行 JS）。Discuz 搜索提交是「302 + Set-Cookie（saltkey/lastvisit/lastact）+ 重定向回同 URL」模式,重定向回访必须带会话 Cookie,否则触发「搜索过于频繁」限频页（含 `alert_error`,按瞬时错误处理不写负缓存）;游客搜索限频约 30 秒,`searchsubmit` 请求有独立 31 秒限速器。标题剥掉 `[HD/4.34G]` 这类体积/画质标记与番号本身,保留 `[无码破解]`/`[中文字幕]` 等有效信息;线程页封面只认 Discuz 附件图（`zoomfile`/`file` 属性）并排除头像（`uc_server`/`avatar`）,图床防盗链需要 `Referer: https://sehuatang.net/`（`cover.rs` 按 provider 注入）。版块名（亚洲无码/有码原创,重写格式 `forum-36-1.html`）用于推断有无码。仅当前面五个源全部无命中时才会触达。DNS 污染网络下需 `HTTPS_PROXY=socks5h://…`（curl 子进程遵循同一环境变量,`socks5h` 由代理解析域名）。
- `cover.rs`：封面下载，校验 ≥30KB、`.tmp` 原子改名写入，存到 `{data_dir}/covers/{小写番号}.jpg`。
- `mod.rs`：`lookup_jav` 按 JavBus → JavDB → Avmoo → JavLibrary → Jav321 → sehuatang 顺序尝试，命中后会把后续源的缺失字段（演员、封面、评分、简介）合并进来（中文标题优先）；`Ok(Some)` 命中、`Ok(None)` 全部源明确 404（可负缓存）、`Err` 存在瞬时失败且无命中（不缓存、下轮重试）。全局每源限速。

刮削语义：

- 命中：写回 `title`、`original_title=番号`、`year`、`rating`、`duration_seconds`、`art_url`（本地封面优先），`payload` 记录 `scrapedBy=jav`、`externalId=番号`、`summary`（源站剧情或由番号/演员/厂商/标签合成的真实简介）、`adult=true`、`contentRating=18+`、`genres=标签`、`metadataSource=jav:{provider}` 与完整 `jav` 对象（演员/制作商/发行商/导演/系列/时长/评分/封面 URL 等）。
- 未命中或瞬时失败：仍标记 `adult=true`、`contentRating=18+` 以从主库隔离，保留番号候选供下轮重试；瞬时失败不写 `scrapedBy`，保证可被再次刮削。
- 缓存：`metadata_cache` 键 `v1:jav:{番号}`，命中 90 天、未命中 7 天。
- 封面失败不致命：条目仍带全部元数据入库，深夜档卡片显示「未刮削」角标并可通过 `adult_cover_fetch` 按需重试。已知问题：Avmoo 封面走 `jp.netcdn.space` CDN，该源偶发整体宕机（Cloudflare 521，2026-08-27 实测）；此时默认管线不受影响——JavBus 为主源且其图床（`javbus.com/pics`）独立可用，仅当某条目只有 Avmoo 命中时封面会暂时缺失，待 CDN 恢复后重试即可。

前端 18+ 隔离与「深夜档」隐藏页：

- 常规影视库、搜索、继续观看、追剧与首页轮播一律过滤 `adult` 条目。
- 连续点击左上角 LumiPlayer Logo 6 次进入「深夜档」隔离页（P 站风格竖版封面卡片、番号/时长角标、标签/演员/厂商筛选、排序、搜索）；深夜档内点击 Logo 或「退出深夜档」按钮退出，进出场带过渡动效。该页不写入 `location.hash`，不出现在常规导航。
- 「刮削缺失项」按钮调用 `library_scrape {providers:["jav"], mediaIds:[...], includeAdult:true}` 补全未匹配条目；卡片封面加载失败时按需调用 `adult_cover_fetch` 重新下载。详情页会把番号、演员、制作商、发行日、时长、标签和真实简介直接铺进作品资料区，不再显示“已从本地目录导入”这类占位文案。

本地联调（真实访问 JavBus/Avmoo）：

```bash
cd src-tauri
cargo run --example jav_smoke -- "ABP-356"
```
