# install_llm_host_task.ps1 -- register the resident `llm-host` as a logon task (A6).
#
# NOTE: ASCII-only on purpose - Windows PowerShell 5.1 reads a .ps1 without BOM as
# ANSI and mangles non-ASCII text (tools/parity/README.md section 3, item 14).
#
# This is the A6 step "transfer ports 8010-8012 to the facade": the task must be
# registered only AFTER the Python roles (llama-server on :8010/:8011/:8012) are
# switched off, otherwise the resident cannot bind the ports and exits with a clear
# error. The script checks that and refuses to continue (use -Force to override).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_llm_host_task.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_llm_host_task.ps1 -Start
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_llm_host_task.ps1 -Status
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\install_llm_host_task.ps1 -Remove

param(
    [string]$Exe = "",
    [string]$Config = "",
    [switch]$Start,
    [switch]$Status,
    [switch]$Remove,
    [switch]$Force
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$taskName = "HermesDiskSearchLlmHost"

if ($Remove) {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Host "[ok] task removed: $taskName"
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
    $pidFile = Join-Path $root "data\llm-host.pid"
    if (Test-Path $pidFile) {
        Write-Host "[ok] pid file: $pidFile (pid $(Get-Content $pidFile -ErrorAction SilentlyContinue))"
    } else {
        Write-Host "[--] no pid file: $pidFile (resident is not running)"
    }
    $log = Join-Path $root "data\logs\llm-host.log"
    if (Test-Path $log) { Write-Host "[ok] log tail:"; Get-Content $log -Tail 10 }
    exit 0
}

if (-not $Exe) { $Exe = Join-Path $root "target\release\llm_host.exe" }
if (-not (Test-Path $Exe)) {
    Write-Error "llm-host binary not found: $Exe`nBuild it first: cargo build --release -p hds-llama --bin llm_host"
    exit 1
}

if (-not $Force) {
    $busy = @()
    foreach ($p in 8010, 8011, 8012) {
        $c = Get-NetTCPConnection -LocalPort $p -State Listen -ErrorAction SilentlyContinue
        if ($c) { $busy += $p }
    }
    if ($busy.Count -gt 0) {
        Write-Host "[!!] ports still busy: $($busy -join ', ')" -ForegroundColor Yellow
        Write-Host "     Python roles (llama-server) must be stopped first - the resident binds"
        Write-Host "     the same ports and will refuse to start otherwise."
        Write-Host "     Stop the roles, then re-run this script (or use -Force to skip the check)."
        exit 1
    }
}

$argList = "run"
if ($Config) { $argList = "$argList --config `"$Config`"" }

try {
    $action = New-ScheduledTaskAction -Execute $Exe -Argument $argList -WorkingDirectory $root
    $trigger = New-ScheduledTaskTrigger -AtLogOn
    # no execution time limit (resident process), restart on failure
    $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
    Write-Host "[ok] task registered: $taskName (start at logon)"
    Write-Host "     binary: $Exe $argList"
    Write-Host "     working dir: $root"
    Write-Host "     status: installers\install_llm_host_task.ps1 -Status"
    if ($Start) {
        Start-ScheduledTask -TaskName $taskName
        Write-Host "[ok] task started: $taskName"
    } else {
        Write-Host "     start now: Start-ScheduledTask -TaskName $taskName"
    }
} catch {
    Write-Host "[!!] scheduler is not available ($($_.Exception.Message.Trim()))" -ForegroundColor Yellow
    Write-Host "     register the resident manually (Task Scheduler or the Startup folder):"
    Write-Host "     $Exe $argList   (working dir: $root)"
    exit 1
}
