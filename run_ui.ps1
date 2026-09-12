# Запуск локального веб-интерфейса hermes-disk-search (браузер открывается сам)
# Идемпотентно: если UI-сервер уже работает — просто открывается страница.
$root = $PSScriptRoot
$uiPort = 8765
$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
if (-not (Test-Path $pythonw)) {
    Write-Host "== venv не найден. Сначала запустите install_windows.ps1 ==" -ForegroundColor Yellow
    Read-Host "Enter для выхода"
    exit 1
}

# Сервер уже работает?
$alive = $false
try {
    Invoke-RestMethod -Uri "http://127.0.0.1:$uiPort/api/status" -TimeoutSec 3 | Out-Null
    $serverRunning = $true
} catch { $serverRunning = $false }

if (-not $serverRunning) {
    Start-Process $pythonw -ArgumentList '-m','hds.cli','ui',('--port',"$uiPort") -WorkingDirectory $root -WindowStyle Hidden
    Start-Sleep 1
}
Start-Process "http://127.0.0.1:$uiPort"