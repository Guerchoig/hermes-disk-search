# update.ps1 - replace the installed version with a new one, safely (W4 / MIGRATION_PLAN 10.6).
#
# A running process cannot replace itself, so the update is performed by this
# SEPARATE script: stop what holds the binaries -> install the new version ->
# switch app\current -> restart. Previous versions are kept under app\<ver>
# (rollback: point app\current back to the older directory with -RollbackTo).
#
# Two profiles (M8 T4):
#   -Profile desktop  (default) - stop the AtLogOn Scheduled Tasks + the resident,
#                                 then restart the tasks (previous behaviour).
#   -Profile server             - stop the SCM services hermes-disk-search-<role>
#                                 (waiting for STOPPED), update, start them again
#                                 (order llm-host -> watch -> mcp-http -> ui) and
#                                 health-check. Requires Administrator.
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   installers\update.ps1 -From hds-0.2.0-windows-x64.zip -Version 0.2.0 -Root D:\hds
#   installers\update.ps1 -From dist\hds-0.3.0 -Version 0.3.0 -Root D:\hds -Profile server
#   installers\update.ps1 -Root D:\hds -ListVersions
#   installers\update.ps1 -Root D:\hds -RollbackTo 0.2.0 -Profile server
param(
    [string]$From = "",
    [string]$Version = "",
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$Force,
    [ValidateSet("desktop", "server")][string]$Profile = "desktop",
    [string]$RollbackTo = "",
    [switch]$ListVersions,
    [string]$HealthUrl = "",
    [switch]$WhatIf
)
$ErrorActionPreference = "Continue"
$here = $PSScriptRoot
if (-not (Test-Path $Root)) { throw "root not found: $Root" }
$Root = (Resolve-Path $Root).Path
$cur = Join-Path $Root "app\current"
$SvcPrefix = "hermes-disk-search-"
# Stop order: dependents first, resident (GPU owner) last.
$ServerStopRoles = @("ui", "mcp-http", "watch", "llm-host")
# Start order: resident first, then the rest.
$ServerStartRoles = @("llm-host", "watch", "mcp-http", "ui")
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

function Stop-All {
    if ($Profile -eq "server") {
        foreach ($role in $ServerStopRoles) {
            $n = "$SvcPrefix$role"
            if ((Get-ScState $n) -eq 'absent') { Write-Host "[..] service not installed: $n"; continue }
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

# --- informational mode: list installed versions ---------------------------
if ($ListVersions) {
    $appDir = Join-Path $Root "app"
    if (-not (Test-Path $appDir)) { Write-Host "[..] no app\ directory under $Root"; exit 0 }
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
    Stop-All
    if ($WhatIf) { Write-Host "[..] (whatif) would switch current -> $target" } else { Switch-Current $target }
    Start-All
    Write-Host "[ok] rollback complete: $RollbackTo"
    exit 0
}

# --- 1. stop everything that may hold the binaries -------------------------
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

# --- 3. restart ------------------------------------------------------------
Start-All
Write-Host "[ok] update complete: $Version"
