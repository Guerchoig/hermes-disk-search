# Автозапуск наблюдателя при входе в Windows: Планировщик задач (нужны права)
# с фолбэком на папку автозагрузки (права не требуются).
$root = $PSScriptRoot
$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
if (-not (Test-Path $pythonw)) { Write-Error "Сначала выполните setup.ps1"; exit 1 }

$ok = $false
try {
    $action   = New-ScheduledTaskAction -Execute $pythonw -Argument "-m hds.cli watch" -WorkingDirectory $root
    $trigger  = New-ScheduledTaskTrigger -AtLogOn
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName "HermesDiskSearchWatch" -Action $action -Trigger $trigger -Settings $settings -Force -ErrorAction Stop | Out-Null
    $ok = $true
    Write-Host "[ok] Задача планировщика HermesDiskSearchWatch создана (запуск при входе)"
    Write-Host "Запустить сейчас: Start-ScheduledTask -TaskName HermesDiskSearchWatch"
    Write-Host "Удалить: Unregister-ScheduledTask -TaskName HermesDiskSearchWatch -Confirm:`$false"
} catch {
    Write-Host "[--] Планировщик недоступен ($($_.Exception.Message.Trim())) — использую папку автозагрузки" -ForegroundColor Yellow
}

if (-not $ok) {
    $startup = [Environment]::GetFolderPath('Startup')
    $ws = New-Object -ComObject WScript.Shell
    $lnk = $ws.CreateShortcut("$startup\HermesDiskSearchWatch.lnk")
    $lnk.TargetPath = $pythonw
    $lnk.Arguments = '-m hds.cli watch'
    $lnk.WorkingDirectory = $root
    $lnk.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
    $lnk.Save()
    Write-Host "[ok] Ярлык создан в автозагрузке: $startup\HermesDiskSearchWatch.lnk"
    Write-Host "Запуск при входе в систему; удалить — уберите ярлык из папки автозагрузки."
}
Write-Host "Процессы в диспетчере задач: pythonw.exe x2 (venv-лаунчер + реальный интерпретатор, аргументы: -m hds.cli watch)"