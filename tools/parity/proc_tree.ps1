# W0 spike 3: memory of a process TREE (uv venv python.exe is a trampoline).
# Usage: powershell -NoProfile -File proc_tree.ps1 -RootPid 1234
param([int]$RootPid = 0)
$ErrorActionPreference = "SilentlyContinue"
$perf = @{}
foreach ($q in (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process)) {
    $perf[[int]$q.IDProcess] = @{
        name = $q.Name
        ws   = [math]::Round([double]$q.WorkingSetPrivate / 1MB, 1)
        priv = [math]::Round([double]$q.PrivateBytes / 1MB, 1)
    }
}
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
$ws = 0.0; $priv = 0.0; $rows = @()
foreach ($procId in $tree) {
    $m = $perf[$procId]
    if ($m) {
        $ws += $m.ws; $priv += $m.priv
        $rows += [pscustomobject]@{ pid = $procId; name = $m.name; ws_priv_mb = $m.ws; commit_mb = $m.priv }
    }
}
[pscustomobject]@{
    root_pid    = $RootPid
    tree_pids   = $tree
    ws_priv_mb  = [math]::Round($ws, 1)
    commit_mb   = [math]::Round($priv, 1)
    processes   = $rows
} | ConvertTo-Json -Depth 4 -Compress