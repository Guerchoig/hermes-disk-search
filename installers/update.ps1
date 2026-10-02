# update.ps1 - replace the installed version with a new one, safely (W4 / MIGRATION_PLAN 10.6).
#
# A running process cannot replace itself, so the update is performed by this
# SEPARATE script: stop the resident/tasks -> install the new version -> switch
# app\current -> restart the tasks. Previous versions are kept (rollback: point
# app\current back to the older directory).
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Example:
#   installers\update.ps1 -From hds-0.2.0-windows-x64.zip -Version 0.2.0 -Root D:\hds
param(
    [Parameter(Mandatory = $true)][string]$From,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$Force
)
$ErrorActionPreference = "Continue"
$here = $PSScriptRoot
$Root = (Resolve-Path $Root).Path
$cur = Join-Path $Root "app\current"
$tasks = @("HermesDiskSearchLlmHost", "HermesDiskSearchWatch", "HermesDiskSearchMcp", "HermesDiskSearchUi")

# 1. stop everything that may hold the binaries
foreach ($t in $tasks) {
    if (Get-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue
        Write-Host "[..] stopped task $t"
    }
}
$llmHost = Join-Path $cur "bin\llm_host.exe"
if (Test-Path $llmHost) {
    Write-Host "[..] graceful stop of the resident"
    & $llmHost stop 2>$null | Out-Null
    Start-Sleep -Seconds 2
}

# 2. install the new version and switch the pointer (separate process)
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $here "install_app_version.ps1") `
    -From $From -Version $Version -Root $Root -SetCurrent -Force:$Force
if ($LASTEXITCODE -ne 0) { throw "install_app_version.ps1 failed (exit $LASTEXITCODE)" }

# 3. restart the tasks (only those that were registered)
foreach ($t in $tasks) {
    if (Get-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue) {
        Start-ScheduledTask -TaskName $t -ErrorAction SilentlyContinue
        Write-Host "[ok] started task $t"
    }
}
Write-Host "[ok] update complete: $Version"
