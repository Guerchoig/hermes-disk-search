# update.ps1 - replace the installed version with a new one, safely (W4 / MIGRATION_PLAN 10.6).
#
# A running process cannot replace itself, so the update is performed by this
# SEPARATE script: stop what holds the binaries -> install the new version ->
# switch app\current -> restart. Previous versions are kept under app\<ver>
# (rollback: point app\current back to the older directory with -RollbackTo).
#
# 0.20.3: after the switch the LAUNCH POINTS are re-registered. They can hold an
# ABSOLUTE path into the old layout: a flat install (`<root>\bin\hds.exe` - what a
# plain "unpack + install-server.cmd" produces) registered its SCM services there,
# and the desktop tasks/shortcut point at the folder they were created from. With-
# out this step the update would put the new version into app\<ver>, switch
# app\current and restart services that still run the OLD files, while -RollbackTo
# would switch a pointer nothing reads (incident 16.10.2026).
#   * server  - `sc config` per role via `<app\current>\bin\hds.exe service install`
#               (idempotent, also refreshes auto-restart) + re-pin the per-service
#               Environment (HDS_CONFIG and the machine paths);
#   * desktop - the logon tasks are recreated from app\current (they keep absolute
#               paths) and the desktop shortcut is refreshed from app\current.
# Already versioned installs point at app\current\bin and need nothing: the switch
# moves the junction.
#
# 0.20.7 (incident 20.10.2026): the SCM FAILURE ACTIONS are suspended before the
# roles are stopped and restored after the pointer switch (also on error - the
# finally block). A stopped role otherwise comes back in ~5 s, and while the new
# version is being copied that resurrected role re-creates app\current as a REAL
# directory (the heartbeat writer), so install_app_version.ps1 fails with
# "directory is not empty" and the update leaves the stand without services and
# the pointer on the old version. `install_server.ps1` never hits this: it does
# not stop anything (README_SERVER_MULTIUSER, "if the server is already running").
#
# Two profiles (M8 T4):
#   -Profile desktop  (default) - stop the AtLogOn Scheduled Tasks + the resident,
#                                 then restart the tasks (previous behaviour).
#   -Profile server             - stop the SCM services hermes-disk-search-<role>
#                                 (waiting for STOPPED), update, start them again
#                                 (order llm-host -> watch -> mcp-http -> ui) and
#                                 health-check. Requires Administrator.
#
# 0.21.0 (F3, review B3): after the pointer switch this script also TOP-UPS the
# Nemotron-3 diarization model (400 MB, pinned sha256) via the new version's
# installers\fetch_nemotron3_model.ps1 into HDS_MODELS_DIR / the machine models
# dir (server) / the engine models dir (desktop). Idempotent: an existing valid
# file is skipped. A fetch failure is NOT fatal - the update still succeeds,
# `hds check` prints the exact command and diarization jobs simply refuse until
# the model is installed. `-NoModels` skips the step (offline hosts).
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   installers\update.ps1 -From hds-0.2.0-windows-x64.zip -Version 0.2.0 -Root D:\hds
#   installers\update.ps1 -From dist\hds-0.3.0 -Version 0.3.0 -Root D:\hds -Profile server
#   installers\update.ps1 -Root D:\hds -ListVersions
#   installers\update.ps1 -Root D:\hds -RollbackTo 0.2.0 -Profile server
#   installers\update.ps1 -Root D:\hds -From dist\hds-0.21.0 -Version 0.21.0 -NoModels
param(
    [string]$From = "",
    [string]$Version = "",
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$Force,
    [ValidateSet("desktop", "server")][string]$Profile = "desktop",
    [string]$RollbackTo = "",
    [switch]$ListVersions,
    [string]$HealthUrl = "",
    [switch]$WhatIf,
    # F3: skip the Nemotron-3 diarization model top-up (offline hosts / manual).
    [switch]$NoModels
)
$ErrorActionPreference = "Continue"
$here = $PSScriptRoot
if (-not (Test-Path $Root)) { throw "root not found: $Root" }
$Root = (Resolve-Path $Root).Path
$cur = Join-Path $Root "app\current"
$SvcPrefix = "hermes-disk-search-"
# Stop order: dependents first, resident (GPU owner) last. `transcribe` depends on llm-host.
$ServerStopRoles = @("ui", "mcp-http", "watch", "transcribe", "llm-host")
# Start order: resident first, then the rest (`transcribe` after llm-host).
$ServerStartRoles = @("llm-host", "transcribe", "watch", "mcp-http", "ui")
$tasks = @("HermesDiskSearchLlmHost", "HermesDiskSearchWatch", "HermesDiskSearchMcp", "HermesDiskSearchUi")

