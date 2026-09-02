# 网盘官网登录流程公开分析

更新日期：2026-08-19

## 范围

本次只分析匿名访问官网时下载的 HTML、公开 JavaScript 和页面配置。没有读取、导出或复用浏览器 Cookie、登录令牌、DPAPI 数据、设备指纹或 LumiPlayer 私有数据；没有完成账号登录，也没有构造私有网页登录请求。

抓取留档位于临时目录 `C:\Users\kioco\AppData\Local\Temp\cloud-login-analysis\root`，仅供本机审计使用。

## 结论

| 来源 | 官网扫码形态 | 是否存在公开 RFC 8628 device-code 契约 | TTV Box 行为 |
| --- | --- | --- | --- |
| 百度网盘 | Passport 网页登录 SDK | 未发现 | 保留浏览器 OAuth；不生成通用二维码 |
| 阿里云盘 | Passport 嵌入式网页登录组件 | 未发现 | 保留浏览器 OAuth；不使用官网客户端标识 |
| 天翼云盘 | UDB 登录 iframe | 未发现 | 保留浏览器 OAuth；不生成通用二维码 |
| 123 云盘 | 123 App 扫码后的网页登录态 | 未发现 | 标记为未确认 OAuth；不生成通用二维码 |
| 夸克网盘 | 私有 CAS 二维码及 service-ticket 会话 | 未发现 | 不接入；只能外部打开官网登录页 |
| 115 网盘 | 私有二维码、跨域 bridge 和 Cookie 会话 | 未发现 | 不接入；只能外部打开官网登录页 |
| 光鸭云盘 | OAuth device-code | 已确认 | 已接入统一二维码创建与轮询 |

## 分来源证据

### 百度网盘

- `pan.baidu.com` 的公开脚本加载百度 Passport 通用登录 SDK，配置包含网盘产品标识和 `qrcodeLogin`。
- 扫码结果是百度网页账号 Cookie 会话；页面还会探测本机已安装客户端，不能作为桌面应用的云端 OAuth 机制。
- 百度开放平台的授权码 OAuth 可用于已登记的自有 `client_id` 和回调地址，但这与网页 Passport 扫码不是同一个协议。

### 阿里云盘

- 官网登录页使用阿里 Passport 的嵌入式网页登录组件，并把授权结果交还给官网注册的客户端和回调页。
- 官网脚本中出现的客户端标识、回调地址和 token 交换属于官网自身，不可复制给 TTV Box。
- 未看到公开的 device authorization endpoint、device code grant 或独立轮询约定。

### 天翼云盘

- 登录页通过 UDB iframe 获取网页登录地址，并携带浏览器相关参数。
- 捕获的页面与脚本中未发现公开 `device_code`、`grant_type` 或二维码轮询契约。
- 该来源只能按用户自行申请的开放平台 OAuth 授权码流程接入。

### 123 云盘

- 公开登录路由提供“使用 123 云盘 App 扫码登录”界面，二维码内容是 App 深链。
- 页面轮询的结果用于建立网站登录态；脚本没有提供公开第三方 `client_id`、device-code grant 或 refresh-token 契约。
- 现有项目 OAuth 元数据继续标记为 `unconfirmed-oauth`，不应显示二维码入口。

### 夸克网盘

- 官网脚本包含二维码创建、轮询和成功后换取网页 service ticket 的逻辑。
- 该流程要求浏览器 Cookie、CSRF 数据与官网客户端上下文，属于私有 CAS 会话，不是可移植的 OAuth 协议。
- 不应把观察到的网页接口写入 TTV Box，也不应导入网页会话。

### 115 网盘

- 官网使用二维码域、跨域 bridge、SharedWorker/iframe 与 Cookie 会话来完成扫码登录。
- 静态代码包含二维码生命周期状态，但没有公开的 RFC 8628 device-code 或第三方 token/refresh 契约。
- 不应复制该私有网页登录流程或保存其会话；无官方开放协议前保持禁用。

## 项目实施边界

`provider_qr_login_create` 和 `provider_qr_login_poll` 只接受已确认的公开 OAuth device-code 配置。必须同时具备：

1. 官方 device authorization endpoint。
2. 官方 device-code grant 值。
3. 已获批准的应用 `clientId` 与回调/权限配置。

因此，目前只有光鸭返回 `deviceCodeLogin: true`。其余来源根据能力进入浏览器 OAuth 或未支持状态，前端不会显示模拟二维码。

## 后续接入条件

百度、阿里和天翼：用户需要在各自开放平台创建并审核通过 TTV Box 的 OAuth 应用后，填写本应用专属的 `clientId`、回调地址和必要权限。123、夸克和 115：等待官方公开的第三方授权协议；不能以逆向网页会话替代官方授权。
