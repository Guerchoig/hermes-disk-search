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

# Сервер уже работает?
$serverRunning = $false
try {
    Invoke-RestMethod -Uri "http://127.0.0.1:$uiPort/api/status" -TimeoutSec 3 | Out-Null
    $serverRunning = $true
} catch { }

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