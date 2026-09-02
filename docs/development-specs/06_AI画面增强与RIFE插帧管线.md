# TTV AI 画面增强与 RIFE 插帧管线规范

> **设计目标**：将 462MB 的 VapourSynth + DirectML + ONNX Runtime + RIFE 运行时打包内置，实现开箱即用的 60/120 FPS 光流补帧与 ArtCNN 神经网络超分。

---

## 1. 运行时物理资源布局 (`resources/vapoursynth/`)

```text
resources/vapoursynth/
├── python312.dll / python.exe        # 内嵌绿色 Python 3.12 运行时
├── python312.zip                     # 标准库字节码包
├── libvapoursynth.dll                # VapourSynth 核心 C 库
├── vsscript.dll / vspipe.exe         # 脚本执行与管道接口
├── vs-plugins/                       # 核心算子扩展
│   ├── mvtools.dll                   # 经典运动向量工具库
│   ├── libakarin.dll                 # 高性能表达式算子
│   ├── vsort.dll                     # ONNX Runtime VapourSynth 桥接插件
│   ├── DirectML.dll                  # 微软 DirectML GPU 硬件加速库 (支持 AMD/Intel/NVIDIA)
│   └── onnxruntime.dll               # ONNX 推理引擎
├── models/rife_v2/
│   └── rife_v4.25_lite.onnx          # 针对实时视频优化的 RIFE v4.25 补帧模型
└── scripts/
    ├── MEMC_RIFE_DML.vpy             # RIFE 光流插帧主执行脚本
    └── MEMC_MVT_LQ.vpy               # 低功耗 MVTools 补帧兜底脚本
```

---

## 2. 补帧执行链路与 VapourSynth 调度

### 2.1 mpv 滤镜链挂载
在播放器初始化或用户点击开启 RIFE 时，Rust 向 `libmpv` 下发：
```text
vf set @ttv-interp:vapoursynth="<APP_DIR>/resources/vapoursynth/scripts/MEMC_RIFE_DML.vpy":buffered-frames=4:concurrent-frames=2
```

### 2.2 `MEMC_RIFE_DML.vpy` 核心执行逻辑
```python
import vapoursynth as vs
from vapoursynth import core
import os

clip = video_in
# 仅对低于 45 FPS 的片源执行插帧 (避免 60fps 原始片源重复计算)
if clip.fps_num / clip.fps_den < 45.0:
    # 转换为 RGB 通道送入 ONNX 推理
    clip_rgb = core.resize.Bicubic(clip, format=vs.RGBS, matrix_in_s="709")
    # 调用 vsort + DirectML 运行 RIFE v4.25
    interpolated = core.vsort.Model(
        clip_rgb,
        model_path=os.path.join(os.path.dirname(__file__), "../models/rife_v2/rife_v4.25_lite.onnx"),
        engine=core.vsort.DirectML,
        device_id=0,
        factor=2  # 2倍帧率平滑插帧 (24fps -> 48fps / 30fps -> 60fps)
    )
    video_out = core.resize.Bicubic(interpolated, format=clip.format, matrix_s="709")
else:
    video_out = clip

video_out.set_output()
```

---

## 3. GLSL 着色器超分管线 (零 Python 损耗)

对于 1080P 及以下分辨率视频，通过 Direct3D 11 GPU Shader 链实现毫秒级超分与降噪：

```
[原始画面] ──▶ [SSimDownscaler.glsl] ──▶ [ArtCNN.glsl] ──▶ [adaptive-sharpen.glsl] ──▶ [最终输出]
               高质量抗锯齿降采样        神经网络纹理重构         自适应边缘锐化
```

### mpv 着色器下发命令
```text
change-list glsl-shaders set "<APP_DIR>/resources/shaders/SSimDownscaler.glsl;<APP_DIR>/resources/shaders/ArtCNN.glsl;<APP_DIR>/resources/shaders/adaptive-sharpen.glsl"
```

---

## 4. 四档画质策略与性能熔断机制

| 档位 | 模式 | 组合管线 | 目标场景与 GPU 负载 |
|---|---|---|---|
| **0** | 原画直通 | 无滤镜，纯 D3D11VA 硬解 | 低配核显、省电模式 (GPU < 5%) |
| **1** | 高清重构 | ArtCNN 超分 + 自适应锐化 | 1080P 片源、核显/轻薄本 (GPU 10-20%) |
| **2** | 极速流畅 | RIFE v4.25 光流插帧至 60FPS | 24FPS 动漫与电影、主流独显 (GPU 25-45%) |
| **3** | 极致影院 | RIFE 60FPS 插帧 + 4K 超分着色器 | 4K 巨幕影院体验、中高端独显 (GPU > 50%) |

### 降级与熔断保护策略
1. **冷启动隔离**：应用启动时不加载 ONNX 模型，确保主界面 3 秒内完全可交互。
2. **显存/内存不足自动回退**：若 DirectML 初始化返回 `E_OUTOFMEMORY` 或丢帧率连续 5 秒超过 15%，自动静默降级至档位 1 并提示用户，绝不发生闪退。
