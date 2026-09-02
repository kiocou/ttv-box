# 增强链冒烟测试：在正确设置的运行时环境下验证 RIFE / MVTools / UAI 三条 VapourSynth 管线。
#
# 之前的 smoke 记录失败是因为裸跑 mpv 时缺少 PYTHONHOME/PATH，且 Git Bash 会改写
# Windows 的 DLL 搜索路径导致 vsscript.dll 依赖解析失败。本脚本使用 PowerShell
# 原生环境并预先设置全部变量，与应用内 prepare_runtime_environment 的行为一致。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\enhancement-smoke.ps1
# 可选参数：
#   -Media <path>   测试视频（默认 .downloads\player-smoke.mp4，缺失时自动生成）
#   -Mode <rife|mvt|uai|all>   默认 all

param(
    [string]$Media = "",
    [ValidateSet("rife", "mvt", "uai", "all")]
    [string]$Mode = "all"
)

$ErrorActionPreference = "Stop"
$workspace = Split-Path -Parent $PSScriptRoot
$resources = Join-Path $workspace "src-tauri\resources"
$vs = Join-Path $resources "vapoursynth"
$mpv = Join-Path $resources "mpv\mpv.exe"

foreach ($required in @($mpv, (Join-Path $vs "libvapoursynth.dll"), (Join-Path $vs "vsscript.dll"))) {
    if (-not (Test-Path $required)) {
        Write-Host "[MISS] $required"
        Write-Host "先运行 scripts\bootstrap-player-runtime.ps1 下载运行时资源。"
        exit 1
    }
}

if (-not $Media) {
    $Media = Join-Path $workspace ".downloads\player-smoke.mp4"
    if (-not (Test-Path $Media)) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Media) | Out-Null
        & (Join-Path $resources "mpv\ffmpeg.exe") -y -hide_banner -loglevel error `
            -f lavfi -i "testsrc2=size=64x64:rate=24" -f lavfi -i "sine=frequency=440" `
            -t 2 -c:v libx264 -pix_fmt yuv420p $Media
        if ($LASTEXITCODE -ne 0) { throw "无法生成测试视频 $Media" }
        Write-Host "[GEN ] $Media"
    }
}

# 与 src-tauri/src/runtime/mod.rs 的 prepare_runtime_environment 保持一致。
$env:PYTHONHOME = Join-Path $vs "python"
$env:PYTHONPATH = "$vs\site-packages;$vs\python"
$env:PATH = "$vs;$vs\vsscript\python\bridge;$vs\python;" + $env:PATH

$scripts = @{
    rife = @{ file = "MEMC_RIFE_DML.vpy"; frames = 4 }
    mvt  = @{ file = "MEMC_MVT_LQ.vpy";   frames = 2 }
    uai  = @{ file = "MIX_UAI_DML.vpy";   frames = 2 }
}
if ($Mode -ne "all") { $scripts = @{ $Mode = $scripts[$Mode] } }

$logDir = Join-Path $workspace ".downloads"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

$failed = 0
foreach ($name in ($scripts.Keys | Sort-Object)) {
    $spec = $scripts[$name]
    $scriptPath = Join-Path $vs "scripts\$($spec.file)"
    if (-not (Test-Path $scriptPath)) {
        Write-Host "[FAIL] $name 缺少脚本 $($spec.file)"
        $failed++
        continue
    }
    $log = Join-Path $logDir "mpv-$name-smoke.ps.log"
    Write-Host "[RUN ] $name ($($spec.file)) ..."
    & $mpv --no-config --vo=null --ao=null --frames=$($spec.frames) `
        --log-file="$log" --msg-level=vapoursynth=v `
        --vf="vapoursynth=file=[$scriptPath]:buffered-frames=$($spec.frames):concurrent-frames=1" `
        "$Media" 2>$null | Out-Null

    $logText = if (Test-Path $log) { Get-Content $log -Raw } else { "" }
    $success = $logText -match "finished playback, success"
    $scriptError = $logText -match "Script evaluation failed|Failed to initialize VapourSynth"
    if ($success -and -not $scriptError) {
        Write-Host "[PASS] $name"
    } else {
        Write-Host "[FAIL] $name —— 详情见 $log"
        $logText -split "`n" | Select-String -Pattern "\[f\]|\[e\]" | Select-Object -First 5 | ForEach-Object { Write-Host "       $_" }
        $failed++
    }
}

if ($failed -gt 0) { exit 1 }
Write-Host "[OK  ] 全部增强管线冒烟通过。"
