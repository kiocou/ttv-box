# TTV Box

面向 Windows 的影院级桌面媒体中心与播放客户端。基于 Tauri 2 + Rust + libmpv,单机运行、零外部运行时依赖:网盘直连播放、本地/NAS 媒体库、自动元数据刮削与独立影院播放窗口,全部能力内置,无需安装 Node.js、MPV 或其他运行时。

## 核心特性

- **播放核心**:libmpv-2.dll 动态链接 + Direct3D 11 渲染,支持 HLS/DASH 自适应流、外挂字幕导入(srt → vtt)、音效处理器与声场预设,主界面与独立影院播放窗口分离。
- **云盘播放器三件套(对齐官方播放器)**:光鸭云盘视频支持真实分辨率切换(videoResource 多 gcid 换流 + 进度续播)、字幕搜索(官方在线字幕库 + 云盘同目录字幕文件)与音轨/语言选择(mpv track-list 实时轨道表 + 语言中文名);详见[光鸭播放器特性](docs/guangya-playback-features.md)。
- **多源聚合**:光鸭云盘(OAuth 设备码扫码 + 短信验证码双通道)、StreamHub 媒体库同步与 OpenList 直连;更多网盘与 Emby/Jellyfin 适配见路线图。
- **本地媒体库**:目录扫描入库,SQLite(WAL + FTS5)存储,TMDB / TVMaze 自动刮削海报、简介、评分;批量重刮与缺失项补全入口在媒体库工具栏。首页大屏海报复用 `media.backdrop_url` 字段；缺失时仅对本地视频使用现有视频探针生成并缓存，不新增在线图片服务，卡片封面仍保留在 `media.art_url`。
- **18+ 隔离(深夜档)**:番号识别 + 六源元数据刮削(JavBus → JavDB → Avmoo → JavLibrary → Jav321 → sehuatang,中文标题优先合并),成人条目从常规库、搜索与首页完全隔离,隐藏页独立入口,不写入导航历史。
- **隐私与安全**:无账号体系,凭据使用 Windows DPAPI 加密落盘;TMDB 密钥只从进程环境读取,不写入配置或数据库。

## 快速开始

开发环境要求:Node.js ≥ 18、Rust stable(MSVC toolchain)、Windows 10/11。

```bash
npm install

# 前端开发服务器 + Tauri 桌面端热重载
npm run tauri:dev

# 仅构建前端到 dist/
npm run build

# 发布构建(打包安装程序)
npm run tauri:build
```

注意:`tauri.conf.json` 的 `frontendDist` 指向 `../dist`,因此修改 `前端/` 下任何页面后必须执行 `npm run build`(或走 dev 服务器)才会进入打包产物。

TMDB 刮削(可选)通过环境变量启用:

```powershell
$env:TTV_TMDB_READ_TOKEN = "你的 TMDB Read Access Token"
npm run tauri:dev
```

糖心影院的完整播放使用站点正式签发的授权凭据。凭据只由 Rust 后端从
环境变量读取，不进入前端、配置文件或日志；未配置时继续使用访客会话：

```powershell
$env:TANGXIN_JWT = "站点签发的 token 或 JWT"
$env:TANGXIN_USER_ID = "可选：findByAccount 返回的 user_id"
$env:TANGXIN_DEVICE_ID = "与该授权会话绑定的稳定 web 设备号"
npm run tauri:dev
```

`TANGXIN_JWT` 可以填写已经拼好的 `token_user_id`，也可以配合
`TANGXIN_USER_ID` 填写原始 token。站点仍会决定该授权会话返回完整清单还是试看清单；
若只返回试看，播放器会明确报错，不会伪装成完整播放。

## 项目结构

```
前端/                 原生 JS + Vite 前端(页面、播放器运行时、深夜档 UI)
src-tauri/            Rust 后端(Tauri 2 命令、业务模块)
  src/playback/       libmpv 播放核心与音效/声场预设
  src/providers/      光鸭云盘、StreamHub、OpenList 等媒体源 Provider
  src/library/        本地媒体库扫描与管理
  src/metadata.rs     TMDB / TVMaze 元数据刮削管线
  src/adult/          18+ 番号识别与六源刮削(sehuatang 为末位备源)
  src/storage/        SQLite 存储(WAL + FTS5)
  src/security/       DPAPI 凭据加密
dist/                 前端构建产物(tauri.conf.json 直接引用,勿手改)
docs/                 需求、开发规范与逆向分析文档
scripts/              播放器运行时引导与冒烟测试脚本
```

## 文档

- [产品全景与路线规划](docs/development-specs/01_产品全景与路线规划.md) — 五阶段演进路线
- [总体技术架构](docs/development-specs/02_总体技术架构与系统设计.md)
- [播放核心规范](docs/development-specs/03_播放核心与libmpv渲染规范.md)
- [云盘与媒体源 Provider 规范](docs/development-specs/04_云盘与媒体源Provider规范.md)
- [元数据刮削](docs/metadata-scraper.md) — 含 JAV 六源刮削与 sehuatang gate 说明
- [播放运行时](docs/player-runtime.md) / [OpenList 集成](docs/openlist-integration.md) / [Provider 认证矩阵](docs/provider-auth-matrix.md)

## 已知限制

- sehuatang.net 在部分网络下存在 DNS 污染，需要走代理访问：设置 `HTTPS_PROXY=socks5h://127.0.0.1:<端口>`（`socks5h` 由代理解析域名，可绕过污染；应用已启用 reqwest 的 socks 支持，该源也会遵循此变量）。该源失败时按瞬时错误处理，不会污染负缓存。
- sehuatang 为游客搜索，Discuz 限频约 30 秒一次；该源排在六源末位、仅在其余五源全部无命中时触达，批量重刮时对缺失番号会有明显间隔。
- 全流程当前仅在 Windows 上验证；macOS/Linux 未纳入测试矩阵。
