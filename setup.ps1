# Единый установщик hermes-disk-search для Windows:
# системные зависимости (ffmpeg/Tesseract по запросу) + venv + зависимости +
# модель эмбеддингов bge-m3 + диагностика + ярлык/автозапуск + интеграция с Hermes.
# Пользователю достаточно запустить этот скрипт; всё остальное — из веб-интерфейса.
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
Set-Location $root

# Снятие пометки «скачано из интернета» (Zone.Identifier): архив релиза,
# распакованный Проводником, передаёт её всем файлам, и политика RemoteSigned
# требует цифровую подпись для запуска скриптов. Текущий запуск уже идёт
# с -ExecutionPolicy Bypass, поэтому снять пометку безопасно — повторные
# запуски и «Запустить с помощью PowerShell» будут работать без Bypass.
try {
    Get-ChildItem -Path $root -Recurse -File -ErrorAction SilentlyContinue |
        Unblock-File -ErrorAction SilentlyContinue
} catch { }

# --- Поиск рабочего Python 3.x ---
# Кандидаты: py-лаунчер, затем python из PATH.
# Заглушка Microsoft Store (WindowsApps) отсеивается фактическим запуском --version.
$pyExe = $null
$pyArgs = @()
foreach ($cand in @(@{ exe = "py"; args = @("-3") }, @{ exe = "python"; args = @() })) {
    $cmd = Get-Command $cand.exe -ErrorAction SilentlyContinue
    if (-not $cmd) { continue }
    try {
        $ver = & $cmd.Source @($cand.args + @("--version")) 2>&1
        if ($LASTEXITCODE -eq 0 -and "$ver" -match "Python 3\.") {
            $pyExe = $cmd.Source
            $pyArgs = $cand.args
            break
        }
    } catch { }
}
if (-not $pyExe) {
    Write-Host "ОШИБКА: не найден рабочий Python 3.x." -ForegroundColor Red
    Write-Host "Возможные причины:"
    Write-Host "  1) Python не установлен: команда 'python' — это заглушка Microsoft Store,"
    Write-Host "     которая ничего не запускает (открывает магазин)."
    Write-Host "  2) Установлен только Python 2 или 'python' не добавлен в PATH."
    Write-Host ""
    Write-Host "Установите Python 3.10+ с https://www.python.org/downloads/"
    Write-Host "(при установке отметьте галочку 'Add python.exe to PATH'),"
    Write-Host "затем откройте НОВОЕ окно PowerShell и запустите setup.ps1 снова."
    exit 1
}
Write-Host "[ok] Python: $pyExe $($pyArgs -join ' ')"

# --- Системные зависимости через winget ---
$haveWinget = [bool](Get-Command winget -ErrorAction SilentlyContinue)

# ffmpeg (транскрипция видео) — ставим автоматически
if (-not (Get-Command ffmpeg -ErrorAction SilentlyContinue)) {
    if ($haveWinget) {
        Write-Host "[..] ffmpeg не найден — установка через winget (нужен для транскрипции видео)..."
        winget install -e --id Gyan.FFmpeg --accept-source-agreements --accept-package-agreements
        Write-Host "     Если ffmpeg не появился в PATH — откройте НОВОЕ окно PowerShell и перезапустите setup.ps1." -ForegroundColor DarkGray
    } else {
        Write-Host "[--] ffmpeg не найден, winget недоступен (видео будут без транскрипции)." -ForegroundColor Yellow
        Write-Host "     Установите вручную: https://www.gyan.dev/ffmpeg/builds/" -ForegroundColor Yellow
    }
} else {
    Write-Host "[ok] ffmpeg найден"
}

# Tesseract OCR (текст на картинках/сканах) — по разрешению пользователя
if (-not (Get-Command tesseract -ErrorAction SilentlyContinue) -and
        -not (Test-Path "$env:ProgramFiles\Tesseract-OCR\tesseract.exe")) {
    $ans = Read-Host "[?] Установить Tesseract OCR (текст на картинках/сканах)? [y/N]"
    if ($ans -match '^[YyДд]') {
        if ($haveWinget) {
            winget install -e --id UB-Mannheim.TesseractOCR --accept-source-agreements --accept-package-agreements
            Write-Host "   Русский язык OCR: если не установлен, скачайте пакет 'rus' с https://github.com/tesseract-ocr/tessdata" -ForegroundColor Yellow
        } else {
            Write-Host "   winget недоступен: https://github.com/UB-Mannheim/tesseract/wiki" -ForegroundColor Yellow
        }
    } else {
        Write-Host "[--] Пропущено: картинки будут индексироваться без OCR (можно установить позже)." -ForegroundColor DarkGray
    }
} else {
    Write-Host "[ok] Tesseract OCR найден"
}

