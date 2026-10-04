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
    [switch]$Force,
    [switch]$StartupFolder
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$taskName = "HermesDiskSearchLlmHost"
$startup = [Environment]::GetFolderPath('Startup')
$startupLnk = Join-Path $startup "$taskName.lnk"

if ($Remove) {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $startupLnk) { Remove-Item -LiteralPath $startupLnk -Force -ErrorAction SilentlyContinue }
    Write-Host "[ok] removed: $taskName (scheduled task and/or Startup shortcut)"
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

if (-not $Exe) {
    # Prefer the binary of OUR running resident: it is the build that already owns
    # ports 8010-8012, so autostart keeps using the exact same deployment. This also
    # avoids a newer debug build silently replacing a release resident.
    $residentPidFile = Join-Path $root "data\llm-host.pid"
    if (Test-Path -LiteralPath $residentPidFile) {
        $residentPid = (Get-Content -LiteralPath $residentPidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
        if ($residentPid -match '^\d+$') {
            $proc = Get-Process -Id ([int]$residentPid) -ErrorAction SilentlyContinue
            if ($proc -and $proc.Path -and (Test-Path -LiteralPath $proc.Path)) { $Exe = $proc.Path }
        }
    }
    if (-not $Exe) {
        . (Join-Path $root 'hds_bin.ps1')
        # bin\ (packaged release) or target\{release,debug}\ (source checkout).
        $Exe = Get-HdsBinPath -Root $root -Name "llm_host.exe"
    }
}
if (-not $Exe -or -not (Test-Path -LiteralPath $Exe)) {
    Write-Error "llm-host binary not found (looked in bin\llm_host.exe, target\release\llm_host.exe, target\debug\llm_host.exe)`nBuild the release first: installers\build_rust_release.ps1 (or cargo build --release -p hds-llama --bin llm_host for a dev tree)"
    exit 1
}
Write-Host "[..] llm-host binary: $Exe"

if (-not $Force) {
    # If OUR resident already serves the ports (pid file + /health), this is not the
    # Python stack: registering the task is safe (it will restart the same binary at
    # the next logon). Otherwise check whether a foreign process holds the ports.
    $ourPidFile = Join-Path $root "data\llm-host.pid"
    $weOwn = $false
    if (Test-Path $ourPidFile) {
        try {
            $r = Invoke-RestMethod -Uri "http://127.0.0.1:8010/health" -TimeoutSec 3
            $weOwn = ($r.status -eq "ok")
        } catch { $weOwn = $false }
    }
    if ($weOwn) {
        Write-Host "[ok] ports 8010-8012 are already served by llm-host (pid $(Get-Content $ourPidFile))"
        Write-Host "     registration is safe: the same binary owns the ports"
    } else {
        $busy = @()
        foreach ($p in 8010, 8011, 8012) {
            $c = Get-NetTCPConnection -LocalPort $p -State Listen -ErrorAction SilentlyContinue
            if ($c) { $busy += $p }
        }
        if ($busy.Count -gt 0) {
            Write-Host "[!!] ports are busy with a foreign process: $($busy -join ', ')" -ForegroundColor Yellow
            Write-Host "     Python roles (llama-server) must be stopped first - the resident binds"
            Write-Host "     the same ports and will refuse to start otherwise."
            Write-Host "     Stop the roles, then re-run this script (or use -Force to skip the check)."
            exit 1
        }
    }
}

$argList = "run"
if ($Config) { $argList = "$argList --config `"$Config`"" }

$ok = $false
if (-not $StartupFolder) {
    try {
        $action = New-ScheduledTaskAction -Execute $Exe -Argument $argList -WorkingDirectory $root
        $trigger = New-ScheduledTaskTrigger -AtLogOn
        # no execution time limit (resident process), restart on failure
        $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
        Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Force -ErrorAction Stop | Out-Null
        # mutual exclusion: with a scheduled task in place, drop a leftover Startup shortcut
        if (Test-Path -LiteralPath $startupLnk) { Remove-Item -LiteralPath $startupLnk -Force -ErrorAction SilentlyContinue }
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
        $ok = $true
    } catch {
        Write-Host "[--] scheduler is not available ($($_.Exception.Message.Trim())) - using the Startup folder" -ForegroundColor Yellow
    }
} else {
    Write-Host "[..] -StartupFolder: skipping the scheduler, writing the Startup shortcut" -ForegroundColor DarkGray
}
if (-not $ok) {
    # mutual exclusion: with a Startup shortcut in place, drop a leftover scheduled task
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    # wscript.exe + hidden_launch.vbs: starts the console binary with no window
    $vbs = Join-Path $root 'hidden_launch.vbs'
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($startupLnk)
    $sc.TargetPath = Join-Path $env:SystemRoot 'System32\wscript.exe'
    $sc.Arguments = "`"$vbs`" `"$Exe`"" + (($argList -split '\s+' | ForEach-Object { " `"$_`"" }) -join '')
    $sc.WorkingDirectory = $root
    $sc.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
    $sc.Description = "hermes-disk-search: llm-host (Rust GPU owner, ports 8010-8012)"
    $sc.Save()
    Write-Host "[ok] startup shortcut: $startupLnk"
    Write-Host "     it starts the resident at logon (no admin rights needed);"
    Write-Host "     remove it with: Remove-Item '$startupLnk'"
    if ($Start) {
        Write-Host "     start now: $Exe $argList"
    } else {
        Write-Host "     start now: Start-Process -FilePath '$Exe' -ArgumentList '$argList' -WorkingDirectory '$root'"
    }
}
