# Build the Rust release binaries and stage a Windows x64 distribution folder (W4).
#
# Usage (from repo root):
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0
#
# Result: dist\hds-<Version>-windows-x64\ with bin\{hds,hds_mcp,llm_host}.exe,
# config.example.yaml, README.md and sha256.txt. ASCII-only (PS 5.1 reads a
# BOM-less .ps1 as ANSI and breaks on Cyrillic).
param(
    [string]$Version = "0.1.0"
)
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "[w4] building release binaries..."
cargo build --release -p hds-cli -p hds-mcp -p hds-llama
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$stage = Join-Path $root "dist\hds-$Version-windows-x64"
$binDir = Join-Path $stage "bin"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

$bins = @("hds", "hds_mcp", "llm_host")
foreach ($b in $bins) {
    $src = Join-Path $root "target\release\$b.exe"
    if (-not (Test-Path $src)) { throw "missing $src" }
    Copy-Item $src (Join-Path $binDir "$b.exe") -Force
}

Copy-Item (Join-Path $root "config.example.yaml") $stage -Force
Copy-Item (Join-Path $root "README.md") $stage -Force

# sha256 manifest (bin/*)
$lines = @()
foreach ($f in Get-ChildItem -File $binDir) {
    $h = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
    $lines += ("{0}  bin/{1}" -f $h, $f.Name)
}
$lines | Set-Content -Encoding ascii (Join-Path $stage "sha256.txt")

Write-Host "[w4] staged: $stage"
Get-ChildItem -Recurse -File $stage | ForEach-Object { Write-Host ("  " + $_.FullName.Replace($root + '\', '')) }