# llama.cpp (llama-server) — локальный LLM-бэкенд: пре-билд с GitHub Releases.
# CUDA-сборка при NVIDIA, иначе Vulkan (AMD/Intel); llama-server переживает
# перезапуски UI (отвязанный процесс, управление — python -m hds.llama_server)
$llamaDir = "$root\tools\llama.cpp"
$llamaExe = "$llamaDir\llama-server.exe"
if (Get-Command llama-server -ErrorAction SilentlyContinue) {
    Write-Host "[ok] llama-server найден в PATH: $((Get-Command llama-server).Source)"
} elseif (Test-Path $llamaExe) {
    Write-Host "[ok] llama-server уже установлен: $llamaExe"
} else {
    $assetPattern = "llama-*bin-win-cuda*x64*.zip"
    if (-not (Get-Command nvidia-smi -ErrorAction SilentlyContinue)) {
        $assetPattern = "*bin-win-vulkan-x64*.zip"
    }
    Write-Host "[..] Скачиваю пре-билд llama.cpp ($assetPattern)..."
    try {
        # latest-релиз llama.cpp может не содержать бинарей (только nightly-tag) —
        # берём ПЕРВЫЙ релиз, где есть win-сборка (перебор последних 10)
        $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=10" -TimeoutSec 30 |
            Where-Object { ($_.assets | Where-Object { $_.name -like $assetPattern }).Count -gt 0 } |
            Select-Object -First 1
        if (-not $rel) { throw "в последних релизах llama.cpp нет ассета $assetPattern" }
        $asset = $rel.assets | Where-Object { $_.name -like $assetPattern } | Select-Object -First 1
        if (-not $asset) { throw "в релизе $($rel.tag_name) нет ассета $assetPattern" }
        $zipPath = Join-Path $env:TEMP $asset.name
        Write-Host "[..] Загрузка $($asset.name) (llama.cpp $($rel.tag_name))..."
        $ProgressPreference = "SilentlyContinue"  # иначе прогресс-бар замедляет загрузку в разы
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -TimeoutSec 900
        if ((Get-Item $zipPath).Length -ne $asset.size) {
            throw ("zip недокачан: получено {0} байт, ожидалось {1} — перезапустите setup.ps1" -f (Get-Item $zipPath).Length, $asset.size)
        }
        New-Item -ItemType Directory -Force -Path $llamaDir | Out-Null
        Expand-Archive -Path $zipPath -DestinationPath $llamaDir -Force
        Remove-Item $zipPath -ErrorAction SilentlyContinue
        # CUDA-сборке нужен runtime: рядом лежит cudart-llama-bin-win-cuda-*.zip
        if ($assetPattern -like "*cuda*") {
            $cudart = $rel.assets | Where-Object { $_.name -like "cudart-llama-bin-win-cuda*x64.zip" } | Select-Object -First 1
            if ($cudart) {
                $cudartZip = Join-Path $env:TEMP $cudart.name
                Write-Host "[..] Загрузка $($cudart.name) (CUDA runtime)..."
                Invoke-WebRequest -Uri $cudart.browser_download_url -OutFile $cudartZip -TimeoutSec 900
                if ((Get-Item $cudartZip).Length -ne $cudart.size) {
                    throw ("cudart zip недокачан: {0} != {1}" -f (Get-Item $cudartZip).Length, $cudart.size)
                }
                Expand-Archive -Path $cudartZip -DestinationPath $llamaDir -Force
                Remove-Item $cudartZip -ErrorAction SilentlyContinue
            } else {
                # без cudart-DLL (cublas64_12.dll и др.) CUDA-сборка llama-server
                # не стартует — предупреждаем сразу, а не молчаливым сбоем --version
                Write-Host "[!!] В релизе $($rel.tag_name) нет cudart-llama-bin-win-cuda-*.zip —" -ForegroundColor Yellow
                Write-Host "     CUDA-сборка llama-server может не стартовать. Скачайте cudart вручную" -ForegroundColor Yellow
                Write-Host "     с https://github.com/ggml-org/llama.cpp/releases в $llamaDir" -ForegroundColor Yellow
            }
        }
        # llama.cpp кладёт бинари в подпапку вида llama-<tag>-bin-win-.../
        $nested = Get-ChildItem -Path $llamaDir -Recurse -Filter "llama-server.exe" | Select-Object -First 1
        if ($nested -and (Split-Path $nested.FullName) -ne $llamaDir) {
            Move-Item (Join-Path (Split-Path $nested.FullName) "*.exe") $llamaDir -Force -ErrorAction SilentlyContinue
        }
        & $llamaExe --version | Select-Object -First 1
        if ($LASTEXITCODE -ne 0) { throw "llama-server --version упал с кодом $LASTEXITCODE" }
        Write-Host "[ok] llama-server установлен: $llamaExe" -ForegroundColor Green
    } catch {
        Write-Host "[!!] Не удалось скачать llama.cpp: $_" -ForegroundColor Yellow
        Write-Host "     Скачайте бинарь вручную с https://github.com/ggml-org/llama.cpp/releases" -ForegroundColor Yellow
        Write-Host "     и укажите путь в config.yaml (llm_server.bin)" -ForegroundColor Yellow
    }
}

