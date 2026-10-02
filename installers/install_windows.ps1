# Historical entry point: all Windows installation lives in setup.ps1 (repo root).
# Kept for compatibility - this script just delegates to setup.ps1.
# ASCII-only on purpose (Windows PowerShell 5.1 + BOM-less .ps1).
$ErrorActionPreference = "Continue"
$root = Split-Path $PSScriptRoot -Parent
Write-Host "== hermes-disk-search: Windows installer ==" -ForegroundColor Cyan
Write-Host "Installation is unified in setup.ps1 - starting it..."
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "setup.ps1")
