# publish_engine_patch.ps1 - publish the HDS engine patch DLLs as release assets.
#
# Maintainer helper (needs the GitHub CLI `gh` and push rights). Takes the DLL set
# produced by the patched engine build (bridge/README.md "patchable layer"; see
# engine-patch/README.md for the full build recipe), verifies sha256 against
# runtime-manifests/engine-patch.json and uploads the files to the release tag from
# that manifest (created if missing).
#
# Run it BEFORE a release so that installers/fetch_engine_runtime.ps1 -PatchEngine can
# download the patch. Existing assets with the same name must be removed first
# (gh has no --clobber): use -Clobber.
#
# ASCII-only on purpose (Windows PowerShell 5.1 + BOM-less .ps1).
#
# Examples:
#   # 1) copy the build output into dist\engine-patch (or pass -SourceDir)
#   # 2) publish
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\publish_engine_patch.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\publish_engine_patch.ps1 -SourceDir C:\path\to\bin\Release -Clobber
param(
    [string]$Manifest = "",
    [string]$SourceDir = "",
    [string]$Repository = "Guerchoig/hermes-disk-search",
    [switch]$Clobber
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $Manifest) { $Manifest = Join-Path $root "runtime-manifests\engine-patch.json" }
if (-not (Test-Path $Manifest)) { throw "engine patch manifest not found: $Manifest" }
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw "GitHub CLI 'gh' not found in PATH" }

if (-not $SourceDir) {
    if ($env:HDS_ENGINE_PATCH_SRC) {
        $SourceDir = $env:HDS_ENGINE_PATCH_SRC
    } elseif (Test-Path (Join-Path $root "dist\engine-patch\multi-node-server.dll")) {
        $SourceDir = Join-Path $root "dist\engine-patch"
    } else {
        throw "engine patch sources not found: pass -SourceDir or set HDS_ENGINE_PATCH_SRC (copy the build output: bin\Release of the patched v1.15 tree)"
    }
}

$mf = Get-Content $Manifest -Raw -Encoding UTF8 | ConvertFrom-Json
$tag = $mf.tag
Write-Host "[..] publishing HDS engine patch from $SourceDir to $Repository@$tag (base $($mf.base_tag))"

$files = @()
foreach ($f in @($mf.files)) {
    $src = Join-Path $SourceDir $f.file_name
    if (-not (Test-Path $src)) { throw "missing source file: $src" }
    $got = (Get-FileHash $src -Algorithm SHA256).Hash.ToLower()
    if ($got -ne ([string]$f.sha256).Trim().ToLower()) {
        throw "sha256 mismatch for $($f.file_name): manifest $($f.sha256), file $got - update the manifest first"
    }
    $files += $src
}

# Native gh writes expected diagnostics to stderr; with ErrorActionPreference=Stop
# Windows PowerShell 5.1 raises NativeCommandError before reading $LASTEXITCODE.
# Use Continue in the gh section and rely on the explicit $LASTEXITCODE checks below.
$ErrorActionPreference = "Continue"
& gh release view $tag --repo $Repository *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host "[..] creating release $tag"
    & gh release create $tag --repo $Repository --title "$tag (HDS engine patch)" --notes "HDS patch for Openresearchtools-Engine $($mf.base_tag): bounded slot wait, model load outside the instance lock, lock order, KV cache type + flash attention. See engine-patch/README.md."
    if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }
}
if ($Clobber) {
    foreach ($f in $files) {
        & gh release delete-asset $tag (Split-Path $f -Leaf) --repo $Repository --yes *> $null
    }
}
& gh release upload $tag @files --repo $Repository
if ($LASTEXITCODE -ne 0) { throw "gh release upload failed (use -Clobber to replace existing assets)" }
Write-Host "[ok] HDS engine patch published: $tag"