# --- helpers ---------------------------------------------------------------

# State word from `sc query` (values are English even on localized Windows).
function Get-ScState([string]$name) {
    $o = (& sc.exe query $name 2>&1 | Out-String)
    if ($o -match '1060') { return 'absent' }
    if ($o -match 'RUNNING') { return 'running' }
    if ($o -match 'STOP_PENDING') { return 'stopping' }
    if ($o -match 'START_PENDING') { return 'starting' }
    if ($o -match 'STOPPED') { return 'stopped' }
    return 'other'
}

# Wait (bounded) until the service reaches the wanted state.
function Wait-ScState([string]$name, [string]$want, [int]$timeoutSec) {
    $deadline = (Get-Date).AddSeconds($timeoutSec)
    while ((Get-Date) -lt $deadline) {
        if ((Get-ScState $name) -eq $want) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return ((Get-ScState $name) -eq $want)
}

# --- auto-restart guard (incident 20.10.2026) --------------------------------
# Roles carry failure actions (restart in 5 s/5 s/30 s). They must be cleared
# BEFORE `sc.exe stop`: otherwise a role comes back during the copy window,
# re-creates app\current as a real directory and install_app_version.ps1 dies
# with "directory is not empty". Restore-Restart is idempotent and is called
# both on the happy path and from `finally`.
$script:RestartPaused = @()

function Suspend-Restart([string]$name) {
    # SCM failure actions are Windows-only; macOS launchd is a different mechanism.
    if ($env:OS -ne 'Windows_NT') { return }
    if ($WhatIf) { Write-Host "[..] (whatif) would suspend auto-restart of $name"; return }
    # The empty `actions= ""` argument must go through cmd.exe: PowerShell 5.1
    # does not pass an empty argument to a native program (README 5B).
    cmd /c ('sc.exe failure ' + $name + ' reset= 0 actions= ""') *> $null
    if ($LASTEXITCODE -eq 0) {
        if ($script:RestartPaused -notcontains $name) { $script:RestartPaused += $name }
        Write-Host "[ok] auto-restart suspended: $name"
    } else {
        Write-Host "[!!] could not suspend auto-restart: $name" -ForegroundColor Yellow
    }
}

function Restore-Restart {
    if ($env:OS -ne 'Windows_NT') { return }
    if ($WhatIf -or $script:RestartPaused.Count -eq 0) { return }
    foreach ($name in $script:RestartPaused) {
        & sc.exe failure $name reset= 86400 "actions= restart/5000/restart/5000/restart/30000" *> $null
        if ($LASTEXITCODE -eq 0) { Write-Host "[ok] auto-restart restored: $name" }
        else { Write-Host "[!!] could not restore auto-restart: $name" -ForegroundColor Yellow }
    }
    $script:RestartPaused = @()
}

function Stop-All {
    if ($Profile -eq "server") {
        foreach ($role in $ServerStopRoles) {
            $n = "$SvcPrefix$role"
            if ((Get-ScState $n) -eq 'absent') { Write-Host "[..] service not installed: $n"; continue }
            Suspend-Restart $n
            if ($WhatIf) { Write-Host "[..] (whatif) would stop $n"; continue }
            & sc.exe stop $n *> $null
            if (Wait-ScState $n 'stopped' 30) { Write-Host "[ok] stopped $n" } else { Write-Host "[!!] $n did not stop in 30s" -ForegroundColor Yellow }
        }
        return
    }
    foreach ($t in $tasks) {
        if (Get-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue) {
            if ($WhatIf) { Write-Host "[..] (whatif) would stop task $t"; continue }
            Stop-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue
            Write-Host "[..] stopped task $t"
        }
    }
}

function Start-All {
    if ($Profile -eq "server") {
        foreach ($role in $ServerStartRoles) {
            $n = "$SvcPrefix$role"
            if ((Get-ScState $n) -eq 'absent') { continue }
            if ($WhatIf) { Write-Host "[..] (whatif) would start $n"; continue }
            & sc.exe start $n *> $null
            if (Wait-ScState $n 'running' 60) { Write-Host "[ok] started $n" } else { Write-Host "[!!] $n not running in 60s" -ForegroundColor Yellow }
        }
        if (-not $WhatIf) {
            $url = $HealthUrl
            if (-not $url) { $url = "http://127.0.0.1:8787/health" }
            try {
                $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 10
                Write-Host "[ok] health $url -> $($r.StatusCode)"
            } catch {
                Write-Host "[!!] health check failed: $url ($($_.Exception.Message))" -ForegroundColor Yellow
            }
        }
        return
    }
    foreach ($t in $tasks) {
        if (Get-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue) {
            if ($WhatIf) { Write-Host "[..] (whatif) would start task $t"; continue }
            Start-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue
            Write-Host "[ok] started task $t"
        }
    }
}

function Switch-Current([string]$target) {
    $curLink = Join-Path $Root "app\current"
    if (Test-Path $curLink) { cmd /c rmdir "$curLink" | Out-Null }
    New-Item -ItemType Junction -Path $curLink -Target $target | Out-Null
    Write-Host "[ok] current -> $target"
}

# --- 0.20.3: launch-point re-registration (see the header) ------------------

# exe path from the service registration (`sc qc`); '' when the role is absent.
function Get-ScBinPath([string]$name) {
    $o = (& sc.exe qc $name 2>&1 | Out-String)
    if ($o -match '1060') { return '' }
    $m = [regex]::Match($o, '"([^"]*[hH][dD][sS]\.exe)"')
    if ($m.Success) { return $m.Groups[1].Value }
    return ''
}

# Per-service Environment: HDS_CONFIG + the machine paths (same set as
# install_server.ps1 pins; a freshly started service must not pick up a stale
# machine env cached by the SCM at boot).
function Pin-ServiceEnv([string]$role, [string]$cfg) {
    $svc = "$SvcPrefix$role"
    $pairs = @("HDS_CONFIG=$cfg")
    foreach ($v in @("LLAMA_RUNTIME_DIR", "HDS_ENGINE_DIR", "HDS_EXTRACT_PYTHON", "HDS_MODELS_DIR")) {
        $val = [Environment]::GetEnvironmentVariable($v, "Machine")
        if ($val) { $pairs += "$v=$val" }
    }
    reg.exe add "HKLM\SYSTEM\CurrentControlSet\Services\$svc" /v Environment /t REG_MULTI_SZ /d ($pairs -join '\0') /f *> $null
    return ($LASTEXITCODE -eq 0)
}

# Re-register the launch points on the NEW version (called after Switch-Current).
function Repoint-Layout {
    $isWin = ($env:OS -eq 'Windows_NT')
    $newExe = if ($isWin) { Join-Path $cur "bin\hds.exe" } else { Join-Path $cur "bin/hds" }
    if (-not (Test-Path $newExe)) {
        Write-Host "[..] re-point: no binary under $cur (bin) - skipped"
        return
    }
    if ($Profile -eq "server") {
        if (-not $isWin) {
            # macOS: the LaunchDaemon plist keeps an absolute path as well.
            foreach ($role in $ServerStartRoles) {
                $plist = "/Library/LaunchDaemons/com.hermes-disk-search.$role.plist"
                if (-not (Test-Path $plist)) { continue }
                if ((Get-Content $plist -Raw) -like "*$cur*") { Write-Host "[ok] $role already runs $cur"; continue }
                if ($WhatIf) { Write-Host "[..] (whatif) would re-point $role (launchd) -> $newExe"; continue }
                & $newExe service install $role *> $null
                if ($LASTEXITCODE -eq 0) { Write-Host "[ok] re-pointed $role (launchd)" } else { Write-Host "[!!] re-point failed for $role" -ForegroundColor Yellow }
            }
            return
        }
        $cfg = Join-Path $env:ProgramData "hermes-disk-search\config.yaml"
        if (-not (Test-Path $cfg)) { $cfg = "" }
        foreach ($role in $ServerStartRoles) {
            $n = "$SvcPrefix$role"
            if ((Get-ScState $n) -eq 'absent') { continue }
            $bp = Get-ScBinPath $n
            if (-not $bp) { Write-Host "[..] $n : binPath not parsed - skipped"; continue }
            if ($bp -eq $newExe) { Write-Host "[ok] $n already runs $newExe"; continue }
            if ($WhatIf) { Write-Host "[..] (whatif) would re-point $n : $bp -> $newExe"; continue }
            & $newExe service install $role *> $null
            if ($LASTEXITCODE -ne 0) {
                Write-Host "[!!] re-point failed for $n (exit $LASTEXITCODE)" -ForegroundColor Yellow
                continue
            }
            Write-Host "[ok] re-pointed $n -> $newExe"
            if ($cfg) {
                if (Pin-ServiceEnv $role $cfg) { Write-Host "[ok]   env re-pinned for $n" } else { Write-Host "[!!]   could not re-pin env for $n" -ForegroundColor Yellow }
            }
        }
        return
    }
    if (-not $isWin) { return }   # desktop = logon tasks + shortcut (Windows only)
    # desktop: logon tasks and the desktop shortcut hold ABSOLUTE paths
    $haveTasks = $false
    foreach ($t in @("HermesDiskSearchUi", "HermesDiskSearchLlmHost", "HermesDiskSearchWatch", "HermesDiskSearchMcp", "HermesDiskSearchTranscribe")) {
        if (Get-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue) { $haveTasks = $true }
    }
    if ($haveTasks) {
        foreach ($s in @("installers\install_ui_task.ps1", "installers\install_llm_host_task.ps1", "install_autostart.ps1")) {
            $p = Join-Path $cur $s
            if (-not (Test-Path $p)) { continue }
            if ($WhatIf) { Write-Host "[..] (whatif) would re-register the logon tasks: $p"; continue }
            & powershell -NoProfile -ExecutionPolicy Bypass -File $p *> $null
            if ($LASTEXITCODE -eq 0) {
                Write-Host "[ok] logon tasks re-registered: $s"
            } else {
                # install_llm_host_task.ps1 refuses while ports 8010-8012 are busy
                & powershell -NoProfile -ExecutionPolicy Bypass -File $p -Force *> $null
                if ($LASTEXITCODE -eq 0) { Write-Host "[ok] logon tasks re-registered (-Force): $s" } else { Write-Host "[!!] task re-registration failed: $s" -ForegroundColor Yellow }
            }
        }
    }
    $lnk = Join-Path ([Environment]::GetFolderPath('Desktop')) "Hermes Disk Search.lnk"
    $sc = Join-Path $cur "shortcuts\windows\create_shortcut.ps1"
    if ((Test-Path $lnk) -and (Test-Path $sc)) {
        if ($WhatIf) {
            Write-Host "[..] (whatif) would refresh the desktop shortcut from $cur"
        } else {
            & powershell -NoProfile -ExecutionPolicy Bypass -File $sc *> $null
            Write-Host "[ok] desktop shortcut refreshed ($cur)"
        }
    }
}

# --- informational mode: list installed versions ---------------------------
if ($ListVersions) {
    $appDir = Join-Path $Root "app"
    if (-not (Test-Path $appDir)) {
        Write-Host "[..] no app\ directory under $Root"
        if (Test-Path (Join-Path $Root "bin\hds.exe")) {
            Write-Host "[--] flat layout: no version is kept yet. The first update creates app\<ver> and (0.20.3+) re-points the roles to app\current\bin itself." -ForegroundColor Yellow
            Write-Host "     To keep versions from the start: unpack the archive elsewhere and pass -Root <this folder>."
        }
        exit 0
    }
    $curTarget = ""
    if (Test-Path $cur) { $curTarget = [string](Get-Item $cur).Target }
    foreach ($d in (Get-ChildItem $appDir -Directory -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne 'current' })) {
        $mark = if ($curTarget -and ($curTarget -like "*$($d.Name)*")) { "*" } else { " " }
        Write-Host ("{0} {1}" -f $mark, $d.Name)
    }
    exit 0
}

