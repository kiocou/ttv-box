# Lumi Cloud（api_3）逆向 —— 自有托管媒体后端

> 优先级①。来源：`lumiplayer-tauri.exe` 原始字节挖掘（命令 `api_3` + 混淆头块 + 反混淆函数 + 端点字符串）。
> 结论：**LumiPlayer 不是只有"本地 StreamHub + Rust 直连"双轨，它还有一个远端托管媒体服务层 `api_3`，把架构变成三轨。**

---

## 1. 端点（已确认）

| 服务 | 地址 | 用途（推断）|
|---|---|---|
| 主后端 | `http://117.72.12.20:9651/api/3` | 元数据 + Emby 兼容流媒体 + 鉴权（4 处命中）|
| 鉴权 | `http://117.72.12.20:9651/api/auth/login` | 登录换凭据（见 `unsupported_auth_endpoint` 上下文）|
| 二级服务 | `http://117.72.12.20:9321/4e5961726092ac0e8c620006452aee17fa8007b7` | 按 40-hex hash 提供的**内容元数据 blob**（非转码/源站）|

- 配置项：`method / body / timeoutMs / query / direct_timeout_ms / endpoint / allow_invalid_tls / bypas...`（请求构造器参数）。
- `unsupported_auth_endpoint` + `invalid url: /api/auth/login..` → 代理对某些端点（如 login）有特殊处理/拦截。

### 1.1 `:9321` 二级服务本质（已确认 = 元数据 blob，非转码/源站）

- 命中（exe @0x0141aa9e）：`http://117.72.12.20:9321/4e5961726092ac0e8c620006452aee17fa8007b7`，相邻上下文：`builtin` · `episode_number` · `no` · `episodeTitle` · `episode_title` · `index`，以及 `/api/v2/comments/` · `/comments/` · `episodeId` · `API` · `ID`。
- **结论**：`:9321` 是一个**按内容 hash（40-hex，sha1 形态）索引的元数据 blob 服务**，返回剧集级元数据（`episode_number / episode_title / index` 等）。它**不是**转码、也不是播放源站——播放源仍由 `:9651/api/3` 的 Emby 兼容 `PlaybackInfo` 给出。`:9321` 更像是运营方的内容元数据缓存/CDN（与评论 API `/api/v2/comments/` 同源体系）。
- 重建含义：Lumi Cloud 把"内容元数据"与"流媒体"拆成两个服务（9651 流 + 9321 元数据），客户端按内容 hash 拉元数据、按 Emby `Items/{guid}/PlaybackInfo` 拉流。

## 2. 协议形态：Emby 兼容 + TMDB 兼容的混合代理

- Emby 路由（直接命中）：`/Items/`、`/PlaybackInfo`、`api_key`、`UserId`、`X-Emby-Token`。
- TMDB 路由：`/tv/`、`/external_ids`、`imdbId`（元数据类型，非 Emby 原生）。
- 因此 `api_3` 既提供**内容流**（Emby 风格 PlaybackInfo），又提供**元数据**（TMDB 风格 external_ids/imdbId），是 LumiPlayer 的"自带内容源"。

## 3. 混淆请求头（XOR 字符串混淆，已破解算法）

### 3.1 混淆算法（不是"标记丢弃"，是 XOR）
反混淆函数位于 ~`0x1789d9`（邻近 `api_3` 立即数）。核心逻辑（x86-64 反汇编还原）：
```
mov rax, imm64("access_t")      ; 代码内嵌明文片段（立即数）
xor rax, [rbx]                  ; 与 .rdata 密文段异或 → 真实 8 字节
mov ecx, [rbx+8]                ; 取下一段密文
xor rcx, 0x486e656b6f ("kenH")  ; 与代码内嵌密钥异或 → "oken"
or  ecx, eax                    ; 拼接
```
即每个字符串片段 = `(代码内嵌立即数) XOR (.rdata 密文)`；密钥本身也部分内嵌在代码里（如 `"kenH"`）。这是手写/轻量 `obfstr` 模式，专为对抗字符串扫描。**ASCII 扫描里看到的 `H3/H3O/H9/H` 是密文字节（0x48/0x33/0x4F/0x39）的巧合可读渲染，不是标记**（`UAWAVVWSH` 等则是函数序言 `push rbp;push r13..` 的 ASCII 巧合）。

### 3.2 还原出的请求头/参数（confidence 标注）
| 明文 | 来源片段 | 置信 |
|---|---|---|
| `X-Emby-Token` | `X-Emby-TH` + `okenH` | 高 |
| `access_token` | `access_t` + `oken` | 高（代码中直接解码）|
| `api_key` | `_key` 常量 + `apik3` | 高 |
| `playsessionid` | `playsess` + `essionid` | 高 |
| `starttimeticks` | `starttim` + `imeticks` | 高 |
| `deviceid` / `device_id` / `deviceID` | `deviceid` + `deviceID` | 高 |
| `lumi_ret` / `lumi_retry` | `lumi_ret` + `ry`（重试逻辑）| 高 |
| `sign` | `?sign` | 中 |
| `UserId` | Emby 上下文 | 中 |
| Emby CDN 域：`emby.media` / `*.emby.me` / `by.media` | `emby.med` + `.emby.me` + `by.media` | 高 |

