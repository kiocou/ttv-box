# TTV 播放核心与 libmpv 渲染规范

> **规范目标**：基于动态链接 `libmpv-2.dll` 与 Direct3D 11 构建极致平滑、HDR 硬件直通的独立影院播放核心。

---

## 1. 播放器底层选型与架构

| 维度 | 规范参数 | 依据与技术说明 |
|---|---|---|
| **核心库** | `libmpv-2.dll` (C ABI) | 动态链接，FFI 绑定，零子进程开销 |
| **底层解码** | FFmpeg n8.1 (BtbN Shared) | 硬件硬解 (D3D11VA / DXVA2 / NVDEC) |
| **渲染后端** | `vo=gpu-next,gpu` | 优先 Vulkan/D3D11 新一代 gpu-next 渲染器 |
| **图形 API** | `gpu-api=d3d11` | Windows 11 原生 Direct3D 11 管线 |
| **窗口嵌入** | `wid=<HWND>` | 注入独立播放窗口 HWND，实现原生硬件层画面贴合 |

---

## 2. mpv.conf 权威渲染参数规范

```ini
# ========== 日志与核心生命周期 ==========
terminal=no
msg-level=all=warn
keep-open=yes
idle=yes
load-scripts=no
force-window=yes

# ========== 窗口与 HWND 嵌入 ==========
wid=0                           # 由 Rust 在运行时动态传入独立播放窗口 HWND
input-default-bindings=yes
input-vo-keyboard=yes
input-cursor=yes

# ========== 视频渲染与 D3D11 ==========
vo=gpu-next,gpu
gpu-api=d3d11
d3d11-output-format=rgba16hf     # 支持 10-bit / 16-bit 广色域与 HDR 渲染
fbo-format=rgba16hf
vd-lavc-dr=no
vd-lavc-threads=0                # 自动分配 CPU 解码线程

# ========== HDR 与色调映射 (Tone-Mapping) ==========
target-colorspace-hint=yes
target-colorspace-hint-mode=perceptual
tone-mapping=perceptual
hdr-compute-peak=yes
hdr-contrast-recovery=0.30
hdr-contrast-smoothness=3.5
dither-depth=auto
dither=fruit

# ========== 网络流媒体缓存与抗弱网重连 (云盘直链必备) ==========
cache=yes
cache-on-disk=yes
autoload-files=no
stream-buffer-size=1MiB
hr-seek=yes
cache-secs=30
demuxer-readahead-secs=20
demuxer-max-bytes=128MiB
demuxer-max-back-bytes=64MiB
demuxer-lavf-probe-info=nostream
cache-pause-wait=0.35
cache-pause-initial=6
network-timeout=15
stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_delay_max=2
```

---

## 3. Rust `MpvActor` 线程模型与 FFI 调用序列

由于 libmpv 的 C 接口存在阻塞调用且非全部线程安全，Rust 侧采用 **Actor 模型** 维护专属播放线程：

```
[Tauri Command (Tokio Async)]
              │
              │ MPSC Channel (Command Envelope)
              ▼
[MPV Dedicated OS Thread]
  ├─ mpv_create()
  ├─ mpv_initialize()
  ├─ mpv_set_option_string("wid", hwnd)
  ├─ mpv_set_option_string("vo", "gpu-next")
  ├─ Loop: mpv_wait_event()
  │     ├─ MPV_EVENT_PROPERTY_CHANGE (time-pos, duration, pause)
  │     ├─ MPV_EVENT_FILE_LOADED
  │     └─ MPV_EVENT_END_FILE
  └─ mpv_terminate_destroy()
```

### 核心 FFI 交互命令表
- `loadfile <url> [replace|append]`：加载本地文件路径或云盘 HTTPS 直链
- `seek <seconds> absolute|relative`：高精度关键帧/时间点跳转
- `set pause yes|no`：播放/暂停控制
- `set volume <0-100>`：硬件级音量调节
- `set speed <0.25-4.0>`：平滑倍速播放（无变调处理）
- `set aid <id>` / `set sid <id>`：多音轨与多字幕轨道实时切换
- `change-list glsl-shaders set <shaders>`：动态挂载 ArtCNN / SSimDownscaler GLSL 着色器

---

## 4. 外部播放器探测与外抛机制

支持用户在遇到极端罕见编码时，一键唤起本机已安装的外部专业播放器：
1. **探测注册表与标准路径**：
   - PotPlayer：`DAUMPotPlayerMini64.exe`、`PotPlayer64.exe`
   - MPC-HC / MPC-BE：`mpc-hc64.exe`、`mpc-be64.exe`
   - VLC：`vlc.exe`
2. **命令行外抛协议**：
   - 携带已解析的直链 URL、Referer 与 User-Agent 启动子进程，实现无缝交接。
