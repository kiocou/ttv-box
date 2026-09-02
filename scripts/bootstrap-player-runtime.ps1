param(
    [string]$DownloadDir = ".downloads"
)

$ErrorActionPreference = "Stop"
$downloads = [System.IO.Path]::GetFullPath($DownloadDir)
[System.IO.Directory]::CreateDirectory($downloads) | Out-Null

$artifacts = @(
    @{
        Name = "akarin-v0.96g3.7z"
        Url = "https://github.com/AkarinVS/vapoursynth-plugin/releases/download/v0.96/akarin-release-lexpr-amd64-v0.96g3.7z"
    },
    @{
        Name = "mvtools-v24-win64.7z"
        Url = "https://github.com/dubhatervapoursynth/vapoursynth-mvtools/releases/download/v24/vapoursynth-mvtools-v24-win64.7z"
    },
    @{
        Name = "fftw-3.3.10.zip"
        Url = "https://github.com/Vapoursynth-Plugins-Gitify/fftw3/releases/download/3.3.10/FFTW-3.3.10.zip"
    }
)

foreach ($artifact in $artifacts) {
    $destination = Join-Path $downloads $artifact.Name
    if (-not (Test-Path -LiteralPath $destination)) {
        Invoke-WebRequest -UseBasicParsing $artifact.Url -OutFile $destination
    }
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $destination).Hash.ToLowerInvariant()
    Write-Output "$($artifact.Name) $hash"
}

Write-Output "Large mpv, VapourSynth, vs-mlrt and model artifacts are pinned in docs/player-runtime.md and THIRD_PARTY_NOTICES.md."
