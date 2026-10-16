# Build the THIN CLIENT distribution (M9 / Э9 / W7).
#
# The client package contains ONLY what is needed to point Cline at a REMOTE
# disk-search server: bin\hds.exe + the Cline rule/skill sources + the client
# masters. NO engine / models / index / sidecar (that is the SERVER package built
# by installers\build_rust_release.ps1).
#
# Usage (from the repo root):
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_client_release.ps1 -Version 0.18.0
#   ... -SkipBuild   # reuse target\release\hds.exe
#
# Result:
#   dist\hds-client-<Version>-windows-x64\        thin client layout (bin, installers, ...)
#   dist\hds-client-<Version>-windows-x64.zip     the same tree as a zip (contents at the root)
#   dist\hds-client-<Version>-windows-x64.zip.sha256.txt
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [string]$Version = "0.1.0",
    [switch]$SkipBuild,
    [switch]$SkipZip
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $SkipBuild) {
    Write-Host "[m9] building release hds.exe (client only)..."
    # -p hds-cli builds hds.exe; the resident (llm_host.exe) is NOT touched.
    cargo build --release -p hds-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} else {
    Write-Host "[m9] -SkipBuild: reusing target\release\hds.exe"
}

$stage = Join-Path $root "dist\hds-client-$Version-windows-x64"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
$binDir = Join-Path $stage "bin"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

# the ONLY binary of a thin client
$src = Join-Path $root "target\release\hds.exe"
if (-not (Test-Path $src)) { throw "missing $src (build the release first)" }
Copy-Item $src (Join-Path $binDir "hds.exe") -Force

# client masters (Windows) at the archive root
foreach ($f in @("setup-client.ps1", "setup-client.cmd")) {
    $p = Join-Path $root $f
    if (Test-Path $p) { Copy-Item $p (Join-Path $stage $f) -Force }
}

# Cline rule/skill sources (the client master copies them into ~/.cline)
foreach ($d in @("cline-rules", "hermes-skill")) {
    $srcD = Join-Path $root $d
    if (-not (Test-Path $srcD)) { continue }
    $dstD = Join-Path $stage $d
    New-Item -ItemType Directory -Force -Path $dstD | Out-Null
    Copy-Item (Join-Path $srcD "*") $dstD -Recurse -Force
}

# the user memo becomes the client README (installers\client_readme.md)
$readme = Join-Path $root "installers\client_readme.md"
if (Test-Path $readme) { Copy-Item $readme (Join-Path $stage "README.md") -Force }

# Версия продукта рядом с клиентом (M9/W7): мастер сверяет её с `version` из
# /health сервера и предупреждает, если клиент отстал (hds_core::client).
Set-Content -Encoding ascii (Join-Path $stage "version.txt") -Value $Version

# --- guard: a thin client must NOT carry heavy/server parts ------------------
$forbidden = @("hds_mcp.exe", "llm_host.exe", "hds_mcp", "llm_host",
               "sidecar", "runtime-manifests", "assets", "install_server.ps1",
               "install_server.command", "update.ps1")
$bad = Get-ChildItem -Recurse $stage -ErrorAction SilentlyContinue |
    Where-Object { $forbidden -contains $_.Name }
if ($bad) {
    throw ("thin client package must not contain heavy/server parts: " +
           (($bad | ForEach-Object { $_.Name }) -join ", "))
}

# sha256 manifest for bin/*
$lines = @()
foreach ($f in Get-ChildItem -File $binDir) {
    $h = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
    $lines += ("{0}  bin/{1}" -f $h, $f.Name)
}
$lines | Set-Content -Encoding ascii (Join-Path $stage "sha256.txt")

Write-Host "[m9] staged: $stage"
Get-ChildItem -Recurse -File $stage | ForEach-Object { Write-Host ("  " + $_.FullName.Replace($root + '\', '')) }

if (-not $SkipZip) {
    $zip = Join-Path $root "dist\hds-client-$Version-windows-x64.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -Force
    $zh = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    "$zh  hds-client-$Version-windows-x64.zip" | Set-Content -Encoding ascii "$zip.sha256.txt"
    Write-Host "[m9] package: $zip"
    Write-Host "[m9] sha256:  $zh"
}
