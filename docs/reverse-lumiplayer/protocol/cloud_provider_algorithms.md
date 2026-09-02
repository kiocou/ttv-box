# LumiPlayer — Cloud Drive / Subtitle / Intro-Skip 直链与签名算法恢复

> 来源：`lumiplayer-tauri.exe` 字符串（`analysis/strings_ascii.txt`）+ §12.2 端点映射。
> 全局结论：**端点 / OAuth·QR 流程 / 关键请求头 / 常量（client_id、RSA 模数、API key/salt）均为静态可恢复**；但各家 `sign` / `x-nd-authorization` 的**具体计算公式以编译后的 Rust 代码实现，字符串中无公式字面量**（未出现 `sign=md5(...)` 之类）。凡涉及 `sign` 值生成的，必须抓包 / hook `direct_cloud.rs`（`src\direct_cloud.rs` 已确认）动态恢复。

---

## 1. 阿里云盘 (aliyun)
- **端点**：QR 登录 `passport.aliyundrive.com/newlogin/qrcode/generate.do` + `/qrcode/query.do`；token `auth.aliyundrive.com/v2/account/token`、`api.aliyundrive.com/token/refresh`；用户 `user.aliyundrive.com/v2/user/get`、`openapi.alipan.com/adrive/v1.0/user/getDriveInfo`；下载 `api.aliyundrive.com/adrive/v2/file/get_download_url`、`openapi.alipan.com/adrive/v1.0/openFile/getDownloadUrl`、`api.aliyundrive.com/v2/file/get_video_preview_play_info`；列表 `api.aliyundrive.com/adrive/v3/file/list`；重命名 `openapi.alipan.com/adrive/v1.0/openFile/update`。
- **鉴权**：OAuth2 QR，`appName=aliyun_drive`、`fromSite=52`。用 `refresh_token` 换 `access_token`（标准 aliyun open 授权码/刷新流）。请求头 `Authorization: Bearer <access_token>` + `X-Canary: client=web,app=adrive,version=v6.8.12`（**静态可恢复**）。
- **签名**：无自定义请求签名（标准 Bearer）。
- **刷新**：`api.aliyundrive.com/token/refresh` 持 `refresh_token` 续期。
- **置信度**：HIGH。动态缺口：refresh 请求体字段（`grant_type=refresh_token` 等）为标准值，风险低。

## 2. 115
- **端点**：QR 登录 `passportapi.115.com/app/1.0/web/1.0/login/qrcode`、`qrcodeapi.115.com/api/1.0/web/1.0/token`、`qrcodeapi.115.com/get/status/time`；列表 `aps.115.com/natsort/files.php`；重命名 `webapi.115.com/files/batch_rename`、`aps.115.com/nd.bizuserres.s/v1/file/rename`；视频 `115.com/api/video/m3u8?definition=0`、`webapi.115.com/files/video`、`webapi.115.com/files/download`；下载签名 URL `aps.115.com/nd.bizuserres.s/v1/get_res_download_url`、`proapi.115.com/app/chrome/downurl`。
- **鉴权**：Cookie 体系（UID/CID/SEID/KID，`%mEA` 回调格式）；Native API 头：`Accept: application/json`、`X-Trim-Client-Version`、`x-nd-client-unique-id`（设备 ID）、`x-nd-authorization`（**签名头**）、`authorization`。
- **签名**：
  - 登录凭据加密：硬编码 **RSA-2048 公钥模数**（完整静态恢复）：
    `8686980c0f5a24c4b9d43020cd2c22703ff3f450756529058b1cf88f09b8602136477198a6e2683149659bd122c33592fdb5ad47944ad1ea4d36c6b172aad6338c3bb6ac6227502d010993ac967d1aef00f0c8e038de2e4d3bc2ec368af2e9f10a6f1eda4f7262f136420c07c331b871bf139f74f3010e3c4fe57df3afb71683115`（指数假定 0x10001）。用于登录密码加密。
  - `x-nd-authorization` 头值 = 请求签名，**公式未入字符串（编译）**；Web `get_res_download_url` 经典为 `sign=md5(pickcode+ts+secret)`，本文未证实。
