# Создание ярлыка «Hermes Disk Search» (веб-интерфейс) на рабочем столе и в папке проекта
$root = $PSScriptRoot | Split-Path | Split-Path   # shortcuts\windows -> корень проекта
$ws = New-Object -ComObject WScript.Shell

function New-Lnk($path) {
    $lnk = $ws.CreateShortcut($path)
    $lnk.TargetPath = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    $lnk.Arguments  = '-NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $root 'run_ui.ps1') + '"'
    $lnk.WorkingDirectory = $root
    $lnk.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
    $lnk.Description = 'hermes-disk-search: веб-интерфейс (настройки, индексация, watcher)'
    $lnk.Save()
}

New-Lnk (Join-Path $root 'shortcuts\windows\Hermes Disk Search.lnk')
New-Lnk (Join-Path $env:USERPROFILE 'Desktop\Hermes Disk Search.lnk')
# старый ярлык индексации больше не нужен — UI умеет запускать индексацию
$old = Join-Path $env:USERPROFILE 'Desktop\Индексация дисков.lnk'
if (Test-Path $old) { Remove-Item $old -Force }
Write-Host 'Ярлык «Hermes Disk Search» создан на рабочем столе (открывает веб-интерфейс)'