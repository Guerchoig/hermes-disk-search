# publish_clip_models.ps1 - publish the local CLIP ONNX models as release assets (W4).
#
# One-off helper (release maintainer, needs the GitHub CLI `gh` and push rights):
# uploads models\clip_onnx\* (or tools\parity\out\clip_onnx\*) to the release tag
# referenced by runtime-manifests\clip-manifest.json. Run it BEFORE a release build
# so that installers\fetch_clip_models.ps1 can download them.
#
# The tag/release is created if missing. Existing assets with the same name must be
# deleted first (gh has no --clobber): use -Clobber to remove and re-upload them.
#
# ASCII-only on purpose (Windows PowerShell 5.1 + BOM-less .ps1).
#
# Example:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\publish_clip_models.ps1
param(
    [string]$Manifest = "",
    [string]$SourceDir = "",
    [string]$Repository = "Guerchoig/hermes-disk-search",
    [switch]$Clobber
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $Manifest) { $Manifest = Join-Path $root "runtime-manifests\clip-manifest.json" }
if (-not (Test-Path $Manifest)) { throw "clip manifest not found: $Manifest" }
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw "GitHub CLI 'gh' not found in PATH" }

if (-not $SourceDir) {
    $installed = Join-Path $root "models\clip_onnx"
    $dev = Join-Path $root "tools\parity\out\clip_onnx"
    if (Test-Path (Join-Path $installed "vision\clip_vision.onnx")) { $SourceDir = $installed }
    elseif (Test-Path (Join-Path $dev "vision\clip_vision.onnx")) { $SourceDir = $dev }
    else { throw "CLIP models not found (looked in $installed and $dev)" }
}

$mf = Get-Content $Manifest -Raw -Encoding UTF8 | ConvertFrom-Json
$tag = $mf.tag
Write-Host "[..] publishing CLIP models from $SourceDir to $Repository@$tag"

$files = @()
foreach ($a in @($mf.assets)) {
    $src = Join-Path $SourceDir ($a.target -replace '/', '\')
    if (-not (Test-Path $src)) { throw "missing source file: $src" }
    $got = (Get-FileHash $src -Algorithm SHA256).Hash.ToLower()
    if ($got -ne ([string]$a.sha256).Trim().ToLower()) {
        throw "sha256 mismatch for $($a.file_name): manifest $($a.sha256), file $got - update the manifest first"
    }
    $files += $src
}

# gh writes its "release not found" to stderr; with $ErrorActionPreference = "Stop"
# Windows PowerShell 5.1 turns that into a NativeCommandError and aborts, so run gh
# with stderr merged into stdout (EAP "Continue") and decide by $LASTEXITCODE.
$ErrorActionPreference = "Continue"
$null = & gh release view $tag --repo $Repository 2>&1
$ErrorActionPreference = "Stop"
if ($LASTEXITCODE -ne 0) {
    Write-Host "[..] creating release $tag"
    $ErrorActionPreference = "Continue"
    $null = & gh release create $tag --repo $Repository --title "$tag (CLIP ONNX models)" --notes "CLIP ONNX models for hermes-disk-search (see runtime-manifests/clip-manifest.json)." 2>&1
    $ErrorActionPreference = "Stop"
    if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }
}
if ($Clobber) {
    foreach ($f in $files) {
        $ErrorActionPreference = "Continue"
        $null = & gh release delete-asset $tag (Split-Path $f -Leaf) --repo $Repository --yes 2>&1
        $ErrorActionPreference = "Stop"
    }
}
$ErrorActionPreference = "Continue"
$null = & gh release upload $tag @files --repo $Repository 2>&1
$ErrorActionPreference = "Stop"
if ($LASTEXITCODE -ne 0) { throw "gh release upload failed (use -Clobber to replace existing assets)" }
Write-Host "[ok] CLIP models published: $tag"
