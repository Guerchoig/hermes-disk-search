# fetch_whisper_model.ps1 - download the whisper GGML model used by ASR (W4).
#
# The engine runtime already contains the ASR bridge (llama-server-audio.dll); the
# model itself is a whisper.cpp GGML .bin (NOT a llama.cpp GGUF). It is placed under
# the engine's shared models directory so that both the engine UI and llm-host reuse
# the same file. hds-llama::transcribe resolves it from %APPDATA%\OpenResearchTools\
# models\*whisper* (see crates/hds-llama/src/bin/audio_probe.rs).
#
# Hugging Face does not publish a sha256 for this file, so the installer only checks
# the size; the file is optional (the model is downloaded on first transcription).
# ASCII-only on purpose: PowerShell 5.1 reads a BOM-less .ps1 as ANSI.
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_whisper_model.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_whisper_model.ps1 -Force
param(
    [string]$ModelsDir = "",
    [string]$File = "whisper-large-v3-turbo-GGML.bin",
    [string]$Repo = "openresearchtools/whisper-large-v3-turbo-GGML",
    [switch]$Force
)
$ErrorActionPreference = "Continue"

if (-not $ModelsDir) {
    if ($env:OS -eq "Windows_NT") {
        $ModelsDir = Join-Path $env:APPDATA "OpenResearchTools\models"
    } else {
        $ModelsDir = Join-Path $HOME "Library/Application Support/OpenResearchTools/models"
    }
}
$dir = Join-Path $ModelsDir "openresearchtools__whisper-large-v3-turbo-GGML"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$dest = Join-Path $dir $File
$minBytes = [int64]1000 * 1MB   # the turbo GGML model is about 1.5 GB

if ((-not $Force) -and (Test-Path $dest) -and ((Get-Item $dest).Length -gt $minBytes)) {
    Write-Host "[ok] whisper model already present: $dest"
    exit 0
}

$url = "https://huggingface.co/$Repo/resolve/main/$File?download=true"
Write-Host "[..] downloading whisper model ($File, about 1.5 GB)..."
if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
    & curl.exe -L --fail --progress-bar -o "$dest.part" $url
} else {
    $ProgressPreference = "SilentlyContinue"
    Invoke-WebRequest -Uri $url -OutFile "$dest.part" -UseBasicParsing -TimeoutSec 3600
}
if ((Test-Path "$dest.part") -and ((Get-Item "$dest.part").Length -gt $minBytes)) {
    Move-Item "$dest.part" $dest -Force
    Write-Host "[ok] whisper model: $dest"
} else {
    Remove-Item "$dest.part" -Force -ErrorAction SilentlyContinue
    Write-Host "[!!] whisper model download failed - get the file manually:" -ForegroundColor Yellow
    Write-Host "     $url  ->  $dest"
}
