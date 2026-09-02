# TTV 云盘与媒体源 Provider 规范

> **规范原则**：统一抽象 `MediaProvider`，首发闭环光鸭云盘（设备码扫码与短信验证码双通道），保留主流网盘与 Emby/Jellyfin/Plex 媒体服务器标准适配接口。

---

## 1. 光鸭云盘 (Guangya Drive) 协议规范

### 1.1 通道 A：官方 OAuth2 Device-Code 扫码登录 (主通道 · 已确证)

```
[TTV 客户端]                                          [光鸭授权服务端 (account.guangyapan.com)]
     │                                                               │
     ├───── 1. POST /v1/auth/device/code ───────────────────────────▶│
     │      (client_id=ttv-cinema, scope=offline_access)             │
     │                                                               │
     │◀──── 2. 返回 device_code, user_code, verification_uri ────────┤
     │                                                               │
     ├───── 3. 渲染二维码，手机 App 扫码确认 ─────────────────────────┤
     │                                                               │
     ├───── 4. 轮询 POST /v1/auth/token (按 interval 间隔) ──────────▶│
     │      (grant_type=urn:ietf:params:oauth:grant-type:device_code)│
     │                                                               │
     │◀──── 5. 返回 accessToken, refreshToken, expiresAt ────────────┤
     │                                                               │
     └───── 6. Windows DPAPI 加密保存凭据至 SQLite ──────────────────┘
```

#### 请求头构造规范 (SDK 风格)
```http
User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36
x-client-id: ttv-cinema
x-client-version: 1.0.0
x-device-id: <本地唯一生成的 UUID 硬件标识>
x-device-name: PC-Windows
x-device-model: chrome/139.0.0.0
x-os-version: Win32
x-platform-version: 1
x-protocol-version: 301
x-sdk-version: 9.0.2
Authorization: Bearer <accessToken>
```

---

### 1.2 通道 B：短信验证码登录 (备用通道 · 兼容 Guanya-GUI)

1. **获取前置验证 Token**：`POST https://account.guangyapan.com/v1/shield/captcha/init`
2. **请求发送短信验证码**：`POST https://account.guangyapan.com/v1/auth/verification`（传入手机号与客户端标识，获得 `verification_id`）
3. **验证短信校验码**：`POST https://account.guangyapan.com/v1/auth/verification/verify`（提交短信码，换取短效 `verification_token`）
4. **最终登录**：`POST https://account.guangyapan.com/v1/auth/signin`（获取 `access_token` 与 `refresh_token`）

---

### 1.3 直链解析与清晰度档位 (`resolve_playback`)

- **端点**：`https://api.guangyapan.com/v1/file/download_url` / `get_vod_download_url`
- **清晰度档位映射**：
  - `slow`：标清 480P
  - `normal`：高清 720P
  - `high`：超清 1080P
  - `super`：超清高码率 1080P 60FPS
  - `2k`：2K 原画
  - `4k`：4K 杜比视界 / HDR 原盘
- **直链特征**：服务端预签名 HTTPS 临时地址，支持标准 HTTP `Range: bytes=start-end` 断点续传与 mpv 多线程分段缓冲。

---

### 1.4 Token 自动续期与退避重试状态机

```rust
// 当收到 HTTP 401、code=117 或 "token expired" 时触发
pub async fn handle_token_refresh(&mut self) -> Result<(), ProviderError> {
    let url = "https://account.guangyapan.com/v1/auth/token";
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": self.session.refresh_token,
        "client_id": "ttv-cinema"
    });
    
    let resp = self.client.post(url).json(&body).send().await?;
    let new_token: TokenResponse = resp.json().await?;
    
    // 更新内存与 DPAPI 数据库
    self.session.access_token = new_token.access_token;
    if let Some(r) = new_token.refresh_token {
        self.session.refresh_token = r;
    }
    self.save_encrypted_session().await?;
    Ok(())
}
```

---

## 2. 媒体服务器与扩展 Provider 规范

### 2.1 Emby / Jellyfin
- **协商接口**：`GET /Items/{guid}/PlaybackInfo`
- **鉴权头**：`X-Emby-Token: <Token>` 或 `X-MediaBrowser-Token: <Token>`
- **直链提取**：直接读取 `MediaSources[0].DirectStreamUrl`，bypass 本地转码直接喂给 libmpv。

### 2.2 Plex
- **协商接口**：`GET /library/metadata/{ratingKey}?X-Plex-Token=<Token>`
- **直链提取**：解析 `MediaContainer.Metadata[0].Media[0].Part[0].key`，构造直连播放地址。