# --- validation ------------------------------------------------------------
$doRollback = [bool]$RollbackTo
if (-not $doRollback) {
    if (-not $From) { throw "-From is required (or use -RollbackTo / -ListVersions)" }
    if (-not $Version) { throw "-Version is required (or use -RollbackTo / -ListVersions)" }
}

Write-Host "=== update.ps1: profile=$Profile root=$Root ==="
if ($WhatIf) { Write-Host "[..] (whatif) no changes will be made" }

# --- rollback: switch app\current back to an existing version --------------
if ($doRollback) {
    $target = Join-Path $Root "app\$RollbackTo"
    if (-not (Test-Path $target)) { throw "rollback target not found: $target" }
    Write-Host "[..] rollback to $RollbackTo"
    try {
        Stop-All
        if ($WhatIf) { Write-Host "[..] (whatif) would switch current -> $target" } else { Switch-Current $target }
        Repoint-Layout
        Restore-Restart
        Start-All
        Write-Host "[ok] rollback complete: $RollbackTo"
    } finally {
        Restore-Restart
    }
    exit 0
}

# --- 1. stop everything that may hold the binaries -------------------------
try {
    Stop-All
    $llmHost = Join-Path $cur "bin\llm_host.exe"
    if ((Test-Path $llmHost) -and -not $WhatIf) {
        Write-Host "[..] graceful stop of the resident"
        try { & $llmHost stop *> $null } catch { }
        Start-Sleep -Seconds 2
    }

    # --- 2. install the new version and switch the pointer (separate process) ---
    if ($WhatIf) {
        Write-Host "[..] (whatif) would run: install_app_version.ps1 -From $From -Version $Version -SetCurrent"
    } else {
        # Build the argument list explicitly: `-Force:$Force` would be passed to the
        # external powershell.exe as a STRING ("-Force:False") and fail to bind to the
        # [switch] parameter (pre-existing bug fixed in M8 T4).
        $iaArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass",
            "-File", (Join-Path $here "install_app_version.ps1"),
            "-From", $From, "-Version", $Version, "-Root", $Root, "-SetCurrent")
        if ($Force) { $iaArgs += "-Force" }
        & powershell @iaArgs
        if ($LASTEXITCODE -ne 0) { throw "install_app_version.ps1 failed (exit $LASTEXITCODE)" }
    }

    # --- 3. Nemotron-3 model top-up (F3, B3) --------------------------------
    # Before: an updated install got `diarization.backend: nemotron3` (product
    # default) but no model file, so every auto-transcription job failed and was
    # blacklisted. The fetch runs AFTER the pointer switch (app\current is the new
    # version) and is NOT fatal: without the model the daemon refuses jobs and
    # `hds check` prints the exact command, so a manual run fixes it later.
    if (-not $WhatIf -and -not $NoModels) {
        $fetch = Join-Path $cur "installers\fetch_nemotron3_model.ps1"
        if (Test-Path $fetch) {
            $modelsDir = $env:HDS_MODELS_DIR
            if (-not $modelsDir) {
                if ($Profile -eq "server") {
                    $modelsDir = Join-Path $env:ProgramData "hermes-disk-search\models"
                } else {
                    $modelsDir = Join-Path $env:APPDATA "OpenResearchTools\models"
                }
            }
            Write-Host "[..] Nemotron-3 diarization model -> $modelsDir (idempotent)"
            try {
                & powershell -NoProfile -ExecutionPolicy Bypass -File $fetch -ModelsDir $modelsDir
                if ($LASTEXITCODE -ne 0) {
                    Write-Host "[!!] Nemotron-3 model NOT fetched (exit $LASTEXITCODE): diarization jobs fail until it is installed - run: $fetch" -ForegroundColor Yellow
                } else {
                    Write-Host "[ok] Nemotron-3 diarization model ready"
                }
            } catch {
                Write-Host "[!!] Nemotron-3 model fetch failed: $_ - run manually: $fetch" -ForegroundColor Yellow
            }
        } else {
            Write-Host "[..] fetch_nemotron3_model.ps1 not found in $cur - model step skipped"
        }
    } elseif ($WhatIf -and -not $NoModels) {
        Write-Host "[..] (whatif) would run: installers\fetch_nemotron3_model.ps1 -ModelsDir <models dir>"
    } else {
        Write-Host "[..] -NoModels: Nemotron-3 diarization model step skipped"
    }

    # --- 4. launch points + restart -----------------------------------------
    Repoint-Layout
    Restore-Restart
    Start-All
    Write-Host "[ok] update complete: $Version"
} finally {
    # An aborted update must not leave the roles without auto-restart.
    Restore-Restart
}