- **刷新**：Cookie/SEID 续期，依赖 passport 刷新。
- **置信度**：MEDIUM（端点/头/RSA 模数静态可恢复；签名公式需动态抓包）。

## 3. 百度网盘 (baidu)
- **端点**：QR 登录 `passport.baidu.com/v2/api/getqrcode`、`/v2/api/qrcode`、`/v3/login/main/qr`、`/v3/login/api/auth/?return_type=5&tpl=netdisk&u=https://pan.baidu.com/`、`/channel/unicast`（`sign`+`gid` 轮询）；列表 `pan.baidu.com/rest/2.0/xpan/file`（`method=list`、`app_id=250528`、`bdstoken`）、`/api/list`、`rest/2.0/xpan/multimedia`（filemetas）；`pan.baidu.com/api/filemeta`；重命名 `pan.baidu.com/api/filemanager`(`/file/rename`)、`pan.baidu.com/disk/main`。
- **鉴权**：`BDUSS`/`BDUSS_BFESS` Cookie → `access_token`；`app_id=250528`；`bdstoken`（CSRF，来自 account info）。
- **签名**：passport channel 用 `sign`+`gid`（`channel/unicast`），`v3/login/main/qr` 出现 `algsig`/`shaOne`/`elapsed7` —— 确认登录 `sign` 走 **SHA-1**（`shaOne`）。精确公式属百度私有、已编译，未入字符串。
- **刷新**：`access_token` + `bdstoken` 续期（标准 BAIDU 流）。
- **置信度**：MEDIUM 端点；签名 LOW（需抓包）。

## 4. 夸克 (quark)
- **端点**：QR 登录 `uop.quark.cn/cas/ajax/getTokenForQrcodeLogin`（`client_id=532`,`v1.2`）、`uop.quark.cn/cas/ajax/getServiceTicketByQrcodeToken`、`pan.quark.cn/tck`、`su.quark.cn/4_eMHBJ?token=`；账户 `pan.quark.cn/account/info`、`/data/members/service_ticket`；文件 `drive-pc.quark.cn/1/clouddrive/file/sort`、`/file/rename`、`/file/v2/play`（返回 `url_expire_sec`）。
- **鉴权**：CAS 式：`client_id=532` → `getTokenForQrcodeLogin` → `getServiceTicketByQrcodeToken` → `pan.quark.cn/tck` → `service_ticket` → `account/info`；Cookie 会话。`request_id` 随 CAS 调用。
- **签名**：`file/v2/play` 播放 URL 服务端签名（返回 `url_expire_sec`）；客户端无 sign 公式入字符串。
- **刷新**：service_ticket/Cookie 续期。
- **置信度**：MEDIUM 端点；签名 LOW。

## 5. 123 云盘
- **端点**：Web/登录 `yun.123pan.cn/`、`user.123pan.cn/centerlogin`、`user.123pan.cn/api/user/qr-code/result`；device_code 流 `urn:ietf:params:oauth:grant-type:device_code` + `client_id=aMe-8VSlkrbQXpUR`；API `api.123278.com/b/api`：`/file/list/new`（`orderBy`,`orderDirection=asc`,`parentFileId`,`trashed`）、`/file/download_info`（返回 `FileDownloadUrl`）。常量 `LoginUuid=lumi-player-123pan`、`env=prod`、`source=123pan`、`type=login123`、`uniID`。
- **鉴权**：QR 或 device_code OAuth；`uniID`=用户 ID；令牌存 `authorToken`/`accessToken`。
- **签名**：开放 API（`api.123278.com/b/api`）经典为 `signature=md5(排序参数+secret)`，未入字符串。
- **刷新**：OAuth token 续期。
- **置信度**：MEDIUM 端点（`client_id` 静态可恢复）；签名 LOW。

