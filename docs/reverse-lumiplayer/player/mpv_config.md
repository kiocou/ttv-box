# 播放引擎：mpv 配置与渲染管线重建

来源：二进制内嵌 mpv 选项串（`LUMI_MPV_LOG` 上下文）+ libmpv FFI 符号 + shaders 目录。

## 1. mpv 选项集（从二进制恢复，选项名为权威，部分数值因打包合并需动态校准）

```
# 日志
log-file=<LUMI_MPV_LOG 路径>
terminal=yes
msg-level=all=v

# 输入
input-default-bindings=yes
input-vo-keyboard=yes
input-cursor=yes

# 窗口/生命周期（嵌入 WebView2 hwnd）
wid=<WebView2 子窗口 HWND>      # 嵌入而非独立窗口
force-window=yes
keep-open=yes
idle=yes
load-scripts=no

# 缓存（流媒体关键）
cache=yes
cache-on-disk=yes
autoload-files=no
stream-buffer-size=1MiB
hr-seek=yes
cache-secs=<N>
demuxer-readahead-secs=<N>
demuxer-max-bytes=<64MiB..>
demuxer-max-back-bytes=<N>
demuxer-lavf-probe-info=nostream
cache-pause-wait=0.35
cache-pause-initial=6

# 网络重连（云盘直链稳定性）
network-timeout=<N>
stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_delay_max=2

# 视频渲染
vo=gpu-next,gpu
gpu-api=d3d11
vd-lavc-dr=no
vd-lavc-threads=<CPU 线程数>

# 配置
config-dir=<LumiPlayer mpv 配置目录>
```

**重建提示**：`wid` 是嵌入关键 —— mpv 渲染到 WebView2 内的子窗口 HWND（非弹出）。
`gpu-next,gpu` 表示优先 gpu-next 渲染器，回退 gpu。`d3d11` 是 Windows 的 GPU API 选择。

## 2. libmpv FFI 调用序列（从二进制符号确认）
```
mpv_create
mpv_initialize
mpv_set_option / mpv_set_option_string   # 含 wid, vo, gpu-api 等
mpv_render_context_create
mpv_render_context_render
mpv_render_context_update
mpv_render_context_free
mpv_command                              # ["loadfile", url] / ["seek", ...]
mpv_get_property
mpv_wait_event
mpv_terminate_destroy
```
错误诊断字符串（二进制中存在）：`libmpv start superseded by a newer request`、`libmpv wid hwnd is zero`、`mpv_create returned null`。

## 3. 着色器管线（画质增强）
目录 `shaders/` 含 GLSL，重建需复刻调用顺序（动态确认优先级 ★★）：
- `ArtCNN.glsl` — 超分辨率/锐化
- `SSimDownscaler.glsl` — 高质量降采样
- `adaptive-sharpen.glsl` — 自适应锐化
典型链：`SSimDownscaler`(降采样) → `ArtCNN`(超分) → `adaptive-sharpen`(收尾)。
配置键含 `shaders` 项（见 `value_up_...shaders...` 配置键串）。

## 4. 外部播放器探测与外抛
Rust 侧枚举本机播放器，用户可选「用外部播放器打开」：
```
mpv.exe
DAUMPotPlayerMini64.exe / PotPlayerMini.exe / PotPlayerMini64.exe
MPC-HC / mpc-hc64.exe / mpc-hc.exe
VideoLAN (VLC)
MPC-BE x64 / mpc-be64.exe / MPC-BE
K-Lite Codec Pack (MPC-HC64 / MPC-HC (K-Lite))
```
环境变量探测：`LOCALAPPDATA`, `Programs`, `ProgramFiles`, `ProgramFiles(x86)`。

## 5. 重建任务清单
- [ ] 复刻 mpv.conf（校准缓存/重连数值，建议动态抓一次真实播放）
- [ ] 实现 HWND 嵌入到 WebView2 子窗口
- [ ] 确认着色器调用顺序（从 mpv 实际读取的 profile 或 shaders 目录加载逻辑）
- [ ] 实现外部播放器探测 + 外抛命令行构造
