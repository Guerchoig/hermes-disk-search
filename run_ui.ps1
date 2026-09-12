# Запуск локального веб-интерфейса hermes-disk-search (браузер открывается сам)
$root = $PSScriptRoot
$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
if (-not (Test-Path $pythonw)) {
    Write-Host "== venv не найден. Сначала запустите install_windows.ps1 ==" -ForegroundColor Yellow
    Read-Host "Enter для выхода"
    exit 1
}
# Если сервер ещё не работает — запустить скрыто; затем открыть страницу
Start-Process $pythonw -ArgumentList '-m','hds.cli','ui' -WorkingDirectory $root -WindowStyle Hidden
Start-Sleep 1
Start-Process 'http://127.0.0.1:8765'