# Guangya Web 公网页面分析

分析日期：2026-08-19

范围：`https://www.guangyapan.com/` 的公开 HTML/JavaScript，以及不携带账号凭据的匿名请求。没有读取 Cookie、access token、refresh token 或本地客户端凭据。

## 已确认的服务

| 用途 | 地址 |
| --- | --- |
| 账号与 OAuth | `https://account.guangyapan.com` |
| 云盘业务 API | `https://api.guangyapan.com` |
| H5/分享页 | `https://h5.guangyapan.com` |
| 支付中心 | `https://paycenter.guangyapan.com` |

网页 SDK 使用的公开 Web OAuth client id 是 `aMe-8VSlkrbQXpUR`。它与 Token 上传文档中的开发者 `client_id` 不是同一类凭据；项目现已通过 `oauthClientId` 单独配置，避免把开发者 client id 错用到二维码登录端点。

## 二维码登录流程

1. `POST /v1/auth/device/code`
2. JSON 至少包含 `client_id`，可带 `scope`。
3. 返回 `device_code`、`user_code`、`expires_in`、`interval`、`verification_url` 和 `verification_uri_complete`。
4. 轮询 `POST /v1/auth/token`，JSON 为：

```json
{
  "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
  "device_code": "<device_code>",
  "client_id": "<client_id>"
}
```

未扫码时服务返回 OAuth 标准的 `authorization_pending`。刷新令牌使用同一 token 端点和 `grant_type: "refresh_token"`。

## 云盘业务接口

公开 bundle 中确认了这些接口路径：

- `POST /userres/v1/file/get_file_list`
- `POST /userres/v1/file/search_files`
- `POST /userres/v1/file/get_file_detail`
- `POST /userres/v1/file/get_vod_download_url`
- `POST /userres/v1/get_direct_link`

请求拦截器会附加 `Authorization: Bearer <access_token>`、`dt: 4`、`did`、`traceparent` 和 JSON `Content-Type`。匿名调用文件列表、文件详情和播放地址接口均得到：

```json
{"code":117,"msg":"无效token"}
```

这确认了业务 API 的主机、方法和 Bearer 认证边界，但没有伪造登录态去获取用户数据。

## 前端已暴露的请求字段线索

文件列表页面调用 `get_file_list` 时使用过 `parentId`、`page`、`pageSize`、`fileTypes`、`orderBy`、`sortType`、`resType`、`needPlayRecord`、`needSubFolderStat` 等字段。播放流程先调用 `get_file_detail({fileId})`，从 `videoResource` 或 `fileInfo.gcid` 取得资源 ID，再调用 `get_vod_download_url({fileId,gcid})`，成功响应使用 `data.signedURL`。

这些字段来自公开前端调用上下文；完整文件项和播放响应映射仍需要用户通过官方登录后，在自己的账号范围内完成一次动态验证。

## 项目落地

已将公开网页确认的 OAuth 常量写入 Guangya 适配器默认值和 `.ttv-data/config.json`：

- `deviceCodeGrantType = urn:ietf:params:oauth:grant-type:device_code`
- `refreshGrantType = refresh_token`
- 兼容响应字段 `verification_url`
- `oauthClientId = aMe-8VSlkrbQXpUR`

开发者 secret/token 不写入仓库、普通配置或日志。
