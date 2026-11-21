# install_agent_sync_task.ps1 - register the agent mirror daemon as a logon task (A6.2).
#
# The task runs `hds agent sync --all` at logon: ONE process for every project in
# data\agent\projects.json. Each project keeps its own sync.lock, journal, registry
# and workers (AGENT_A6_STAGES.md A6.2, owner decision B-A6-2 in the plan). Without
# --once the command is a daemon with agent.sync.interval_sec; starting it twice is
# harmless - every project whose sync.lock is taken is skipped ("already running"),
# so the logon task and a manual run never conflict.
#
# Fallback when the Task Scheduler is not available: a Startup-folder shortcut
# through wscript.exe + hidden_launch.vbs (no console window, no admin rights).
#
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
# The E2E stand registers the task under a unique -TaskName and removes it in finally.
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_agent_sync_task.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_agent_sync_task.ps1 -Start
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_agent_sync_task.ps1 -Status
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_agent_sync_task.ps1 -Remove
param(
    [string]$TaskName = "HermesDiskSearchAgentSync",
    [switch]$Start,
    [switch]$Status,
    [switch]$Remove,
    [switch]$StartupFolder
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$startup = [Environment]::GetFolderPath('Startup')
$startupLnk = Join-Path $startup "$TaskName.lnk"
# --all makes the command a daemon (interval from agent.sync.interval_sec);
# --once would exit right after one pass, which is useless for autostart.
$argList = "agent sync --all"

function Show-DaemonHint {
    Write-Host "     daemon state (sync.lock, sync.log, pending): hds agent check"
}

if ($Remove) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $startupLnk) { Remove-Item -LiteralPath $startupLnk -Force -ErrorAction SilentlyContinue }
    Write-Host "[ok] removed: $TaskName (scheduled task and/or Startup shortcut)"
    exit 0
}

if ($Status) {
    $t = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($t) {
        Write-Host "[ok] task exists: $TaskName (state: $($t.State))"
        $t.Actions | ForEach-Object { Write-Host "     action: $($_.Execute) $($_.Arguments)" }
    } else {
        Write-Host "[--] task is not registered: $TaskName"
    }
    if (Test-Path -LiteralPath $startupLnk) {
        Write-Host "[ok] startup shortcut: $startupLnk"
    } else {
        Write-Host "[--] no startup shortcut: $startupLnk"
    }
    Show-DaemonHint
    exit 0
}

. (Join-Path $root 'hds_bin.ps1')
# bin\ (packaged release) or target\{release,debug}\ (source checkout).
$hdsExe = Get-HdsBinPath -Root $root -Name "hds.exe"
if (-not $hdsExe) {
    Write-Host "[!!] hds.exe not found. Looked in: $(Get-HdsBinHint 'hds.exe')" -ForegroundColor Red
    Write-Host "     Run setup.cmd (release) or build it: cargo build --release -p hds-cli" -ForegroundColor Red
    exit 1
}


$ok = $false
if (-not $StartupFolder) {
    try {
        $action = New-ScheduledTaskAction -Execute $hdsExe -Argument $argList -WorkingDirectory $root
        $trigger = New-ScheduledTaskTrigger -AtLogOn
        # the mirror daemon runs for the whole session: no time limit, restart on failure
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -Force -ErrorAction Stop | Out-Null
        # mutual exclusion: with a scheduled task in place, drop a leftover Startup shortcut
        if (Test-Path -LiteralPath $startupLnk) { Remove-Item -LiteralPath $startupLnk -Force -ErrorAction SilentlyContinue }
        Write-Host "[ok] task registered: $TaskName (start at logon)"
        Write-Host "     binary: $hdsExe $argList"
        Write-Host "     working dir: $root"
        Write-Host "     status: installers\install_agent_sync_task.ps1 -Status"
        if ($Start) {
            Start-ScheduledTask -TaskName $TaskName
            Write-Host "[ok] task started: $TaskName"
        } else {
            Write-Host "     start now: Start-ScheduledTask -TaskName $TaskName"
        }
        Show-DaemonHint
        $ok = $true
    } catch {
        Write-Host "[--] scheduler is not available ($($_.Exception.Message.Trim())) - using the Startup folder" -ForegroundColor Yellow
    }
} else {
    Write-Host "[..] -StartupFolder: skipping the scheduler, writing the Startup shortcut" -ForegroundColor DarkGray
}
if (-not $ok) {
    # mutual exclusion: with a Startup shortcut in place, drop a leftover scheduled task
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    # wscript.exe + hidden_launch.vbs: starts the console binary with no window
    $vbs = Join-Path $root 'hidden_launch.vbs'
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($startupLnk)
    $sc.TargetPath = Join-Path $env:SystemRoot 'System32\wscript.exe'
    $sc.Arguments = "`"$vbs`" `"$hdsExe`"" + (($argList -split '\s+' | ForEach-Object { " `"$_`"" }) -join '')
    $sc.WorkingDirectory = $root
    $sc.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
    $sc.Description = "hermes-disk-search: agent mirror daemon (hds agent sync --all)"
    $sc.Save()
    Write-Host "[ok] startup shortcut: $startupLnk"
    Write-Host "     it starts the mirror daemon at logon (no admin rights needed);"
    Write-Host "     remove it with: installers\install_agent_sync_task.ps1 -Remove"
    if ($Start) {
        Write-Host "     start now: $hdsExe $argList"
    } else {
        Write-Host "     start now: Start-Process -FilePath '$hdsExe' -ArgumentList '$argList' -WorkingDirectory '$root' -WindowStyle Hidden"
    }
    Show-DaemonHint
}
