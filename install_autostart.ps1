# Автозапуск наблюдателя при входе в Windows (Планировщик задач, без окна)
$root = $PSScriptRoot
$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
$module = "hds.cli"
if (-not (Test-Path $pythonw)) { Write-Error "Сначала выполните setup.ps1"; exit 1 }

$action  = New-ScheduledTaskAction -Execute $pythonw -Argument "-m hds.cli watch" -WorkingDirectory $root
$trigger = New-ScheduledTaskTrigger -AtLogOn
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName "HermesDiskSearchWatch" -Action $action -Trigger $trigger -Settings $settings -Force
Write-Host "Готово. Запустить сейчас: Start-ScheduledTask -TaskName HermesDiskSearchWatch"
Write-Host "Удалить: Unregister-ScheduledTask -TaskName HermesDiskSearchWatch -Confirm:\$false"