> 注：`x-emby-token` 与 `X-Emby-Token` 同义（大小写差异源于不同调用点）；实际发送头为 `X-Emby-Token`。

### 3.3 完整静态解码的剩余工作
- 精确还原所有 `api_3` 相关字符串需把 `rbx`（密文基址）→ `.rdata` 地址映射出来，逐段 XOR（或运行时 hook 该反混淆函数直接 dump 明文）。
- 当前用"丢弃密文噪音字节"的启发式已能可靠还原上述 Emby 头；非头部字符串（如具体路径模板）仍需动态提取。

## 4. 鉴权流（推断 + 本轮新增静态证据）

**登录端点（双端点，均确证）**：
- `http://117.72.12.20:9651/api/auth/login`（明文串 @0x0145ae9c："`invalid url: /api/auth/login..`" + "`device_id`/`deviceID`" + "`unsupported_auth_endpoint`"）。
- `http://117.72.12.20:9651/api/user/login`（XOR 混淆串 @0x0095b140：`/api/use[H3]er/login[H3J]` 还原为 `/api/user/login`）。
- 两者并存：推测 `api/auth/login` 为账号密码/设备登录主入口，`api/user/login` 为另一种登录形态（设备码/第三方）。具体差异待动态确认。

**Rust `api_3` 代理对登录的特殊处理（关键）**：
- 客户端并非直连 117.72.12.20；Rust 侧有一个 `api_3` 代理命令，转发到远端并做规则拦截。
- 命中代理错误串（@0x0145ae9c）：`unsupported http method` · `LumiPlayer-Tauri/1.0` · `system only http/https requests are allowed` · `direct-fallback` · `response too large` · `invalid url: /api/auth/login..` · `unsupported_auth_endpoint` · `invalid_account_endpoint` · `unsupported_method`。
- 即：代理**拦截/白名单**登录与账户类端点（`unsupported_auth_endpoint` / `invalid_account_endpoint`），可能存在"登录走专用通道、普通流量才经代理"的策略。重建时需区分"直连登录"与"代理转发"。

**登录请求参数**：`device_id` / `deviceID`（@0x0145af08 确证）—— 登录绑定设备标识（与 Emby `deviceid`、`api_3` 的 `device_id` 同源）。

**令牌与后续请求**：
- 登录返回 `access_token`（+ `UserId` / `device_id` 绑定），见 §3.2 头表。
- 后续请求在 `X-Emby-Token` 头携带 `access_token`，并附 `api_key` / `playsessionid` / `starttimeticks` / `deviceid` / `lumi_ret` 等。
- 另有 `authorToken` / `author_token` / `session_token` 标记（@0x014055a4 上下文）—— 存在 `access_token` 之外的"授权令牌/会话令牌"概念，具体角色待动态确认。
- 与本地 `__LUMI_SESSION_AUTH__` 会话键、`.enc` 凭据文件的关系：推测 `api_3` 登录态被本地缓存进 `lumi-store.db` 的 `kv` 或 `cloud-accounts.json`，由 Rust 层管理。

## 5. 与 StreamHub / Rust 的关系
```
                    LumiPlayer
          ┌────────────┼────────────┐
          │            │            │
       Rust         StreamHub    Lumi Cloud (api_3)
          │            │            │
   云盘/Emby直连   本地媒体中心   远程托管内容
   mpv/HDR/RIFE    HLS/AI/TMDB    /api/auth/login
                   :18400         :9651(+:9321)
```
- `api_3` 是**内容源**，不是播放引擎；实际解码仍由 Rust 的 libmpv 完成。
- `api_3` 与 StreamHub 不重叠：StreamHub 管"用户自有的本地/WebDAV 片库"，`api_3` 提供"运营方托管的流媒体 + 元数据"。
- Rust 层是这三者的统一编排者（resolveProvider 同时路由到 云盘 / Emby / api_3 / 本地）。

## 6. 待动态确认
1. `/api/auth/login` 与 `/api/user/login` 的请求/响应结构差异（用户名密码？设备码？第三方？）。
2. `access_token` 有效期与刷新机制（是否复用本地 refresh 逻辑）；`authorToken`/`session_token` 的角色。
3. ~~`:9321` 二级服务究竟是元数据还是转码/源站~~ —— **【已确认 = 元数据 blob，见 §1.1】**。
4. 反混淆函数全量 dump（确认有无其它隐藏路径/密钥）。
5. Rust `api_3` 代理对 `login`/`account` 端点的精确拦截策略（`unsupported_auth_endpoint` / `invalid_account_endpoint` 触发条件）。
