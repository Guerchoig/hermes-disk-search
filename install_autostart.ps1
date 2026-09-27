# Автозапуск наблюдателя при входе в Windows: Планировщик задач (нужны права)
# с фолбэком на папку автозагрузки (права не требуются).
$root = $PSScriptRoot
$pythonw = Join-Path $root ".venv\Scripts\pythonw.exe"
if (-not (Test-Path $pythonw)) { Write-Error "Сначала выполните setup.ps1"; exit 1 }

$ok = $false
try {
    # Две задачи: наблюдатель ФС и ОДИН общий MCP-сервер (streamable-http, :8787).
    # Клиенты (Cline/Hermes) ходят к нему по URL и НЕ запускают свои процессы.
    $tasks = @(
        @{ Name = "HermesDiskSearchWatch"; Args = "-m hds.cli watch" },
        @{ Name = "HermesDiskSearchMcp";   Args = "-m hds.cli mcp-http run" }
    )
    foreach ($t in $tasks) {
        $action   = New-ScheduledTaskAction -Execute $pythonw -Argument $t.Args -WorkingDirectory $root
        $trigger  = New-ScheduledTaskTrigger -AtLogOn
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
        Register-ScheduledTask -TaskName $t.Name -Action $action -Trigger $trigger -Settings $settings -Force -ErrorAction Stop | Out-Null
        Write-Host "[ok] Задача планировщика $($t.Name) создана (запуск при входе)"
        Write-Host "     Старт сейчас: Start-ScheduledTask -TaskName $($t.Name)"
    }
    $ok = $true
} catch {
    Write-Host "[--] Планировщик недоступен ($($_.Exception.Message.Trim())) — использую папку автозагрузки" -ForegroundColor Yellow
}

if (-not $ok) {
    $startup = [Environment]::GetFolderPath('Startup')
    $ws = New-Object -ComObject WScript.Shell
    foreach ($t in $tasks) {
        $lnk = $ws.CreateShortcut("$startup\$($t.Name).lnk")
        $lnk.TargetPath = $pythonw
        $lnk.Arguments = $t.Args
        $lnk.WorkingDirectory = $root
        $lnk.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
        $lnk.Save()
        Write-Host "[ok] Автозагрузка: $startup\$($t.Name).lnk"
    }
    Write-Host "Запуск при входе в систему; удалить — уберите ярлыки из папки автозагрузки."
}
Write-Host "Процессы в диспетчере задач: pythonw.exe x2 на каждый юнит (venv-лаунчер + реальный интерпретатор; аргументы: -m hds.cli watch и -m hds.cli mcp-http run)"