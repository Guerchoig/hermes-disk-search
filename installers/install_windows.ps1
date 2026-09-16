# Историческая точка входа: вся установка Windows объединена в setup.ps1 (корень проекта).
# Скрипт оставлен для совместимости — просто передаёт управление setup.ps1.
$ErrorActionPreference = "Continue"
$root = Split-Path (Split-Path $PSScriptRoot)
Write-Host "== hermes-disk-search: установка (Windows) ==" -ForegroundColor Cyan
Write-Host "Установка объединена в setup.ps1 — запускаю его..."
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root "setup.ps1")