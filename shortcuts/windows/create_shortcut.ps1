# Создание ярлыка «Индексация дисков» на рабочем столе и в папке проекта
$root = $PSScriptRoot | Split-Path | Split-Path   # корень проекта (shortcuts/windows -> project)
$ws = New-Object -ComObject WScript.Shell

function New-Lnk($path) {
    $lnk = $ws.CreateShortcut($path)
    $lnk.TargetPath = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    $lnk.Arguments  = '-NoProfile -ExecutionPolicy Bypass -File "' + (Join-Path $root 'run_index.ps1') + '"'
    $lnk.WorkingDirectory = $root
    $lnk.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
    $lnk.Description = 'hermes-disk-search: инкрементальная индексация дисков'
    $lnk.Save()
}

New-Lnk (Join-Path $root 'shortcuts\windows\Индексация дисков.lnk')
New-Lnk (Join-Path $env:USERPROFILE 'Desktop\Индексация дисков.lnk')
Write-Host 'Ярлыки созданы: рабочий стол + shortcuts\windows\'