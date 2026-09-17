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
$serverRunning = $false
$serverVersion = $null
try {
    $st = Invoke-RestMethod -Uri "http://127.0.0.1:$uiPort/api/status" -TimeoutSec 3
    $serverRunning = $true
    $serverVersion = $st.app_version
} catch { }

$curVersion = (Select-String -Path (Join-Path $root "hds\__init__.py") `
    -Pattern '__version__\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value

if ($serverRunning -and $serverVersion -ne $curVersion) {
    Write-Host "[..] На порту $uiPort — UI-сервер предыдущей установки (v$serverVersion); перезапускаю на v$curVersion..."
    Get-CimInstance Win32_Process -Filter "Name = 'pythonw.exe'" |
        Where-Object { $_.CommandLine -match '-m hds\.cli ui' } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    Start-Sleep 1
    $serverRunning = $false
}

if (-not $serverRunning) {
    try {
        Start-Process $pythonw -ArgumentList '-m','hds.cli','ui','--port',"$uiPort" `
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
        Write-Host "== UI-сервер не запустился ==" -ForegroundColor Red
        Write-Host "Лог ошибок: $errLog" -ForegroundColor Yellow
        if (Test-Path $errLog) {
            Write-Host "--- последние строки ui.err.log ---"
            Get-Content $errLog -Tail 30
        }
        Write-Host "-----------------------------------"
        Read-Host "Enter для выхода"
        exit 1
    }
}
Write-Host "Логи UI-сервера: $errLog"
Start-Process "http://127.0.0.1:$uiPort"