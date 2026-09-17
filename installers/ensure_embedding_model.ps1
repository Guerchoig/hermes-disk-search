# Скачивание embedding-модели bge-m3 (GGUF, ~1,2 ГБ) в папку моделей LM Studio,
# если файл ещё не на месте, и попытка загрузить её через lms (best effort).
# Используется setup.ps1 (Windows). Аналог для macOS: ensure_embedding_model.sh
param([string]$ProjectRoot = (Split-Path (Split-Path $PSScriptRoot)))

$ErrorActionPreference = "Continue"
$gguf = Join-Path $env:USERPROFILE ".lmstudio\models\lm-kit\bge-m3-gguf\bge-m3-Q8_0.gguf"

if (Test-Path $gguf) {
    Write-Host "[ok] Модель эмбеддингов bge-m3 уже установлена: $gguf"
} else {
    Write-Host "[..] Модель эмбеддингов bge-m3 не найдена — скачиваю (~1,2 ГБ, разово)..." -ForegroundColor Cyan
    Write-Host "     https://huggingface.co/lm-kit/bge-m3-gguf"
    New-Item -ItemType Directory -Path (Split-Path $gguf) -Force | Out-Null
    & curl.exe -L --fail --progress-bar -o "$gguf.part" `
        "https://huggingface.co/lm-kit/bge-m3-gguf/resolve/main/bge-m3-Q8_0.gguf"
    if ($LASTEXITCODE -eq 0 -and (Test-Path "$gguf.part") -and ((Get-Item "$gguf.part").Length -gt 100MB)) {
        Move-Item "$gguf.part" $gguf -Force
        Write-Host "[ok] Модель bge-m3 скачана: $gguf" -ForegroundColor Green
    } else {
        Remove-Item "$gguf.part" -Force -ErrorAction SilentlyContinue
        Write-Host "[!!] Не удалось скачать модель. Скачайте файл вручную с" -ForegroundColor Yellow
        Write-Host "     https://huggingface.co/lm-kit/bge-m3-gguf (bge-m3-Q8_0.gguf)" -ForegroundColor Yellow
        Write-Host "     и положите в $gguf" -ForegroundColor Yellow
        Write-Host "     (можно позже из веб-интерфейса: кнопка «Скачать модель»)." -ForegroundColor Yellow
    }
}

# Попытка загрузить модель в LM Studio через lms CLI (best effort)
$lms = Get-Command lms -ErrorAction SilentlyContinue
if ($lms) {
    Write-Host "[..] Загрузка модели в LM Studio (lms load text-embedding-bge-m3)..."
    & lms load text-embedding-bge-m3 -y 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) {
        Write-Host "[ok] Модель загружена в LM Studio" -ForegroundColor Green
    } else {
        Write-Host "[--] Автозагрузка не удалась — загрузите модель в LM Studio:" -ForegroundColor Yellow
        Write-Host "     Developer -> Select a model to load -> text-embedding-bge-m3" -ForegroundColor Yellow
    }
}

# Итоговая проверка: сервер отвечает И модель реально загружена
try {
    $ids = (Invoke-RestMethod -Uri "http://localhost:1234/v1/models" -TimeoutSec 3).data |
        ForEach-Object { $_.id }
    if ($ids -contains "text-embedding-bge-m3") {
        Write-Host "[ok] Эмбеддинги готовы: модель загружена в LM Studio" -ForegroundColor Green
    } else {
        Write-Host "[--] Файл модели на месте, но сервер её не загрузил." -ForegroundColor Yellow
        Write-Host "     В LM Studio: Developer -> Select a model to load -> text-embedding-bge-m3," -ForegroundColor Yellow
        Write-Host "     или позже нажмите «Загрузить в LM Studio» в веб-интерфейсе." -ForegroundColor Yellow
    }
} catch {
    Write-Host "[--] LM Studio не запущен. Установите (https://lmstudio.ai), запустите сервер" -ForegroundColor Yellow
    Write-Host "     (Developer -> Start Server) и загрузите модель (Developer -> Load)." -ForegroundColor Yellow
    Write-Host "     Файл модели уже скачан — в списке моделей LM Studio она появится." -ForegroundColor Yellow
}
