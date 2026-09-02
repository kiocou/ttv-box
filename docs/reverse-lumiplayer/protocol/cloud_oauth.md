# 云盘 OAuth 二维码登录协议（逆向恢复）

来源：二进制内嵌 URL 串（百度/115/阿里/夸克/天翼）+ `direct-cloud-auth.json`(光鸭明文)。

各云盘均走 **二维码登录** 流程：App 展示二维码 → 手机 App 扫码确认 → 轮询状态接口拿 token。
注意：二进制中 URL 被无分隔符合并，下面已还原基址与关键参数。

## 1. 百度网盘 (Baidu Pan)
| 用途 | URL | 关键参数 |
|------|-----|---------|
| 获取二维码 | `https://passport.baidu.com/v2/api/getqrcode` | lppcqrlogin, from=apiver, v3, tpl=netdisk |
| 轮询扫码状态 | `https://passport.baidu.com/channel/unicast` | channel_id, tpl=netdisk, apiver=v3, tt, callback=bd__cbs__lumi |
| 取 uid | `https://passport.baidu.com/uid` | — |
| 文件列表 | `https://pan.baidu.com/api/list` | method, start, limit, order, desc, showempty, pagenum, chunlei, clienttype, app_id=250528, bdstoken |
| 文件 API | `https://pan.baidu.com/rest/2.0/xpan/file` | — |
| Cookie 名 | `BDUSS`, `BDUSS_BFESS`, `STOKEN`, `PTOKEN`, `PANPSC`, `PANWEB` | — |

## 2. 115 网盘
| 用途 | URL | 关键参数 |
|------|-----|---------|
| 获取 token/二维码 | `https://qrcodeapi.115.com/api/1.0/web/1.0/token` | — |
| 轮询状态 | `https://qrcodeapi.115.com/get/status/time` | — |
| 文件列表 | `https://aps.115.com/natsort/files.php` | offsets, show_dirs, snap, natsort, record_open_time, format=json, fc_mix |

命令证据：`get_file_list115`, `batch_rename115` —— 115 的 provider 后缀直接烤进命令名。

## 3. 阿里云盘 (Aliyun Drive)
| 用途 | URL | 关键参数 |
|------|-----|---------|
| 生成二维码 | `https://passport.aliyundrive.com/newlogin/qrcode/generate.do` | appName=aliyun_drive, fromSite=52, appEntrance |
| 轮询 | `https://passport.aliyundrive.com/newlogin/qrcode/query.do` | — |
| 刷新 token | `https://api.aliyundrive.com/token/refresh` | — |

## 4. 夸克网盘 (Quark)
| 用途 | URL | 关键参数 |
|------|-----|---------|
| 取服务票据 | `https://uop.quark.cn/cas/ajax/getServiceTicketByQrcodeToken` | 532, v1.2, poll, request_id |
| 取 token | `https://uop.quark.cn/cas/ajax/getTokenForQrcodeLogin` | client_id, 532, v1.2, request_id |
| 票据换 ticket | `https://pan.quark.cn/tck` | — |

## 5. 天翼云 (189 / Tianyi)
| 用途 | URL | 关键参数 |
|------|-----|---------|
| 主站 | `https://cloud.189.cn/web/main` | — |
| 二维码登录态 | `https://open.e.189.cn/api/logbox/oauth2/qrcodeLoginState.do` | reqId, appKey, cloud, lt, clientType, isOauth2, cbSaveName, loginUrl, cookie, appId, datetimeStamp, cb_SaveName, state, paramId |
| 配置 | `https://open.e.189.cn/api/logbox/oauth2/appConf.do` | version=2.0, appKey |

## 6. 光鸭盘 (GuangYa) — 明文直存
凭据存于 `direct-cloud-auth.json`（**明文 JSON**，安全缺陷）。登录走自定义 OAuth，命令 `direct_cloud_login_create`。
直链解析命令：`direct_url`, `direct_link_qualities`（列出清晰度变体）。

## 7. 飞牛 fnOS (Feiniu)
- 凭据：`feiniu-credentials.enc`（加密）
- 适配器文件：`30-feiniu-adapter-20260610.js`
- 代理绕过配置键：`lumi_bypass_proxy_emby`, `media_bypass_proxy`（媒体流量不走系统代理）

## 8. 重建要点
- 每家的二维码登录状态轮询字段名不同（百度 channel_id、115 status/time、阿里 query.do、夸克 request_id），需逐家适配。
- token 持久化：云盘用明文/各自格式；媒体服务器用 `.enc`（见 architecture §5）。
- 建议把「provider 适配器」做成 plugin 接口（`resolveProvider()` 分发），与架构 §4 一致。
- 命令命名暗示 provider 后缀烤进命令（如 `get_file_list115`），重建时建议改用参数化：`list_files(provider, ...)`。
