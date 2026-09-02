# 光鸭云盘播放器特性:分辨率切换 / 字幕搜索 / 音轨(语言)选择

> 2026-08-29 实现。依据对官方光鸭云盘桌面端 1.0.6(`app.asar` 逆向)与第三方
> Tauri 客户端(Guanya-GUI)的交叉分析,把官方播放器的三项核心能力补齐到
> TTV Box 的 libmpv 播放管线中。

## 1. 官方行为参考(逆向结论)

| 能力 | 官方实现 |
| --- | --- |
| 清晰度 | `get_file_detail` 返回 `videoResource[]`,每个清晰度是独立的 `gcid`;切换 = 用新 gcid 重取 `get_vod_download_url` 直链 + seek 回原位置(无多码率 m3u8)。`definitionId` 映射:原画=10000,按 `resolutionName` 含 240/360/480/720/1080/2K/4K/8K 取值。显示名:240P 极速 / 360P 流畅 / 480P 标清 / 720P 高清 / 1080P 超清 / 2K·4K·8K 超高清 / 原画 / 无损画质。 |
| 字幕 | 三个来源:① 在线字幕库 `POST /misc/v1/get_subtitles {gcid, name, duration秒}` → `data.list[]{url, ext, name, cid}`;② 云盘同目录字幕 `get_file_list {parentId, fileTypes:[6]}`(字幕文件类型枚举 = 6),经 `get_res_download_url {fileId}` 取直链下载;③ 内挂字幕轨。 |
| 音轨 | 内挂轨选择,官方桌面端是自研原生播放库;Web 端等价物是 mpv 的 `aid`/`alang`。语言码映射为中文展示。 |

## 2. TTV Box 实现

### 2.1 分辨率切换(真实换流,非标签)

- `providers/guangya.rs::parse_video_resources`:把 `videoResource[]` 解析为
  `VideoQuality {gcid, definitionId, resolutionName, displayName, shortName, needVipType, source, durationSeconds, isDefault}`,
  官方 definitionId/显示名映射完整复刻。
- `resolve_playback` 的画质选择(`select_resource`):请求值命中 gcid / 分辨率名 /
  显示名 / 短名 / 定义数字 → 用之;否则取 `defaultResolution=true` 的资源,
  再退回第一项,最后回退 `fileInfo.gcid`。
- `PlaybackDescriptor.qualities` 随首次解析返回完整清晰度表,前端不再需要
  第二次详情请求;`quality` 字段现在是官方显示名(如 "1080P 超清")。
- 修复:`get_vod_download_url` 的 `urlDuration`(TTL 秒)现在换算为绝对
  `expiresAt`,前端原有的"短时效直链过期自动重解析"逻辑因此真正生效。
- 前端(`前端/src/app-runtime.js`):`qualityEntriesFor()` 优先用云盘
  qualities,回退 versions 元数据;画质弹窗与灵动岛画质菜单点击后走
  `switchToQuality()`:`saveWatchProgress()` → 携带
  `playbackQualityGcid` 重新 `provider_resolve_playback` → openPlayer 按历史
  进度续播,即官方"换流不换位置"语义。VIP 专属清晰度带"会员"角标。

### 2.2 字幕搜索(在线字幕库 + 云盘同目录)

- trait 新增 `search_subtitles` / `download_subtitle`(默认
  `UnsupportedOperation`,光鸭实现于 guangya.rs)。
- 在线字幕库:`/misc/v1/get_subtitles {gcid, name, duration}`;结果
  `ProviderSubtitle {id: "online:*", source: "online", url, ext, name}`。
- 云盘同目录:按视频的 `fileInfo.parentId` 翻 5 页 `fileTypes:[6]`,过滤
  srt/ass/ssa/sub/vtt/smi/sami/sup;`ProviderSubtitle {id: "cloud:*", fileId}`。
- 下载:online 直接取 `url`;cloud 经 `get_res_download_url` 取签名直链;
  命令层写缓存 `cache_dir/subtitles/provider-<providerId>/`,之后沿用既有
  `subtitle_attach`(mpv `sub-add`)挂载。
- IPC 命令:`provider_subtitle_search`、`provider_subtitle_download`(条目
  原样回传,前端不需要解析 id 约定),都带 SessionExpired 自动重试。
- 前端字幕弹窗新增"搜索云盘字幕"入口;OpenSubtitles 与本地字幕通道保留。

### 2.3 音轨(语言)选择与实时轨道表

- `playback/mpv.rs` 每次轮询读一次 `track-list/count`,数量变化才逐条读
  `track-list/N/{id,type,lang,title,codec,selected,default,forced,ff-index}`
  (≤128 条上限),发出 `PlaybackEvent::TrackListChanged`;快照
  `PlaybackSnapshot.audioTracks / subtitleTracks` 分轨暴露。
- 前端 `liveMpvTracks()` 从 `player_state` 取实时轨道;音轨弹窗与字幕弹窗
  优先展示 mpv 轨道(轨道 id 就是 `aid`/`sid` 的值),"当前"标记来自
  快照的 `audioTrack`/`subtitleTrack`。
- 修复:旧弹窗把 ffprobe 的容器 stream index 直接当 mpv `aid` 用,在
  "视频流在音频流之前"的文件上会切错轨道;原生路径现在不再经过 probe 索引。
- 语言码 → 中文名映射(`languageDisplayName`,chi/zho/eng/jpn… 共 30 项)
  与官方客户端的语言本地化一致。

## 3. IPC 命令一览(新增)

| 命令 | 入参 | 出参 |
| --- | --- | --- |
| `provider_video_qualities` | `{providerId, input:{mediaId}}` | `VideoQuality[]` |
| `provider_subtitle_search` | `{providerId, input:{mediaId, durationSeconds?}}` | `ProviderSubtitle[]` |
| `provider_subtitle_download` | `{providerId, input:{subtitle: ProviderSubtitle}}` | `{path, source, name}` |

`provider_resolve_playback` 的 `request.quality` 现在接受 gcid 或清晰度名,
响应额外携带 `qualities`(可选项为空时缺省)。

## 4. 已知边界

- 光鸭短信登录仍未实现(`login_sms` 返回 UnsupportedOperation),登录走设备码。
- 10001 "绮丽视界"(AI 增强,VIP-only)不在清晰度表内,未实现。
- 在线字幕库匹配依赖官方索引,冷门文件可能返回空列表;云盘同目录字幕不受影响。
