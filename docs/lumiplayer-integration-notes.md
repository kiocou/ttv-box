# LumiPlayer 兼容实现说明

本项目没有调用或绕过 LumiPlayer 的私有账号服务；以下结论来自仓库内的逆向报告、已存在的运行时资源和本地代码实现。

## 播放链路

1. 优先使用 WebView2 `<video>` 播放浏览器支持的 H.264/AAC、HLS 和远程直链。
2. 本地 MKV 或浏览器不支持的编码先交给 `player_prepare_browser_media`。
3. FFmpeg 会选择 `h264_nvenc`、`h264_qsv`、`h264_amf` 或 `libx264`；如果源视频已经是 8-bit H.264，则只复制视频流并将音频规范化为 AAC，避免整片重编码。
4. 输出为带独立分片和音频默认轨的 fMP4 HLS，前端使用 hls.js 加载。
5. 浏览器层无法播放时，才将文件交给 libmpv actor。libmpv 使用专用线程、缓存、重连和 `aid=auto`/`mute=no` 默认音频策略。

## 元数据平台

- `TVMaze`：默认公共来源，不需要 API key，负责剧集名称、年份、评分、海报、播出平台和简介。
- `TMDB`：只有设置 `TTV_TMDB_API_KEY` 时启用，用于电影/剧集的海报、背景图、原名、简介和评分补充。
- `StreamHub`：本机媒体中心和文件/版本来源，不是元数据公共平台；其媒体卡片会携带 `metadata.streamhub`、版本列表和选集映射。
- 光鸭及其他云盘 provider：只负责登录、目录分页和播放地址解析，未验证的 provider 不会被标记为已连接，也不会伪造元数据。

## 数据契约

媒体记录写入 SQLite `media` 表，来源信息放在 `payload.providerId`、`payload.mediaId` 和 `payload.metadata`。刮削完成后写入 `payload.scrapedBy`、`externalId`、`summary`、`sourceTitle`。前端会用源文件名中的 `SxxExx`、`1x02` 或中文集数标记把单集记录合并成剧集卡，原始文件仍保留为可播放选集。

## 与 LumiPlayer 行为对齐的部分

- libmpv 专用 actor 和动态加载的 `libmpv-2.dll`。
- `gpu-next,gpu`、D3D11、缓存、网络重连和着色器/增强入口。
- 播放地址解析、来源 session 恢复、过期后单次刷新重试。
- 播放历史、收藏、版本选择和选集切换均以本地真实数据为准。

