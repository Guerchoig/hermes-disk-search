# Build the Rust release binaries and stage a Windows x64 distribution folder (W4).
#
# Usage (from the repo root):
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 -Version 0.1.0
#   ... -SkipBuild          # reuse existing target\release binaries (e.g. the resident holds
#                           # llm_host.exe -> cargo build --release fails with os error 5)
#
# Result:
#   dist\hds-<Version>-windows-x64\              full install layout (bin, installers, sidecar, ...)
#   dist\hds-<Version>-windows-x64.zip           the same tree as a zip (contents at the zip root)
#   dist\hds-<Version>-windows-x64.zip.sha256.txt
#
# NOTE: a running resident (bin\llm_host.exe or target\release\llm_host.exe) locks its
# exe and breaks the release build (os error 5) - stop it first.
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [string]$Version = "0.1.0",
    [switch]$SkipZip,
    [switch]$WithSidecar,
    [string]$SidecarDir = "",
    [switch]$SkipBuild
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $SkipBuild) {
    Write-Host "[w4] building release binaries..."
    cargo build --release -p hds-cli -p hds-mcp -p hds-llama
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} else {
    Write-Host "[w4] -SkipBuild: reusing existing target\release binaries"
}

$stage = Join-Path $root "dist\hds-$Version-windows-x64"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
$binDir = Join-Path $stage "bin"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

# binaries
foreach ($b in @("hds", "hds_mcp", "llm_host")) {
    $src = Join-Path $root "target\release\$b.exe"
    if (-not (Test-Path $src)) { throw "missing $src" }
    Copy-Item $src (Join-Path $binDir "$b.exe") -Force
}

# installation scripts at the archive root (setup.ps1 resolves paths relative to itself)
foreach ($f in @("setup.cmd", "setup.ps1", "install-server.cmd", "update.cmd",
                 "install_hermes.ps1", "install_cline.ps1",
                 "install_autostart.ps1", "run_ui.ps1", "run_index.ps1", "hds_bin.ps1")) {
    $src = Join-Path $root $f
    if (Test-Path $src) { Copy-Item $src (Join-Path $stage $f) -Force }
}

# RELEASING.md section 1: a silently missing release file breaks the release -
# the server install/update wrappers must be in the archive.
foreach ($f in @("install-server.cmd", "update.cmd")) {
    if (-not (Test-Path (Join-Path $stage $f))) { throw "missing $f in $stage (repo root)" }
}

