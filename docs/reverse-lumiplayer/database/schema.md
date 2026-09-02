# 数据库与持久化模型

## 1. StreamHub 实体模型（13 个 JPA 实体，JAR 内恢复）
来源：`allclasses.txt` (292 类) + 实体类签名。

| 实体 | 说明 |
|------|------|
| `MediaFileEntity` | 媒体文件（剧集/电影文件级） |
| `MediaSourceEntity` | 媒体源（网盘/服务器挂载点） |
| `MovieEntity` | 电影元数据 |
| `TvShowEntity` | 剧集元数据 |
| `TvEpisodeEntity` | 单集元数据 |
| `UserEntity` | 用户账号 |
| `WatchHistoryEntity` | 播放历史/进度 |
| `RefreshTokenEntity` | 刷新令牌 |
| （其余 5 个） | 分类/评分/设置关联等 |

**关系**：`MediaSource` 1—* `MediaFile`；`Movie`/`TvShow` 1—* `MediaFile`；`TvShow` 1—* `TvEpisode`；
`User` 1—* `WatchHistory`/`RefreshToken`。

**依赖**（服务层签名）：`MediaSourceServiceImpl` 依赖
`CredentialCryptoService`（.enc 解密）、`WebDavService`、`SyncOrchestratorService`、`LibraryService`。

## 2. 本地 SQLite（Rust/Tauri 侧）
证据：二进制含 `sqlite_master`, `sqlite_sequence`, `sqlite_schema`, `media_fts`（FTS5 全文索引表）,
`user_version`, `foreign_key_check`, `quick_check` 等。
媒体库使用 FTS5（`media_fts`）做标题搜索 —— 对应命令 `media_search`/`media_fts`。

**重建推测表**（需动态确认实际 schema）：
```sql
CREATE VIRTUAL TABLE media_fts USING fts5(title, sub_title, tokenize='unicode61');
-- media 表: id, guid, title, year, type(movie/show), library_id, poster, backdrop, ...
-- library 表: id(guid), name, source, path, ...
-- playlist 表: id, index, revision, items...
-- kv 表: key, value  (对应 kv_get/set/delete/all 命令)
```

配置键（二进制打包串 `value_up_ffmpegmpvpluginsresourcesshadersauthorizationcookietokenapi_keyapikeypasswordcredentialsecretpolaris`）：
`ffmpeg, mpv, plugins, resources, shaders, authorization, cookie, token, api_key, apikey, password, credential, secret, polaris` + 目录键 `app_data_dir, cache_dir, install_resource_dir, ffmpeg_dir, mpv_dir, plugins_dir, resources_dir, shaders_dir`。

## 3. 云媒体库缓存文件
`lumi_cloud_media_library_v4.json` … `v11.json`（版本化缓存，落地于 data/）。
命令/常量：`lumi_cloud_media_library_sources_v1`, `library_source`。

## 4. 凭据存储（见 architecture §5）
`.enc` 文件由 StreamHub `CredentialCryptoService` 解密；`direct-cloud-auth.json` 明文。

## 5. 重建要点
- FTS5 全文索引是媒体搜索核心，重建必须保留。
- `kv_*` 命令对应一个简单 KV 表（key-value），用于前端偏好/会话状态。
- 目录配置键暴露了运行时目录布局，重建时可直接复用。
- 实际 SQLite schema 需动态抓取（运行后读 data/*.db 或用 SQLite 工具导出）。
