# fetch_clip_models.ps1 - download the CLIP ONNX models (W4).
#
# CLIP search of images by content needs three files exported from the HF models
# (see tools/parity/clip_onnx_w3.py): a vision encoder, a text encoder with
# pooling+Dense and the multilingual tokenizer. They are NOT committed to git
# (about 850 MB); instead they are published as release assets and listed in
# runtime-manifests\clip-manifest.json with sha256. This script verifies the hash
# and unpacks them into models\clip_onnx (the default searched by hds-clip).
#
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_clip_models.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_clip_models.ps1 -Force
param(
    [string]$Manifest = "",
    [string]$ModelsDir = "",
    [switch]$Force
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $Manifest) { $Manifest = Join-Path $root "runtime-manifests\clip-manifest.json" }
if (-not (Test-Path $Manifest)) { throw "clip manifest not found: $Manifest" }
if (-not $ModelsDir) { $ModelsDir = Join-Path $root "models\clip_onnx" }

$mf = Get-Content $Manifest -Raw -Encoding UTF8 | ConvertFrom-Json
Write-Host "[..] CLIP models: tag $($mf.tag) -> $ModelsDir"

foreach ($a in @($mf.assets)) {
    $dest = Join-Path $ModelsDir ($a.target -replace '/', '\')
    $dir = Split-Path $dest -Parent
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    if (-not $Force -and (Test-Path $dest)) {
        $got = (Get-FileHash $dest -Algorithm SHA256).Hash.ToLower()
        if ($got -eq ([string]$a.sha256).Trim().ToLower()) {
            Write-Host "[ok] $($a.id): $($a.file_name) already present"
            continue
        }
    }
    Write-Host "[..] $($a.id): downloading $($a.file_name) ..."
    $tmp = "$dest.part"
    $ProgressPreference = "SilentlyContinue"
    try {
        Invoke-WebRequest -Uri $a.url -OutFile $tmp -UseBasicParsing -TimeoutSec 3600
    } catch {
        if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
            & curl.exe -L --fail --progress-bar -o $tmp $a.url
            if ($LASTEXITCODE -ne 0) {
                Remove-Item $tmp -Force -ErrorAction SilentlyContinue
                throw "curl failed: $($a.file_name)"
            }
        } else {
            Remove-Item $tmp -Force -ErrorAction SilentlyContinue
            throw
        }
    }
    $got = (Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower()
    $want = ([string]$a.sha256).Trim().ToLower()
    if ($got -ne $want) {
        Remove-Item $tmp -Force -ErrorAction SilentlyContinue
        throw "sha256 mismatch for $($a.file_name): expected $want, got $got"
    }
    Move-Item $tmp $dest -Force
    Write-Host "[ok] $($a.id): $dest"
}
Write-Host "[ok] CLIP models ready: $ModelsDir"
