# fetch_diarization_model.ps1 - download the Sortformer diarization model (T5).
#
# The engine runtime already contains the native diarization backend (bridge-audio);
# the model itself is a GGUF placed under the engine's shared models directory, so
# both the engine UI and llm-host reuse the same file. hds-llama looks for it in
# %APPDATA%\OpenResearchTools\models\*sortformer*\*.gguf (see host::default_diarization_model)
# and the engine refuses a `mode: transcript` job without it (hard failure, no fallback).
#
# URL/size are taken from the reference app (transcribeoffline/src/main.rs constants
# DIARIZATION_MODEL_URL / DIARIZATION_MODEL_SIZE_BYTES). Hugging Face publishes no
# sha256 here, so only the size is checked. The file is optional: without it the
# auto-transcription pipeline reports a clear error and keeps the source file.
# ASCII-only on purpose: PowerShell 5.1 reads a BOM-less .ps1 as ANSI.
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_diarization_model.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_diarization_model.ps1 -Force
param(
    [string]$ModelsDir = "",
    [string]$File = "diar_streaming_sortformer_4spk-v2.1.gguf",
    [string]$Repo = "openresearchtools/diar_streaming_sortformer_4spk-v2.1-gguf",
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
$dir = Join-Path $ModelsDir "openresearchtools__diar_streaming_sortformer_4spk-v2.1-gguf"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$dest = Join-Path $dir $File
$exactBytes = [int64]471107712            # reference DIARIZATION_MODEL_SIZE_BYTES
$minBytes = [int64]300 * 1MB              # sanity threshold (about 449 MB in total)

if ((-not $Force) -and (Test-Path $dest) -and ((Get-Item $dest).Length -gt $minBytes)) {
    Write-Host "[ok] diarization model already present: $dest"
    exit 0
}

$url = "https://huggingface.co/$Repo/resolve/main/$File?download=true"
Write-Host "[..] downloading diarization model ($File, about 449 MB)..."
if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
    & curl.exe -L --fail --progress-bar -o "$dest.part" $url
} else {
    $ProgressPreference = "SilentlyContinue"
    Invoke-WebRequest -Uri $url -OutFile "$dest.part" -UseBasicParsing -TimeoutSec 3600
}
if ((Test-Path "$dest.part") -and ((Get-Item "$dest.part").Length -gt $minBytes)) {
    Move-Item "$dest.part" $dest -Force
    $size = (Get-Item $dest).Length
    Write-Host "[ok] diarization model: $dest ($size bytes)"
    if ($size -ne $exactBytes) {
        Write-Host "[--] note: size differs from the reference ($exactBytes bytes) - the file is still usable" -ForegroundColor Yellow
    }
    exit 0
}
Remove-Item "$dest.part" -Force -ErrorAction SilentlyContinue
Write-Host "[!!] diarization model download failed - get the file manually:" -ForegroundColor Yellow
Write-Host "     $url  ->  $dest"
Write-Host "     without this model auto-transcription jobs fail (diarization is required)."
exit 1
