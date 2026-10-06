# Logon autostart for the Rust stack: file watcher + one shared MCP server +
# the auto-transcription daemon (PLAN_AUTO_TRANSCRIBE).
# Scheduler tasks need rights; on failure the script falls back to the Startup
# folder (no admin rights). llm-host is registered separately by
# installers\install_llm_host_task.ps1 (called from setup.ps1).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File install_autostart.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File install_autostart.ps1 -StartupFolder
# (-StartupFolder skips the scheduler and always writes Startup-folder shortcuts.)
#
# Migration: legacy Python autostart entries (scheduled tasks / Startup shortcuts
# named HermesDiskSearchWatch|HermesDiskSearchMcp pointing at pythonw.exe -m hds.cli)
# are removed and replaced with the Rust binary resolved by hds_bin.ps1.
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [switch]$StartupFolder
)
$root = $PSScriptRoot
. (Join-Path $root 'hds_bin.ps1')
# bin\ (packaged release) or target\{release,debug}\ (source checkout).
$hdsExe = Get-HdsBinPath -Root $root -Name "hds.exe"
if (-not $hdsExe) {
    Write-Host "[!!] hds.exe not found. Looked in: $(Get-HdsBinHint 'hds.exe')" -ForegroundColor Red
    Write-Host "     Run setup.cmd (release) or build it: cargo build --release -p hds-cli" -ForegroundColor Red
    exit 1
}
Write-Host "[..] autostart binary: $hdsExe"

$startupDir = [Environment]::GetFolderPath('Startup')
# remove legacy Python tasks/shortcuts so that two watchers never run together
foreach ($name in @("HermesDiskSearchWatch", "HermesDiskSearchMcp")) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
    $old = Join-Path $startupDir "$name.lnk"
    if (Test-Path $old) { Remove-Item $old -Force -ErrorAction SilentlyContinue }
}

$tasks = @(
    @{ Name = "HermesDiskSearchWatch"; Args = "watch" },
    # mcp-http start (not run): the manager writes data\mcp_http.pid and exits,
    # so the UI card "MCP server" can stop/restart the instance later. The spawned
    # child is detached and survives the task exit; a live instance on the port
    # is reused, so a duplicate cannot appear.
    @{ Name = "HermesDiskSearchMcp";   Args = "mcp-http start" },
    # Auto-transcription daemon (PLAN_AUTO_TRANSCRIBE, variant A): inbox_dir -> out_dir.
    # Exits immediately (code 0) while auto_transcribe.enabled is false, so the task
    # is harmless until the feature is switched on in config.yaml.
    @{ Name = "HermesDiskSearchTranscribe"; Args = "transcribe-watch" }
)
$ok = $false
$created = @()
if (-not $StartupFolder) {
    try {
        foreach ($t in $tasks) {
            $action   = New-ScheduledTaskAction -Execute $hdsExe -Argument $t.Args -WorkingDirectory $root
            $trigger  = New-ScheduledTaskTrigger -AtLogOn
            $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
                -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
            Register-ScheduledTask -TaskName $t.Name -Action $action -Trigger $trigger -Settings $settings -Force -ErrorAction Stop | Out-Null
            $created += $t.Name
            Write-Host "[ok] scheduled task $($t.Name) created (starts at logon)"
            Write-Host "     start now: Start-ScheduledTask -TaskName $($t.Name)"
        }
        $ok = $true
    } catch {
        Write-Host "[--] scheduler unavailable ($($_.Exception.Message.Trim())) - using the Startup folder" -ForegroundColor Yellow
        # never leave a half-registered set (e.g. Watch task + Mcp shortcut): drop what
        # was created in THIS run, then fall back to the Startup folder for both roles.
        foreach ($n in $created) {
            Unregister-ScheduledTask -TaskName $n -Confirm:$false -ErrorAction SilentlyContinue
        }
    }
} else {
    Write-Host "[..] -StartupFolder: skipping the scheduler, writing Startup-folder shortcuts" -ForegroundColor DarkGray
}
if (-not $ok) {
    # wscript.exe + hidden_launch.vbs: starts the console binary with no window
    $vbs = Join-Path $root 'hidden_launch.vbs'
    $ws = New-Object -ComObject WScript.Shell
    foreach ($t in $tasks) {
        $lnkArgs = "`"$vbs`" `"$hdsExe`"" + (($t.Args -split '\s+' | ForEach-Object { " `"$_`"" }) -join '')
        $lnk = $ws.CreateShortcut("$startupDir\$($t.Name).lnk")
        $lnk.TargetPath = Join-Path $env:SystemRoot 'System32\wscript.exe'
        $lnk.Arguments = $lnkArgs
        $lnk.WorkingDirectory = $root
        $lnk.IconLocation = (Join-Path $root 'assets\icon.ico') + ',0'
        $lnk.Save()
        Write-Host "[ok] startup shortcut (hidden): $startupDir\$($t.Name).lnk"
    }
}
Write-Host "Autostart: HermesDiskSearchWatch ($hdsExe watch), HermesDiskSearchMcp ($hdsExe mcp-http start) and HermesDiskSearchTranscribe ($hdsExe transcribe-watch)."