# directories copied as a whole (sidecar is handled separately below)
foreach ($d in @("installers", "runtime-manifests", "assets", "hermes-skill", "cline-rules")) {
    $src = Join-Path $root $d
    if (-not (Test-Path $src)) { continue }
    $dst = Join-Path $stage $d
    New-Item -ItemType Directory -Force -Path $dst | Out-Null
    Copy-Item (Join-Path $src "*") $dst -Recurse -Force
    Get-ChildItem -Path $dst -Recurse -Directory -Filter "__pycache__" -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

# sidecar: self-contained tree (`build-sidecar` job / -WithSidecar) or the repo copy
$sidecarDst = Join-Path $stage "sidecar"
if ($WithSidecar) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\build_sidecar.ps1") -OutDir $sidecarDst
    if ($LASTEXITCODE -ne 0) { throw "build_sidecar.ps1 failed" }
} else {
    if (-not $SidecarDir) {
        $SidecarDir = Join-Path $root "sidecar"
    } elseif (-not [System.IO.Path]::IsPathRooted($SidecarDir)) {
        $SidecarDir = Join-Path $root $SidecarDir
    }
    if (-not (Test-Path $SidecarDir)) { throw "sidecar dir not found: $SidecarDir" }
    New-Item -ItemType Directory -Force -Path $sidecarDst | Out-Null
    Copy-Item (Join-Path $SidecarDir "*") $sidecarDst -Recurse -Force
    Get-ChildItem -Path $sidecarDst -Recurse -Directory -Filter "__pycache__" -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

# desktop shortcut helper
$shortcutDir = Join-Path $stage "shortcuts\windows"
New-Item -ItemType Directory -Force -Path $shortcutDir | Out-Null
Copy-Item (Join-Path $root "shortcuts\windows\create_shortcut.ps1") $shortcutDir -Force

# docs (NOTICE.md is optional until the attribution text lands)
# README_SERVER_MULTIUSER.md is REQUIRED: it is what the server administrator reads
# to install the system. installers\client_readme.md is REQUIRED too: it is the user
# memo handed out to the people who connect their PCs (checked below).
foreach ($f in @("config.example.yaml", "server_config.example.yaml", "README.md", "README_SERVER_MULTIUSER.md", "NOTICE.md")) {
    $src = Join-Path $root $f
    if (Test-Path $src) { Copy-Item $src (Join-Path $stage $f) -Force }
}
foreach ($f in @("README.md", "README_SERVER_MULTIUSER.md")) {
    if (-not (Test-Path (Join-Path $stage $f))) { throw "missing $f in $stage (repo root)" }
}
# the user memo ships inside installers\ (copied above) and inside the client package
if (-not (Test-Path (Join-Path $stage "installers\client_readme.md"))) {
    throw "missing installers\client_readme.md in $stage (user memo)"
}

# --- версия продукта и вложенный клиентский пакет (M9/W7) --------------------
# version.txt: версию отсюда отдают /health и мастера тонкого клиента
# (hds_core::client::product_version). Единственный источник — аргумент -Version.
Set-Content -Encoding ascii (Join-Path $stage "version.txt") -Value $Version

# Тонкий клиентский пакет собираем тем же скриптом (hds.exe уже собран) и
# ВКЛАДЫВАЕМ в поставку: сервер отдаёт его по `<mcp-path>/client-package.zip`
# (канал поставки «с сервера»), а сам zip уходит в релиз отдельным ассетом.
Write-Host "[m9] building embedded thin client package..."
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "installers\build_client_release.ps1") -Version $Version -SkipBuild
if ($LASTEXITCODE -ne 0) { throw "build_client_release.ps1 failed" }
$clientZip = Join-Path $root "dist\hds-client-$Version-windows-x64.zip"
if (-not (Test-Path $clientZip)) { throw "missing $clientZip" }
$clientDst = Join-Path $stage "client"
New-Item -ItemType Directory -Force -Path $clientDst | Out-Null
Copy-Item $clientZip $clientDst -Force
if (Test-Path "$clientZip.sha256.txt") { Copy-Item "$clientZip.sha256.txt" $clientDst -Force }

# Скрипты-бутстрапы: сервер отдаёт их ПУБЛИЧНО (без токена) по
# `<mcp-path>/client-install.ps1|sh` — это и есть «общедоступное место», откуда
# клиент скачивает установщик; токен пользователь передаёт аргументом.
foreach ($f in @("client-install.ps1", "client-install.sh")) {
    $src = Join-Path $root "installers\$f"
    if (Test-Path $src) { Copy-Item $src $clientDst -Force }
}

# sha256 manifest for bin/* and client/*
$lines = @()
foreach ($f in Get-ChildItem -File $binDir) {
    $h = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
    $lines += ("{0}  bin/{1}" -f $h, $f.Name)
}
foreach ($f in Get-ChildItem -File $clientDst) {
    $h = (Get-FileHash $f.FullName -Algorithm SHA256).Hash.ToLower()
    $lines += ("{0}  client/{1}" -f $h, $f.Name)
}
$lines | Set-Content -Encoding ascii (Join-Path $stage "sha256.txt")

Write-Host "[w4] staged: $stage"
Get-ChildItem -Recurse -File $stage | ForEach-Object { Write-Host ("  " + $_.FullName.Replace($root + '\', '')) }

if (-not $SkipZip) {
    $zip = Join-Path $root "dist\hds-$Version-windows-x64.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -Force
    $zh = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    "$zh  hds-$Version-windows-x64.zip" | Set-Content -Encoding ascii "$zip.sha256.txt"
    Write-Host "[w4] package: $zip"
    Write-Host "[w4] sha256:  $zh"
}
