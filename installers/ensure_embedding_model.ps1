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

# Фактический контекст загруженной embedding-модели (lms ps --json).
# При контексте меньше 8192 LM Studio МОЛЧА обрезает вход длиннее контекста —
# длинные чанки попадают в индекс неполно. Проверено замером: при ctx=8192
# текст 15 644 токена дал вектор, равный вектору первых ~8192 токенов.
function Get-EmbContext {
    try {
        $json = & lms ps --json 2>$null | Out-String
        foreach ($m in ($json | ConvertFrom-Json)) {
            if ("$($m.identifier)$($m.modelKey)".ToLower().Contains("bge-m3")) {
                return [int]$m.contextLength
            }
        }
    } catch { }
    return $null
}

# Попытка загрузить модель в LM Studio через lms CLI (best effort)
# 'lms load' при каждом вызове создаёт НОВЫЙ инстанс модели (дубликаты едят
# VRAM), поэтому сначала проверяем 'lms ps': уже загружена — не трогаем.
$lms = Get-Command lms -ErrorAction SilentlyContinue
if ($lms) {
    $loaded = @()
    try {
        $psOut = & lms ps 2>$null | Out-String
        foreach ($ln in ($psOut -split "`n")) {
            $t = $ln.Trim()
            if ($t -and -not $t.StartsWith("IDENTIFIER") -and ($t.Trim('-').Length -gt 0) -and
                $t.ToLower().Contains("text-embedding-bge-m3")) {
                $loaded += ($t -split '\s+')[0]
            }
        }
    } catch { }
    if ($loaded.Count -eq 1) {
        $ctx = Get-EmbContext
        if ($null -ne $ctx -and $ctx -lt 8192) {
            Write-Host "[!!] Модель загружена с контекстом $ctx (нужно 8192): LM Studio МОЛЧА" -ForegroundColor Yellow
            Write-Host "     обрезает вход длиннее контекста — длинные фрагменты индексируются неполно." -ForegroundColor Yellow
            Write-Host "[..] Перезагружаю модель с контекстом 8192..."
            foreach ($id in $loaded) { & lms unload $id 2>$null | Out-Null }
            & lms load text-embedding-bge-m3 --context-length 8192 -y 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) {
                Write-Host "[ok] Модель перезагружена с контекстом 8192" -ForegroundColor Green
                Write-Host "     Совет: запустите переиндексацию — часть чанков могла быть проиндексирована неполно." -ForegroundColor DarkGray
            }
        } else {
            Write-Host "[ok] Модель уже загружена в LM Studio ($($loaded[0]))" -ForegroundColor Green
        }
    } else {
        if ($loaded.Count -gt 1) {
            Write-Host "[..] Найдено $($loaded.Count) копий модели — выгружаю дубликаты..."
            foreach ($id in $loaded) { & lms unload $id 2>$null | Out-Null }
        }
        # --context-length обязателен: при меньшем контексте LM Studio молча
        # обрезает вход длиннее контекста (проверено: вектор совпадает с
        # вектором только первых токенов, без ошибки в ответе)
        Write-Host "[..] Загрузка модели в LM Studio (lms load text-embedding-bge-m3 --context-length 8192)..."
        & lms load text-embedding-bge-m3 --context-length 8192 -y 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Write-Host "[ok] Модель загружена в LM Studio (контекст 8192)" -ForegroundColor Green
        } else {
            Write-Host "[--] Автозагрузка не удалась — загрузите модель в LM Studio:" -ForegroundColor Yellow
            Write-Host "     Developer -> Select a model to load -> text-embedding-bge-m3" -ForegroundColor Yellow
        }
    }
    $ctxFinal = Get-EmbContext
    if ($null -ne $ctxFinal -and $ctxFinal -lt 8192) {
        Write-Host "[!!] Фактический контекст модели: $ctxFinal (должно быть 8192)." -ForegroundColor Yellow
        Write-Host "     Вручную: lms unload text-embedding-bge-m3; lms load text-embedding-bge-m3 --context-length 8192 -y" -ForegroundColor Yellow
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
