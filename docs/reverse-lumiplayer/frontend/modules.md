# 前端模块映射（115 个雕刻资产 → 业务模块）

来源：从二进制雕刻出的 `frontend_extract/assets/`（115 资产，主包 5.7MB）。
框架标记（混淆计数）：electron:5, react:4, jquery:3, vite:1 —— 推断为**自定义/vanilla 构建 + 局部 React**，经 electron-compat 垫片伪装成 electron（见 `00-tauri-electron-compat.js`）。

## 1. 核心业务模块（从文件名识别）
| 文件 | 业务模块 |
|------|---------|
| `32-lumi-cloud-media-library-module.js` | 云媒体库 |
| `41-lumi-tauri-cloud-provider-backend.js` | 云盘 Provider 后端桥接（Tauri invoke） |
| `41-lumi-cloud-file-browser.js` | 云盘文件浏览器 |
| `30-feiniu-adapter-20260610.js` | 飞牛 fnOS 适配器 |
| `09-emby-real-ui-20260527.js` | Emby 真实 UI |
| `43-lumi-media-server-auth-compat.js` | 媒体服务器鉴权兼容 |
| `37-lumi-account-media-sync.js` | 账号媒体同步 |
| `26-polaris-tracking-module-script.js` | Polaris 埋点/追踪（配置键含 `polaris`） |
| `00-tauri-electron-compat.js` | Tauri↔Electron API 兼容层 |
| `33-lumi-home-redesign.js` / `38-lumi-cinematic-search-overlay.js` / `31-lumi-unified-navbar-final.js` | UI 重设计/搜索/导航 |
| `34-lumi-mycontent-favorites.js` / `36-lumi-media-library-entitlement.js` / `42-lumi-local-media-cache.js` | 我的内容/收藏/本地缓存 |

## 2. 启动与性能模块
`00-lumi-art-cache.js`, `00-lumi-bench.js`, `00-lumi-boot-tiers.js`, `00-lumi-jank-tracker.js`,
`00-lumi-media-extensions.js`, `00-lumi-view-model-store.js`, `00-lumi-performance-kernel.js`,
`45-lumi-onboarding-tour.js`, `47-lumi-nav-visibility.js`, `36-lumi-notification-center.js`,
`48-lumi-poster-open-transition.js`, `39-lumi-library-stack-overlay.js`。

## 3. 启动影院海报
`renderer/assets/startup-cinema/poster-01..10.webp`（10 张启动画面海报）。

## 4. 云盘图标资源
`assets/cloud-providers/`: 115, 123pan, aliyun, baidu, guangya, quark, tianyi (jpg/png)。
`assets/service-icons/`: emby, feiniu, jellyfin, plex (png)。

## 5. 关键前端↔后端桥接
- `invoke(e,n,t)` 封装层（`00-tauri-electron-compat.js`）—— 命令名以变量传入。
- `window.__TAURI_INTERNALS__.invoke('plugin:window|'+cmd)` —— 插件命令拼接。
- `window.__lumiCloudFlushAndSignal('cloud-flush-complete')` / `ipcRenderer.send('cloud-flush-complete-tray')` —— 云盘刷新信号（electron-compat 残留）。
- `window.__TAURI_INTERNALS__.convertFileSrc` —— 本地文件 → web 可访问 URL（Tauri `convertFileSrc`）。

## 6. 重建要点
- 前端是 vanilla + 局部 React，用 Tauri 官方 API 重建更简单；electron-compat 层可丢弃。
- 业务模块已按文件名可识别，重建时建议按「云盘 / 媒体库 / 播放器 / 设置 / 账户」分包。
- `polaris` 是埋点服务，重建时按需替换或移除（隐私考虑）。
- 实际 JS 源码因混淆无法直接复用，本映射仅用于**功能等价重建**的模块划分参考。
