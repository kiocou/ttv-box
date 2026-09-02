# Player Runtime Integration

## Runtime layout

- `src-tauri/resources/mpv`: libmpv, mpv, FFmpeg and the VC++ runtime.
- `src-tauri/resources/vapoursynth`: portable Python/VSScript environment.
- `src-tauri/resources/vapoursynth/vs-plugins`: VSORT DirectML, Akarin,
  MVTools and enhancement models.
- `src-tauri/resources/shaders`: ArtCNN, SSimDownscaler and adaptive sharpen.

Tauri maps the content of `resources/` into the packaged resource root.
Startup discovers both the packaged layout and the development workspace,
preloads the bundled VC++ runtime, then configures `VSSCRIPT_PATH`,
`PYTHONHOME`, `PYTHONPATH`, plugin paths and native DLL search paths.
The default libmpv backend is headless (`force-window=no`, `vo=null`) because
the current Tauri shell does not yet provide a native video surface. This
prevents an extra standalone `No file - mpv` window; an embedded GPU profile
can be enabled once a `window_id`/render target is supplied.

## Enhancement modes

| Mode | Pipeline |
| --- | --- |
| 0 | Original rendering |
| 1 | GLSL upscaling/sharpening |
| 2 | RIFE DirectML, MVTools fallback |
| 3 | GLSL plus RIFE/MVTools |
| 4 | UAI DirectML upscaling, GLSL fallback |
| 5 | RIFE/MVTools plus UAI DirectML |

The frontend uses `enhancement_set` with `rife`, `glsl`, `vapoursynth`, or
`uai`. Filter paths use mpv's bracket escaping so Windows drive letters and
spaces are safe.

## Verified on August 19, 2026

- Bundled libmpv loaded and initialized with the production option set.
- RIFE 4.25 Lite DirectML produced frames through VapourSynth.
- MVTools produced frames and works as the interpolation fallback.
- UAI DirectML doubled a 64x64 smoke video to 128x128.
- mpv decoded a real H.264 sample and completed playback through all three
  VapourSynth scripts.
