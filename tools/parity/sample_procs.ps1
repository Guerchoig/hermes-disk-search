# W0 measurement: one sample of hds processes (WS + PrivateWS + PrivateBytes + VRAM).
# ASCII-only: PowerShell reads .ps1 without BOM as ANSI, so Cyrillic breaks parsing.
$ErrorActionPreference = "SilentlyContinue"
$perf = @{}
foreach ($q in (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -ErrorAction SilentlyContinue)) {
    $perf[[int]$q.IDProcess] = @{
        ws_private = [math]::Round([double]$q.WorkingSetPrivate / 1MB, 1)
        private    = [math]::Round([double]$q.PrivateBytes / 1MB, 1)
    }
}
$rows = @()
foreach ($p in (Get-CimInstance Win32_Process -Filter "Name LIKE 'python%'" -ErrorAction SilentlyContinue)) {
    $cmd = $p.CommandLine
    if (-not $cmd) { $cmd = "" }
    $role = $null
    if ($cmd -match "hds\.cli\s+watch" -or $cmd -match "hds[\\/]watcher") { $role = "watch" }
    elseif ($cmd -match "hds\.cli\s+mcp" -or $cmd -match "mcp_start" -or $cmd -match "hds\.mcp_") { $role = "mcp" }
    elseif ($cmd -match "hds\.cli\s+ui" -or $cmd -match "hds\.ui_server" -or $cmd -match "run_ui") { $role = "ui" }
    elseif ($cmd -match "run_index" -or $cmd -match "hds\.cli\s+(index|reindex|search|ask)" -or $cmd -match "hds\.indexer") { $role = "index" }
    if (-not $role) { continue }
    $pf = $perf[[int]$p.ProcessId]
    $rows += [pscustomobject]@{
        pid        = $p.ProcessId
        role       = $role
        ws_mb      = [math]::Round($p.WorkingSetSize / 1MB, 1)
        ws_priv_mb = if ($pf) { $pf.ws_private } else { -1 }
        private_mb = if ($pf) { $pf.private } else { [math]::Round($p.PrivatePageCount / 1MB, 1) }
    }
}
foreach ($lp in (Get-Process -Name "llama-server" -ErrorAction SilentlyContinue)) {
    $pf = $perf[[int]$lp.Id]
    $rows += [pscustomobject]@{
        pid        = $lp.Id
        role       = "llama-server"
        ws_mb      = [math]::Round($lp.WorkingSet64 / 1MB, 1)
        ws_priv_mb = if ($pf) { $pf.ws_private } else { -1 }
        private_mb = if ($pf) { $pf.private } else { [math]::Round($lp.PrivateMemorySize64 / 1MB, 1) }
    }
}
$vram = -1
try { $vram = [double](& nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits) } catch { }
[pscustomobject]@{ ts = (Get-Date -Format "HH:mm:ss"); vram_used_mib = $vram; procs = $rows } |
    ConvertTo-Json -Depth 4 -Compress