# Установка hermes-disk-search: venv + зависимости
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
Set-Location $root

$py = Get-Command py -ErrorAction SilentlyContinue
if ($py) { $pyCmd = "py -3" } else { $pyCmd = "python" }
Write-Host "== Создание venv =="
Invoke-Expression "$pyCmd -m venv $root\.venv"

$python = "$root\.venv\Scripts\python.exe"
& $python -m pip install --upgrade pip

Write-Host "== Установка зависимостей =="
& $python -m pip install -r "$root\requirements.txt"

Write-Host "== Опционально: faster-whisper (транскрипция на CUDA) =="
& $python -m pip install faster-whisper
& $python -m pip install nvidia-cublas-cu12 nvidia-cudnn-cu12

Write-Host "== Диагностика =="
& $python -m hds.cli check