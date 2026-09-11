# Запуск индексации дисков hermes-disk-search
$root = $PSScriptRoot
Set-Location $root
$py = Join-Path $root ".venv\Scripts\python.exe"
if (-not (Test-Path $py)) {
    Write-Host "== venv не найден. Сначала запустите install_windows.ps1 ==" -ForegroundColor Yellow
    Read-Host "Enter для выхода"
    exit 1
}
Write-Host "== hermes-disk-search: индексация (инкрементальная) ==" -ForegroundColor Cyan
& $py -m hds.cli index
Write-Host ""
Read-Host "Готово. Нажмите Enter для выхода"