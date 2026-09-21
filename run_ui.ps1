# Запуск локального веб-интерфейса hermes-disk-search (браузер открывается сам)
# Идемпотентно: если UI-сервер уже работает — просто открывается страница.
# Логи сервера (stdout/stderr): %LOCALAPPDATA%\hermes-disk-search\ui.log и ui.err.log
$root = $PSScriptRoot
$uiPort = 8765
$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
if (-not (Test-Path $pythonw)) {
    Write-Host "== venv не найден. Сначала запустите setup.ps1 ==" -ForegroundColor Yellow
    Read-Host "Enter для выхода"
    exit 1
}

# Логи UI-сервера: pythonw — GUI-процесс без консоли, весь вывод идёт в файлы
$logDir = Join-Path $env:LOCALAPPDATA "hermes-disk-search"
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$outLog = Join-Path $logDir "ui.log"
$errLog = Join-Path $logDir "ui.err.log"

# Сервер уже работает? Если да — какой версии?
# Старый сервер (код предыдущей установки) не знает новых эндпоинтов UI,
# поэтому при несовпадении версии он останавливается и запускается заново.
$curVersion = $null
$mVer = Select-String -Path (Join-Path $root "hds\__init__.py") `
    -Pattern '__version__\s*=\s*"([^"]+)"'
if ($mVer) { $curVersion = $mVer.Matches[0].Groups[1].Value }

function Stop-HdsUi {
    # UI-сервер запускается как pythonw (ярлык) или python (вручную); плюс
    # venv-«шим»: у лаунчера и реального интерпретатора одинаковая командная
    # строка. Ищем по содержимому cmdline, без фильтра по имени процесса.
    Get-CimInstance Win32_Process |
        Where-Object { $_.CommandLine -match '-m hds\.cli ui' } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

function Wait-PortFree([int]$Port) {
    # Stop-Process асинхронен: сокет освобождается не сразу (раньше был
    # Start-Sleep 1 — на медленных машинах новый сервер видел занятый порт
    # и молча выходил). Ждём до 10 с, пока /api/status перестанет отвечать.
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 500
        try {
            Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/status" -TimeoutSec 1 | Out-Null
        } catch { return $true }
    }
    return $false
}

$serverUp = $false
for ($attempt = 1; $attempt -le 2 -and -not $serverUp; $attempt++) {
    $up = $false
    $stVer = $null
    try {
        $st = Invoke-RestMethod -Uri "http://127.0.0.1:$uiPort/api/status" -TimeoutSec 3
        $up = $true
        $stVer = $st.app_version
    } catch { }

    if ($up) {
        if (-not $curVersion -or $stVer -eq $curVersion) {
            $serverUp = $true   # тёплый старт: нужная версия уже работает
            break
        }
        Write-Host "[..] На порту $uiPort — UI-сервер предыдущей установки (v$stVer); перезапускаю на v$curVersion..."
        Stop-HdsUi
        if (-not (Wait-PortFree $uiPort)) {
            Write-Host "[!!] Не удалось остановить прежний UI-сервер (порт $uiPort занят)." -ForegroundColor Yellow
        }
        continue   # следующая попытка: порт свободен → стартуем
    }

    # Порт свободен — стартуем сервер.
    # --no-browser: единственная точка открытия страницы — Start-Process ниже,
    # иначе браузер открывается дважды (сервер открывает сам + лаунчер).
    try {
        Start-Process $pythonw -ArgumentList '-m','hds.cli','ui','--port',"$uiPort",'--no-browser' `
            -WorkingDirectory $root `
            -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    } catch {
        Write-Host "Не удалось запустить UI-сервер: $($_.Exception.Message)" -ForegroundColor Red
        Write-Host "Запустите вручную для диагностики:" -ForegroundColor Yellow
        Write-Host "  cd `"$root`""
        Write-Host "  .venv\Scripts\python.exe -m hds.cli ui --port $uiPort"
        Read-Host "Enter для выхода"
        exit 1
    }
    # Ждём подъёма сервера до 15 сек
    $up = $false
    for ($i = 0; $i -lt 15 -and -not $up; $i++) {
        Start-Sleep 1
        try { Invoke-RestMethod -Uri "http://127.0.0.1:$uiPort/api/status" -TimeoutSec 2 | Out-Null; $up = $true } catch { }
    }
    if (-not $up) {
        break   # общий блок ошибки ниже (с хвостом ui.err.log)
    }
    # Контрольная сверка: на порту мог оказаться чужой/старый сервер
    # (гонка двух лаунчеров) — тогда гасим и стартуем ещё раз.
    try {
        $st2 = Invoke-RestMethod -Uri "http://127.0.0.1:$uiPort/api/status" -TimeoutSec 2
        if ($curVersion -and $st2.app_version -ne $curVersion) {
            Write-Host "[..] На порту оказался сервер v$($st2.app_version) вместо v$curVersion — перезапускаю..."
            Stop-HdsUi
            Wait-PortFree $uiPort | Out-Null
            continue
        }
    } catch { }
    $serverUp = $true
}

if (-not $serverUp) {
    Write-Host "== UI-сервер не запустился (v$curVersion, порт $uiPort) ==" -ForegroundColor Red
    Write-Host "Лог ошибок: $errLog" -ForegroundColor Yellow
    if (Test-Path $errLog) {
        Write-Host "--- последние строки ui.err.log ---"
        Get-Content $errLog -Tail 30
    }
    Write-Host "-----------------------------------"
    Read-Host "Enter для выхода"
    exit 1
}
Write-Host "Логи UI-сервера: $errLog"
Start-Process "http://127.0.0.1:$uiPort"
