# fetch_nemotron3_model.ps1 - download the Nemotron-3 diarization model (W6).
#
# The model is a third-party ONNX export (NVIDIA Nemotron-3-Diarization, a
# Sortformer v3 checkpoint, up to 8 speakers). hds-diarize / llm_host run it through
# ONNX Runtime when `diarization.backend: nemotron3` (the product default).
#
# The file is NOT part of the repository and is NOT shipped in the release archive:
# this script downloads it into <ModelsDir>\nemotron3\ and verifies size + sha256.
#
# Source (pinned, do not use "main"):
#   repo     altunenes/parakeet-rs (Hugging Face)
#   revision 4d2a8bc71f5c896ec40faa59732e6716295edaf2
#   file     nemotron-3-diarization/nemotron3_diar_v3.onnx
#   size     400506656 bytes
#   sha256   915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a
#   license  OpenMDW 1.1 (see NOTICE.md)
#
# Model directories are searched in this order (hds_diarize::models_roots, W5):
#   HDS_MODELS_DIR -> %ProgramData%\hermes-disk-search\models ->
#   %APPDATA%\OpenResearchTools\models   (the file lives in <root>\nemotron3\)
# Without -ModelsDir the script uses HDS_MODELS_DIR, then the machine directory when
# it is writable (elevated run), then the per-user engine models directory.
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_nemotron3_model.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_nemotron3_model.ps1 -Force
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_nemotron3_model.ps1 -ModelsDir D:\models
#
# Idempotent: an existing file with the expected size and sha256 is skipped (exit 0);
# -Force downloads it again. ASCII-only on purpose: PowerShell 5.1 reads a BOM-less
# .ps1 as ANSI.
param(
    [string]$ModelsDir = "",
    [string]$Repo = "altunenes/parakeet-rs",
    [string]$Revision = "4d2a8bc71f5c896ec40faa59732e6716295edaf2",
    [string]$File = "nemotron-3-diarization/nemotron3_diar_v3.onnx",
    [switch]$Force
)
$ErrorActionPreference = "Continue"

$exactBytes = [int64]400506656
$sha256 = "915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a"

function Test-WritableDir([string]$path) {
    try {
        New-Item -ItemType Directory -Force -Path $path -ErrorAction Stop | Out-Null
        $probe = Join-Path $path (".hds-write-{0}.tmp" -f $PID)
        Set-Content -Path $probe -Value "x" -ErrorAction Stop
        Remove-Item $probe -Force -ErrorAction SilentlyContinue
        return $true
    } catch {
        return $false
    }
}

if (-not $ModelsDir) {
    if ($env:HDS_MODELS_DIR) {
        # Server deployments pin the machine-wide models dir; roles run as
        # LocalSystem and do not see the administrator profile.
        $ModelsDir = $env:HDS_MODELS_DIR
    } elseif ($env:OS -eq "Windows_NT") {
        $machine = Join-Path $env:ProgramData "hermes-disk-search\models"
        if (Test-WritableDir $machine) {
            $ModelsDir = $machine
        } else {
            $ModelsDir = Join-Path $env:APPDATA "OpenResearchTools\models"
        }
    } else {
        $ModelsDir = Join-Path $HOME "Library/Application Support/OpenResearchTools/models"
    }
}

$dir = Join-Path $ModelsDir "nemotron3"
if (-not (Test-WritableDir $dir)) {
    Write-Host "[!!] cannot create model directory: $dir" -ForegroundColor Yellow
    Write-Host "     run elevated or pass -ModelsDir <writable folder>."
    exit 1
}
$dest = Join-Path $dir (Split-Path -Leaf $File)
$url = "https://huggingface.co/$Repo/resolve/$Revision/${File}?download=true"

if (-not $Force -and (Test-Path $dest)) {
    $size = (Get-Item $dest).Length
    if ($size -eq $exactBytes) {
        $got = (Get-FileHash -Algorithm SHA256 -Path $dest).Hash.ToLowerInvariant()
        if ($got -eq $sha256) {
            Write-Host "[ok] Nemotron-3 diarization model already present: $dest"
            exit 0
        }
        Write-Host "[..] existing file has a wrong sha256 ($got) - downloading again"
    } else {
        Write-Host "[..] existing file has a wrong size ($size) - downloading again"
    }
}

$part = "$dest.part"
Remove-Item $part -Force -ErrorAction SilentlyContinue
$mb = [math]::Round($exactBytes / 1MB)
Write-Host "[..] downloading Nemotron-3 diarization model (about $mb MB)..."
$downloaded = $false
if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
    & curl.exe -L --fail --retry 3 --progress-bar -o "$part" $url
    if ($LASTEXITCODE -eq 0) { $downloaded = $true }
} else {
    $ProgressPreference = "SilentlyContinue"
    try {
        Invoke-WebRequest -Uri $url -OutFile $part -UseBasicParsing -TimeoutSec 3600
        $downloaded = $true
    } catch {
        Write-Host "[!!] download error: $($_.Exception.Message)" -ForegroundColor Yellow
    }
}

if ($downloaded -and (Test-Path $part)) {
    $size = (Get-Item $part).Length
    if ($size -eq $exactBytes) {
        $got = (Get-FileHash -Algorithm SHA256 -Path $part).Hash.ToLowerInvariant()
        if ($got -eq $sha256) {
            Move-Item -Path $part -Destination $dest -Force
            Write-Host "[ok] Nemotron-3 diarization model: $dest ($exactBytes bytes, sha256 verified)"
            exit 0
        }
        Write-Host "[!!] sha256 mismatch: expected $sha256, got $got" -ForegroundColor Yellow
    } else {
        Write-Host "[!!] wrong size: expected $exactBytes bytes, got $size" -ForegroundColor Yellow
    }
}

Remove-Item $part -Force -ErrorAction SilentlyContinue
Write-Host "[!!] Nemotron-3 diarization model download failed - get the file manually:" -ForegroundColor Yellow
Write-Host "     $url"
Write-Host "     -> $dest"
Write-Host "     expected size $exactBytes bytes, sha256 $sha256"
Write-Host "     without it jobs with diarization.backend: nemotron3 fail; use 'sortformer' as a fallback."
exit 1