# --- Создание venv ---
$venvDir = "$root\.venv"
$venvPython = "$venvDir\Scripts\python.exe"
$venvOk = $false
if ((Test-Path $venvPython) -and (Test-Path "$venvDir\pyvenv.cfg")) {
    # Если архив распакован вместе со старой .venv, её пути могут указывать
    # на несуществующий Python — проверяем, что базовый интерпретатор на месте.
    $venvHome = (Select-String -Path "$venvDir\pyvenv.cfg" -Pattern '^\s*home\s*=\s*(.+)$').Matches |
        Select-Object -First 1 | ForEach-Object { $_.Groups[1].Value.Trim() }
    $venvOk = ($venvHome -ne "") -and (Test-Path $venvHome)
}
if (-not $venvOk) {
    if (Test-Path $venvDir) {
        Write-Host "[..] Найдена неполная/перенесённая .venv — удаляю и создаю заново..."
        Remove-Item -Recurse -Force $venvDir
    }
    Write-Host "== Создание venv =="
    & $pyExe @pyArgs -m venv $venvDir
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $venvPython)) {
        Write-Host "ОШИБКА: не удалось создать venv в '$venvDir'." -ForegroundColor Red
        Write-Host "Проверьте, что модуль venv доступен:"
        Write-Host "    & '$pyExe' $($pyArgs -join ' ') -m venv --help"
        exit 1
    }
}

$python = $venvPython
& $python -m pip install --upgrade pip
if ($LASTEXITCODE -ne 0) { Write-Host "ОШИБКА: pip install --upgrade pip завершился с ошибкой." -ForegroundColor Red; exit 1 }

Write-Host "== Установка зависимостей =="
& $python -m pip install -r "$root\requirements.txt"
if ($LASTEXITCODE -ne 0) { Write-Host "ОШИБКА: установка зависимостей из requirements.txt не удалась." -ForegroundColor Red; exit 1 }

Write-Host "== Опционально: faster-whisper (транскрипция на CUDA) =="
& $python -m pip install faster-whisper
& $python -m pip install nvidia-cublas-cu12 nvidia-cudnn-cu12

Write-Host "== GPU для транскрипции (AMD/Intel без CUDA → whisper.cpp Vulkan) =="
$cudaCount = (& $python -c "import ctranslate2; print(ctranslate2.get_cuda_device_count())" 2>$null | Select-Object -Last 1)
if ("$cudaCount".Trim() -eq "0") {
    $gpu = Get-CimInstance Win32_VideoController -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match 'AMD|Radeon|NVIDIA|GeForce|Intel' } | Select-Object -First 1
    if ($gpu) {
        Write-Host "[..] CUDA не найдена, GPU: $($gpu.Name) — установка whisper.cpp (Vulkan)..."
        & $python -m hds.cli vulkan-setup
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[--] whisper.cpp не установлен — транскрипция будет на CPU (int8)." -ForegroundColor Yellow
            Write-Host "     Позже: .venv\Scripts\python.exe -m hds.cli vulkan-setup (или вручную)." -ForegroundColor Yellow
        }
    } else {
        Write-Host "[--] Дискретный GPU с Vulkan не обнаружен — транскрипция на CPU (int8)" -ForegroundColor DarkGray
    }
} else {
    Write-Host "[ok] CUDA доступна ($cudaCount) — whisper.cpp (Vulkan) не требуется"
}

