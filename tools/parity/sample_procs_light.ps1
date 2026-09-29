# W0 measurement (light): быстрый снимок процессов hds — только Get-Process/Win32_Process.
# Тяжёлый вариант (WorkingSetPrivate через Win32_PerfFormattedData) есть в sample_procs.ps1;
# здесь он намеренно не используется: выборка по всем процессам длится десятки секунд и
# сама тормозит замеряемую индексацию.
# ASCII-only: PowerShell читает .ps1 без BOM как ANSI.
$ErrorActionPreference = "SilentlyContinue"
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
    $gp = Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue
    $rows += [pscustomobject]@{
        pid        = $p.ProcessId
        role       = $role
        ws_mb      = if ($gp) { [math]::Round($gp.WorkingSet64 / 1MB, 1) } else { 0 }
        private_mb = if ($gp) { [math]::Round($gp.PrivateMemorySize64 / 1MB, 1) } else { 0 }
    }
}
foreach ($lp in (Get-Process -Name "llama-server" -ErrorAction SilentlyContinue)) {
    $rows += [pscustomobject]@{
        pid = $lp.Id; role = "llama-server"
        ws_mb = [math]::Round($lp.WorkingSet64 / 1MB, 1)
        private_mb = [math]::Round($lp.PrivateMemorySize64 / 1MB, 1)
    }
}
$vram = -1
try { $vram = [double](& nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits) } catch { }
[pscustomobject]@{ ts = (Get-Date -Format "HH:mm:ss"); vram_used_mib = $vram; procs = $rows } |
    ConvertTo-Json -Depth 4 -Compress