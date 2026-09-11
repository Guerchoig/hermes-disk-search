# Инсталлятор hermes-disk-search для Windows.
# Ставит недостающие компоненты; LM Studio и Hermes НЕ устанавливает — предупреждает и даёт ссылки.
$ErrorActionPreference = "Continue"
$root = Split-Path (Split-Path $PSScriptRoot)
Set-Location $root
Write-Host "== hermes-disk-search: установка (Windows) ==" -ForegroundColor Cyan

$haveWinget = [bool](Get-Command winget -ErrorAction SilentlyContinue)

# --- 1. Python ---
$py = Get-Command py -ErrorAction SilentlyContinue
$py3 = Get-Command python -ErrorAction SilentlyContinue
if (-not $py -and -not $py3) {
    Write-Host "[ ] Python не найден" -ForegroundColor Yellow
    if ($haveWinget) {
        winget install --id Python.Python.3.12 -e --accept-source-agreements --accept-package-agreements
    } else {
        Write-Host "   Установите Python вручную: https://www.python.org/downloads/" -ForegroundColor Yellow
    }
} else { Write-Host "[ok] Python найден" }

# --- 2. ffmpeg ---
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    if ($haveWinget) {
        Write-Host "[..] ffmpeg не найден — установка через winget (нужен для транскрипции видео)"
        winget install -e --id Gyan.FFmpeg --accept-source-agreements --accept-package-agreements
    } else {
        Write-Host "[--] ffmpeg не найден; установите с https://www.gyan.dev/ffmpeg/builds/" -ForegroundColor Yellow
    }
} else { Write-Host "[ok] ffmpeg найден" }

# --- 3. Tesseract OCR (опционально) ---
$ans = Read-Host "Установить Tesseract OCR (текст на картинках/сканах)? [y/N]"
if ($ans -match '^[YyДд]') {
    if ($haveWinget) {
        winget install -e --id UB-Mannheim.TesseractOCR --accept-source-agreements --accept-package-agreements
        Write-Host "   Русский язык OCR: если не установлен, скачайте пакет 'rus' с https://github.com/tesseract-ocr/tessdata" -ForegroundColor Yellow
    } else {
        Write-Host "   winget недоступен: https://github.com/UB-Mannheim/tesseract/wiki" -ForegroundColor Yellow
    }
}

# --- 4. LM Studio (не устанавливаем!) ---
$lms = $false
try {
    Invoke-RestMethod -Uri 'http://localhost:1234/v1/models' -TimeoutSec 3 | Out-Null
    $lms = $true
    Write-Host "[ok] LM Studio запущен (localhost:1234)" -ForegroundColor Green
} catch {
    Write-Host "[!!] LM Studio не отвечает на localhost:1234" -ForegroundColor Yellow
    Write-Host "     1) Установите: https://lmstudio.ai" -ForegroundColor Yellow
    Write-Host "     2) Запустите сервер: Developer -> Start Server" -ForegroundColor Yellow
    Write-Host "     3) Скачайте чат-модель (например qwen3.5-9b) и embedding-модель" -ForegroundColor Yellow
    Write-Host "        text-embedding-bge-m3 (https://huggingface.co/lm-kit/bge-m3-gguf)," -ForegroundColor Yellow
    Write-Host "        положите в %USERPROFILE%\.lmstudio\models\lm-kit\bge-m3-gguf\ и загрузите" -ForegroundColor Yellow
}

# --- 5. venv + зависимости ---
if (-not (Test-Path "$root\.venv")) {
    Write-Host "[..] Создание venv и установка зависимостей..."
    if ($py) { py -3 -m venv "$root\.venv" } else { python -m venv "$root\.venv" }
}
$vp = "$root\.venv\Scripts\python.exe"
& $vp -m pip install --upgrade pip -q
& $vp -m pip install -r "$root\requirements.txt" -q
& $vp -m pip install faster-whisper -q
& $vp -m pip install nvidia-cublas-cu12 nvidia-cudnn-cu12 -q
Write-Host "[ok] зависимости установлены" -ForegroundColor Green

# --- 5.1. Предзагрузка модели Whisper (через curl, в папку models\whisper-small) ---
Write-Host "[..] Предзагрузка модели Whisper (small, ~460 МБ) — разовая операция..."
$env:HF_HUB_OFFLINE = "1"; $env:HF_HUB_DISABLE_SYMLINKS_WARNING = "1"
& $vp -c "import sys; sys.path.insert(0, r'$root'); from hds.config import load; from hds.extract_av import _get_whisper; _get_whisper(load()); print('[ok] модель Whisper готова')"
Write-Host "     Совет: включите Режим разработчика Windows (Параметры -> Конфиденциальность ->" -ForegroundColor DarkGray
Write-Host "     Для разработчиков), чтобы кэш моделей не дублировал файлы на диске." -ForegroundColor Yellow

# --- 6. Диагностика ---
& $vp -m hds.cli check

# --- 7. Ярлыки и автозапуск ---
$ans = Read-Host "Создать ярлык «Индексация дисков» на рабочем столе? [Y/n]"
if ($ans -notmatch '^[Nn]') { & "$root\shortcuts\windows\create_shortcut.ps1" }
$ans = Read-Host "Настроить автозапуск наблюдателя файлов при входе в систему? [y/N]"
if ($ans -match '^[YyДд]') { & "$root\install_autostart.ps1" }

Write-Host ""
Write-Host "== Готово. Дальше: запустите run_index.ps1 (или ярлык) для первичной индексации ==" -ForegroundColor Cyan
if (-not $lms) {
    Write-Host "   Не забудьте про LM Studio: https://lmstudio.ai" -ForegroundColor Yellow
}
Read-Host "Enter для выхода"