# --- CLIP-эмбеддинги картинок: PyTorch с CUDA при NVIDIA ---
# requirements.txt ставит обычный (CPU) torch: CLIP-кодировщик картинок
# (sentence-transformers, hds/clip_index.py) при индексации фото работал бы
# на CPU даже на NVIDIA-машине. Ставим CUDA-сборку по согласию пользователя
# (колесо ~2,5 ГБ); при неудаче индексация продолжит работать — CLIP на CPU.
if (Get-Command nvidia-smi -ErrorAction SilentlyContinue) {
    $torchCuda = & $python -c "import torch; print(torch.cuda.is_available())" 2>$null | Select-Object -Last 1
    if ("$torchCuda".Trim() -ne "True") {
        $ans = Read-Host "[?] Установить PyTorch с CUDA (CLIP-эмбеддинги картинок на GPU, ~2,5 ГБ)? [y/N]"
        if ($ans -match '^[YyДд]') {
            $done = $false
            foreach ($idx in @("https://download.pytorch.org/whl/cu126",
                               "https://download.pytorch.org/whl/cu124",
                               "https://download.pytorch.org/whl/cu121")) {
                Write-Host "[..] pip install torch --index-url $idx ..."
                & $python -m pip install --upgrade torch --index-url $idx -q
                if ($LASTEXITCODE -eq 0) {
                    $ok = & $python -c "import torch; print(torch.cuda.is_available())" 2>$null | Select-Object -Last 1
                    if ("$ok".Trim() -eq "True") { $done = $true; break }
                }
            }
            if ($done) {
                Write-Host "[ok] PyTorch CUDA установлен — CLIP-эмбеддинги картинок на GPU" -ForegroundColor Green
            } else {
                Write-Host "[--] PyTorch CUDA не установился (нет колеса под этот Python/драйвер) — CLIP на CPU." -ForegroundColor Yellow
                Write-Host "     Это не блокер: поиск картинок по содержанию продолжит работать (медленнее)." -ForegroundColor DarkGray
            }
        } else {
            Write-Host "[--] Пропущено: CLIP-эмбеддинги картинок останутся на CPU." -ForegroundColor DarkGray
        }
    } else {
        Write-Host "[ok] PyTorch CUDA уже доступна — CLIP-эмбеддинги картинок на GPU"
    }
}

Write-Host "== Модели llama-server: bge-m3 (~1,2 ГБ) + qwen3.5-9b Q6_K (~7,5 ГБ, если не установлены) =="
& powershell -NoProfile -ExecutionPolicy Bypass -File "$root\installers\ensure_models.ps1" -ProjectRoot $root

Write-Host "== Опционально: предзагрузка модели Whisper (~460 МБ, транскрипция аудио/видео) =="
$ans = Read-Host "Предзагрузить сейчас? [y/N]"
if ($ans -match '^[YyДд]') {
    $env:HF_HUB_OFFLINE = "1"; $env:HF_HUB_DISABLE_SYMLINKS_WARNING = "1"
    & $python -c "import sys; sys.path.insert(0, r'$root'); from hds.config import load; from hds.extract_av import _get_whisper; _get_whisper(load()); print('[ok] модель Whisper готова')"
    if ($LASTEXITCODE -ne 0) { Write-Host "[--] Не удалось: модель скачается при первой транскрипции" -ForegroundColor Yellow }
    Write-Host "     Совет: включите Режим разработчика Windows (Параметры -> Конфиденциальность ->" -ForegroundColor DarkGray
    Write-Host "     Для разработчиков), чтобы кэш моделей не дублировал файлы на диске." -ForegroundColor DarkGray
} else {
    Write-Host "[--] Пропущено: модель скачается автоматически при первой транскрипции." -ForegroundColor DarkGray
}

