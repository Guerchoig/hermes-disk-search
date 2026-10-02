# install_ui_task.ps1 - register the web UI server as a logon task (W4, optional).
#
# Keeps the UI available right after logon. The desktop shortcut (run_ui.ps1) still
# starts it on demand and reuses an already running server, so this task is optional.
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_ui_task.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_ui_task.ps1 -Status
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_ui_task.ps1 -Remove
param(
    [int]$Port = 8765,
    [switch]$Status,
    [switch]$Remove
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$taskName = "HermesDiskSearchUi"
$hdsExe = Join-Path $root "bin\hds.exe"
$startupDir = [Environment]::GetFolderPath('Startup')

if ($Remove) {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    $lnk = Join-Path $startupDir "$taskName.lnk"
    if (Test-Path $lnk) { Remove-Item $lnk -Force -ErrorAction SilentlyContinue }
    Write-Host "[ok] $taskName removed"
    exit 0
}
if ($Status) {
    $t = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    if ($t) {
        Write-Host "[ok] task exists: $taskName (state: $($t.State))"
        $t.Actions | ForEach-Object { Write-Host "     action: $($_.Execute) $($_.Arguments)" }
    } else {
        Write-Host "[--] task is not registered: $taskName"
    }
    exit 0
}
if (-not (Test-Path $hdsExe)) {
    Write-Host "[!!] bin\hds.exe not found - run setup.cmd first" -ForegroundColor Red
    exit 1
}

$argList = "ui --port $Port"
try {
    $action = New-ScheduledTaskAction -Execute $hdsExe -Argument $argList -WorkingDirectory $root
    $trigger = New-ScheduledTaskTrigger -AtLogOn
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
    Write-Host "[ok] task registered: $taskName (bin\hds.exe $argList)"
} catch {
    Write-Host "[--] scheduler unavailable ($($_.Exception.Message.Trim())) - using the Startup folder" -ForegroundColor Yellow
    $lnk = Join-Path $startupDir "$taskName.lnk"
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($lnk)
    $sc.TargetPath = $hdsExe
    $sc.Arguments = $argList
    $sc.WorkingDirectory = $root
    $sc.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
    $sc.Save()
    Write-Host "[ok] startup shortcut: $lnk"
}
