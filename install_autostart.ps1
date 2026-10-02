# Logon autostart for the Rust stack: file watcher + one shared MCP server.
# Scheduler tasks need rights; on failure the script falls back to the Startup
# folder (no admin rights). llm-host is registered separately by
# installers\install_llm_host_task.ps1 (called from setup.ps1).
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
$root = $PSScriptRoot
$hdsExe = Join-Path $root "bin\hds.exe"
if (-not (Test-Path $hdsExe)) {
    Write-Host "[!!] bin\hds.exe not found - run setup.cmd first" -ForegroundColor Red
    exit 1
}

$startupDir = [Environment]::GetFolderPath('Startup')
# remove legacy Python tasks/shortcuts so that two watchers never run together
foreach ($name in @("HermesDiskSearchWatch", "HermesDiskSearchMcp")) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
    $old = Join-Path $startupDir "$name.lnk"
    if (Test-Path $old) { Remove-Item $old -Force -ErrorAction SilentlyContinue }
}

$tasks = @(
    @{ Name = "HermesDiskSearchWatch"; Args = "watch" },
    @{ Name = "HermesDiskSearchMcp";   Args = "mcp-http run" }
)
$ok = $false
try {
    foreach ($t in $tasks) {
        $action   = New-ScheduledTaskAction -Execute $hdsExe -Argument $t.Args -WorkingDirectory $root
        $trigger  = New-ScheduledTaskTrigger -AtLogOn
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
        Register-ScheduledTask -TaskName $t.Name -Action $action -Trigger $trigger -Settings $settings -Force -ErrorAction Stop | Out-Null
        Write-Host "[ok] scheduled task $($t.Name) created (starts at logon)"
        Write-Host "     start now: Start-ScheduledTask -TaskName $($t.Name)"
    }
    $ok = $true
} catch {
    Write-Host "[--] scheduler unavailable ($($_.Exception.Message.Trim())) - using the Startup folder" -ForegroundColor Yellow
}
if (-not $ok) {
    $ws = New-Object -ComObject WScript.Shell
    foreach ($t in $tasks) {
        $lnk = $ws.CreateShortcut("$startupDir\$($t.Name).lnk")
        $lnk.TargetPath = $hdsExe
        $lnk.Arguments = $t.Args
        $lnk.WorkingDirectory = $root
        $lnk.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
        $lnk.Save()
        Write-Host "[ok] startup shortcut: $startupDir\$($t.Name).lnk"
    }
}
Write-Host "Tasks: HermesDiskSearchWatch (hds.exe watch) and HermesDiskSearchMcp (hds.exe mcp-http run)."