Write-Host "== Проверка config.yaml (пути с другого компьютера) =="
$cfgPath = "$root\config.yaml"
if (Test-Path $cfgPath) {
    # Читаем строго как UTF-8 (config.yaml может быть без BOM и с русскими комментариями;
    # Get-Content без -Encoding в PS 5.1 читал бы его в ANSI и портил кириллицу)
    $encCfg = New-Object System.Text.UTF8Encoding($false)
    $cfgText = [System.IO.File]::ReadAllText($cfgPath, $encCfg)
    $missingDrives = @()
    foreach ($m in [regex]::Matches($cfgText, "[`"']?([A-Za-z]):\\")) {
        $d = $m.Groups[1].Value + ":\"
        if ((Test-Path "$($m.Groups[1].Value):\") -eq $false -and ($missingDrives -notcontains $d)) { $missingDrives += $d }
    }
    if ($missingDrives.Count -gt 0) {
        Write-Host "[!!] config.yaml содержит пути на отсутствующих дисках: $($missingDrives -join ', ')" -ForegroundColor Yellow
        Write-Host "     (config.yaml перенесён с другого компьютера). Индексация и БД не заработают,"
        Write-Host "     пока db_path и index.roots не указывают на существующие пути."
        $ans = Read-Host "     Заменить пути на профиль этого компьютера ($env:USERPROFILE)? [Y/n]"
        if ($ans -notmatch '^[Nn]') {
            $up = $env:USERPROFILE
            # 1) db_path -> %USERPROFILE%\hermes-disk-search-db\index.db
            $rxDb = [regex]::new("(?m)^db_path:.*$")
            $cfgText = $rxDb.Replace($cfgText, { param($mm) "db_path: '$up\hermes-disk-search-db\index.db'" }, 1)
            # 2) пути списков (index.roots, exclude_paths) на отсутствующих дисках -> %USERPROFILE% (подпуть сохраняется)
            $rxList = [regex]::new('(?m)^(\s*-\s*)(["'']?)([A-Za-z]):\\+(.*?)\2\s*$')
            $cfgText = $rxList.Replace($cfgText, {
                param($mm)
                if ($missingDrives -contains ($mm.Groups[3].Value + ":\")) {
                    $rest = $mm.Groups[4].Value -replace "\\\\+", "\"
                    if ($rest) { "$($mm.Groups[1].Value)'$up\$rest'" } else { "$($mm.Groups[1].Value)'$up'" }
                } else { $mm.Value }
            })
            [System.IO.File]::WriteAllText($cfgPath, $cfgText, (New-Object System.Text.UTF8Encoding($false)))
            Write-Host "[ok] config.yaml обновлён. Корни индексации можно уточнить в веб-интерфейсе (run_ui.ps1)." -ForegroundColor Green
        }
    } else {
        Write-Host "[ok] config.yaml: все диски из путей существуют"
    }
}

Write-Host "== Диагностика =="
& $python -m hds.cli check

Write-Host "== Интеграция с Hermes Desktop (MCP-сервер + скилл) =="
# Не обязателен на этом шаге: если Hermes ещё не установлен, скрипт напечатает,
# как подключить позже, и завершится успешно.
& powershell -NoProfile -ExecutionPolicy Bypass -File "$root\install_hermes.ps1"

Write-Host "== Интеграция с Cline Desktop (MCP-сервер + скилл) =="
# Не обязательна на этом шаге: если Cline ещё не установлен, скрипт напечатает,
# как подключить позже, и завершится успешно.
& powershell -NoProfile -ExecutionPolicy Bypass -File "$root\install_cline.ps1"

Write-Host "== Ярлык на рабочем столе (веб-интерфейс) =="
& powershell -NoProfile -ExecutionPolicy Bypass -File "$root\shortcuts\windows\create_shortcut.ps1"

Write-Host "== Автозапуск наблюдателя файлов =="
$ans = Read-Host "Настроить автозапуск watcher'а при входе в систему? [y/N]"
if ($ans -match '^[YyДд]') {
    & powershell -NoProfile -ExecutionPolicy Bypass -File "$root\install_autostart.ps1"
}

Write-Host ""
Write-Host "== Готово. Дальше — только веб-интерфейс: ярлык «Hermes Disk Search» на рабочем столе ==" -ForegroundColor Cyan
Write-Host "   (корни индексации, ▶ Старт индексации, watcher, перенос базы, скачивание моделей — всё из UI)" -ForegroundColor Cyan
Read-Host "Enter для выхода"