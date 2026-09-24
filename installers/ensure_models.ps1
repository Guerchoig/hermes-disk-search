# Скачивание GGUF-моделей llama-server в папку проекта models\:
#   embedding: bge-m3-Q8_0 (~1,2 ГБ)
#   chat:      qwen3.5-9b Q6_K (~7,5 ГБ, unsloth/Qwen3.5-9B-Instruct-GGUF)
# Идемпотентно: существующий файл не перекачивается. Если модель уже скачана
# в ~/.lmstudio (старая установка), копируется оттуда без повторной загрузки.
# Используется setup.ps1 (Windows). Аналог для macOS: ensure_models.sh
param([string]$ProjectRoot = (Split-Path (Split-Path $PSScriptRoot)))

$ErrorActionPreference = "Continue"

$models = @(
    @{ dir = "embedding"; file = "bge-m3-Q8_0.gguf"; minMB = 500;
       url = "https://huggingface.co/lm-kit/bge-m3-gguf/resolve/main/bge-m3-Q8_0.gguf" },
    @{ dir = "chat"; file = "qwen3.5-9b-Q6_K.gguf"; minMB = 4000;
       url = "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q6_K.gguf" }
)

foreach ($m in $models) {
    $dest = Join-Path $ProjectRoot ("models\" + $m.dir + "\" + $m.file)
    if (Test-Path $dest) {
        Write-Host "[ok] $($m.file) уже установлен: $dest"
        continue
    }
    New-Item -ItemType Directory -Path (Split-Path $dest) -Force | Out-Null

    # Быстрый путь: копирование из старой установки LM Studio (~/.lmstudio)
    $lmsDir = Join-Path $env:USERPROFILE ".lmstudio\models"
    $old = $null
    if (Test-Path $lmsDir) {
        $old = Get-ChildItem -Path $lmsDir -Recurse -Filter $m.file -ErrorAction SilentlyContinue |
            Sort-Object Length -Descending | Select-Object -First 1
        if (-not $old) {
            $base = ($m.file -replace "-Q6_K\.gguf$", "*Q6_K*.gguf")
            $old = Get-ChildItem -Path $lmsDir -Recurse -Filter $base -ErrorAction SilentlyContinue |
                Sort-Object Length -Descending |
                Where-Object { $_.Length -gt $m.minMB * 1MB } | Select-Object -First 1
        }
    }
    if ($old -and (Test-Path $old.FullName)) {
        Write-Host "[..] Найдена модель из LM Studio: $($old.FullName) — копирую в models\..."
        Copy-Item $old.FullName $dest -Force
        Write-Host "[ok] Скопировано: $dest" -ForegroundColor Green
        continue
    }

    Write-Host "[..] Скачиваю $($m.file) (~$([math]::Round($m.minMB / 1000.0, 1)) ГБ, разово)..." -ForegroundColor Cyan
    Write-Host "     $($m.url)"
    & curl.exe -L --fail --progress-bar -o "$dest.part" $m.url
    if ($LASTEXITCODE -eq 0 -and (Test-Path "$dest.part") -and
            ((Get-Item "$dest.part").Length -gt $m.minMB * 1MB)) {
        Move-Item "$dest.part" $dest -Force
        Write-Host "[ok] Скачано: $dest" -ForegroundColor Green
    } else {
        Remove-Item "$dest.part" -Force -ErrorAction SilentlyContinue
        Write-Host "[!!] Не удалось скачать $($m.file). Скачайте вручную:" -ForegroundColor Yellow
        Write-Host "     $m.url" -ForegroundColor Yellow
        Write-Host "     и положите в $dest" -ForegroundColor Yellow
    }
}

# Токен HuggingFace (гейтнутые репозитории): HF_TOKEN в переменных окружения
if (-not $env:HF_TOKEN) {
    Write-Host "[--] HF_TOKEN не задан (нужен только для гейтнутых репозиториев)" -ForegroundColor DarkGray
}