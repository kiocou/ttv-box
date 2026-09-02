# 硬编码解析后端与响应归一化

## 1. 硬编码解析后端
```
Base: http://117.72.12.20:9651/api/3
协议: 明文 HTTP（非 HTTPS）—— 安全缺陷
状态: 实测存活（分析时）
```
命令证据：`api_3`（二进制字面量）。这是「资源解析」后端 —— 给定媒体页 URL，返回可播放直链。
StreamHub 本地 JAR 也对外暴露 `/api`（见 database 章的 Controller 列表），两者职责不同：
- `117.72.12.20:9651/api/3` = 远端解析（第三方资源 → 直链）
- `streamhub-local-api` = 本地媒体库/刮削/流代理

## 2. StreamHub 本地 API（Controller 端点映射）
来源：JAR 内 12 个 Controller 的 javap 端点恢复。

| Controller | 端点 | 职责 |
|-----------|------|------|
| `/api/agent` | recommend / sidebar-shortcuts / feedback / retrieval-profile / stream | AI 代理/推荐 |
| `/api/auth` | register / code / login / refresh / logout / me / forgot-password / reset-password | 账号体系 |
| `/api/events` | sync (SSE) | 事件同步 |
| `/api/history` | progress | 播放进度 |
| `/api/home` | — | 首页聚合 |
| `/api/library` | movies/{id} / shows/{id} / categories/{category} / recent / browse / rematch | 媒体库 |
| `/api/sources` | CRUD / test / sync / ai-rescrape | 媒体源管理 |
| `/api/scrape` | repair | 刮削修复 |
| `/api/settings` | tmdb / ai / mdb-list / player-preferences / ratings / rag | 设置 |
| `/api` | stream / media-files/{id} (stream/redirect/playable/hls.m3u8/subtitles/embedded) | 流/媒体文件 |
| `/api/system` | health / thread-pools | 系统 |
| `/api/discover/tmdb` | lists / items | TMDB 发现 |

**流代理服务**（`StreamProxyServiceImpl`）：
`openVideo / openSubtitle / resolvePlayableUri / describeVideo / ensureHlsPlaylist / createFfmpegProcessBuilder / evaluatePlayback / sendStreamRequest`
→ 确认 StreamHub 用 ffmpeg 做 HLS 转码/remux 并代理云流给 mpv。

## 3. 响应字段归一化（★重建关键）
二进制内嵌一份「字段提取路径表」，用于兼容不同上游 API 的响应形状。
归一化时按以下顺序尝试取值（首个命中即用）：

```
/status
/result
/code
/res_code
/data
  /status /result /code /res_code （嵌套）
/message /msg
/error /errmsg
/redirectUrl /redirect_url /toUrl
  （以上均可在 /data 下再嵌套一层）
```

即：成功判定看 `code`/`res_code`/`status`；消息看 `message`/`msg`/`error`/`errmsg`；
跳转看 `redirectUrl`/`redirect_url`/`toUrl`。所有这些都可能在 `/data` 前缀下再出现一次。

**重建实现**（伪代码）：
```rust
fn extract_code(resp: &Value) -> Option<i64> {
    for p in ["code","res_code","status","data.code","data.res_code","data.status"] {
        if let Some(v) = resp.pointer(&format!("/{}", p)) { return v.as_i64(); }
    }
    None
}
// message/error/redirect 同理，遍历各自候选路径
```

## 4. 重建要点
- 远端解析后端地址硬编码且明文 —— 重建时建议改为可配置 + HTTPS。
- 响应归一化层是「聚合多源」的必需组件，必须实现路径候选列表。
- StreamHub 端点可作为重建的本地媒体服务参考实现（Spring Boot 或换 Rust/Go）。
