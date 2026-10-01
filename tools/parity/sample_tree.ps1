# W2 B-4 measurement: memory of a process TREE by root pid.
# Metrics match W0 (measure_run.py / sample_procs_light.ps1):
#   ws_mb    = WorkingSet64 (total), commit_mb = PrivateMemorySize64.
# ASCII-only (PowerShell reads BOM-less .ps1 as ANSI).
# Usage: powershell -NoProfile -File sample_tree.ps1 -RootPid 1234
param([int]$RootPid = 0)
$ErrorActionPreference = "SilentlyContinue"
$all = Get-CimInstance Win32_Process
$tree = @($RootPid)
$changed = $true
while ($changed) {
    $changed = $false
    foreach ($p in $all) {
        if ($tree -contains [int]$p.ParentProcessId -and -not ($tree -contains [int]$p.ProcessId)) {
            $tree += [int]$p.ProcessId
            $changed = $true
        }
    }
}
$ws = 0.0
$commit = 0.0
$rows = @()
foreach ($procid in $tree) {
    $gp = Get-Process -Id $procid -ErrorAction SilentlyContinue
    if ($gp) {
        $w = [math]::Round($gp.WorkingSet64 / 1MB, 1)
        $c = [math]::Round($gp.PrivateMemorySize64 / 1MB, 1)
        $ws += $w
        $commit += $c
        $rows += [pscustomobject]@{ pid = $procid; name = $gp.ProcessName; ws_mb = $w; commit_mb = $c }
    }
}
[pscustomobject]@{
    root_pid  = $RootPid
    tree_pids = $tree
    ws_mb     = [math]::Round($ws, 1)
    commit_mb = [math]::Round($commit, 1)
    processes = $rows
} | ConvertTo-Json -Depth 4 -Compress