## 6. 天翼云 (tianyi / 189)
- **端点**：OAuth2 QR `open.e.189.cn/api/logbox/oauth2/appConf.do`（`appId`,`appKey`,`version=2.0`）、`getUUID.do`（`uuid`,`encryuuid`）、`image.do?uuid=`、`qrcodeLoginState.do`（`REQID`,`reqId`,`appKey`,`cloudlt`,`clientType`,`isOauth2`,`cb_SaveName`,`loginUrl`,`cookie`,`appId`,`datetimeStamp`,`state`,`paramId`,`returnUrl`）；`cloud.189.cn/api/portal/loginUrl.action`；列表 `cloud.189.cn/api/open/file/listFiles.action`（`mediaType`,`pageNum`,`pageSize=500`,`iconOption=5`,`descending`,`X-Requested-With`）；下载 `cloud.189.cn/api/open/file/batchGetFileDownloadUrl.action`（`fileIds`,`pickcode`）、`/api/open/file/getFileDownloadUrl.action`。
- **鉴权**：OAuth2 Web QR；`appId`/`appKey` 为天翼开放平台凭据（客户端硬编码）。
- **签名**：请求带 `sign` + `datetimeStamp`（时间戳），经典为 `md5(规范参数字典序+secret/appKey)`，公式已编译未入字符串。`datetimeStamp` 存在确认基于时间戳签名。
- **刷新**：OAuth token 续期。
- **置信度**：MEDIUM 端点/参数；签名 LOW。

## 7. 光鸭 (guangyapan)
- **端点**：`www.guangyapan.com`、`account.guangyapan.com`、`api.guangyapan.com`。字符串足迹极少，行为近似自定义/代理后端（同类 Feiniu）。
- **鉴权/签名**：字符串中无具体流程或签名线索。
- **置信度**：LOW（仅基础 URL 可恢复）。动态缺口：需抓包确定登录与直链签名。

## 8. assrt (字幕)
- **端点**：`https://api.assrt.net/v1`（`/infos/languages`、`/download`、搜索）。
- **鉴权**：`assrt API Token` 作为 `Authorization: Bearer <token>`（用户提供）。**无签名算法**，仅 API token。
- **置信度**：HIGH。

## 9. OpenSubtitles
- **端点**：`https://api.opensubtitles.com/api/v1`。
- **鉴权**：`Api-Key` 请求头 + `Authorization: Bearer <JWT>`（用户登录）。无客户端签名。
- **置信度**：HIGH。

## 10. Intro/Outro 跳过
- **AniSkip**：`https://api.aniskip.com/v2/skip-times` — `episodeId`(anilist)、`types`(op/ed)、可选 `episodeNumber`；GET 无鉴权。
- **IntroDB**：`https://api.introdb.app/segments` — 按 anilist/mal id；无鉴权。
- **skipdb.tv**：`https://api.skipdb.tv/api/segments?imdb_id&season&episode&duration` — IMDB 体系；无鉴权。
- **TheIntroDB**：`https://api.theintrodb.org/v3/media?tmdb_id&duration_ms` — 亦可用用户 Emby 服务器作源（`embyServerUrl`）；无鉴权。
- **置信度**：HIGH（端点/参数静态可恢复；均无签名）。

---

## 附：Feiniu (飞牛 NAS，凭据文件 `feiniu-credentials.enc` 提及，非 TARGET ⑥ 但同源)
- 签名常量**完整静态恢复**：`FEINIU_API_KEY=16CCEB3D-AB42-077D-36A1-F355324E4237`、`FEINIU_AUTH_SALT=NDzZTVxnRKP8Z0jXg1VAMonaG8akvh`；模板 `nonce=&timestamp=&sign=`。公式未入字符串（应 `sign=HMAC/MD5(nonce+timestamp+salt)`），需动态确认。

## 动态恢复缺口汇总
| Provider | 需动态抓包/hook 的内容 |
|---|---|
| 115 | `x-nd-authorization` 值算法；`get_res_download_url` 的 `sign` |
| 百度 | passport `sign`（SHA-1）参数字典与 secret；netdisk `sign` |
| 夸克 | `file/v2/play` 播放 URL 签名 |
| 123 | `api.123278.com` 的 `signature` 算法 |
| 天翼 | OAuth2 请求 `sign` + `datetimeStamp` 公式 |
| 光鸭 | 整个登录/直链流程 |
| 阿里/字幕/intro-skip | 无需（标准 Bearer / 无签名） |
