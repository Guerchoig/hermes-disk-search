# measure_rss.ps1 — замер резидентной памяти процессов hds (W0, §4 п.1, §12.3).
#
# Замеряет WorkingSet (RSS) процессов Python-стека в трёх состояниях:
#   idle        — простой (watcher жив, очередь пуста, llama-роли на месте);
#   watch       — работа watcher'а с очередью (сэмплирование во время индексации);
#   transcribe  — транскрипция медиа (сэмплирование во время обработки аудио/видео).
# Состояния watch/transcribe — это просто сэмплирование N раз с интервалом в момент,
# когда пользователь запустил индексацию; различие только в подписи состояния.
#
# Примеры:
#   powershell -File tools\measure_rss.ps1 -State idle
#   powershell -File tools\measure_rss.ps1 -State watch -Samples 30 -IntervalSec 10
#
# Результат: JSON в tools\parity\measurements\<state>-<stamp>.json
param(
    [string]$State = "idle",
    [int]$Samples = 5,
    [int]$IntervalSec = 10,
    [string]$OutFile = ""
)

$ErrorActionPreference = "SilentlyContinue"

function Get-HdsProcesses {
    $out = @()
    $procs = Get-CimInstance Win32_Process -Filter "Name LIKE 'python%'"
    foreach ($p in $procs) {
        $cmd = $p.CommandLine
        if (-not $cmd) { $cmd = "" }
        $role = $null
        if ($cmd -match "hds\.cli\s+watch" -or $cmd -match "hds[\\/]watcher") { $role = "watch" }
        elseif ($cmd -match "hds\.cli\s+mcp" -or $cmd -match "mcp_start" -or $cmd -match "hds\.mcp_") { $role = "mcp" }
        elseif ($cmd -match "hds\.cli\s+ui" -or $cmd -match "hds\.ui_server" -or $cmd -match "run_ui") { $role = "ui" }
        elseif ($cmd -match "run_index" -or $cmd -match "hds\.cli\s+index" -or $cmd -match "hds\.indexer") { $role = "index" }
        elseif ($cmd -match "hds\.llama_server") { $role = "llama-server-manager" }
        if (-not $role) { continue }
        $ws = 0
        try {
            $gp = Get-Process -Id $p.ProcessId -ErrorAction Stop
            $ws = [math]::Round($gp.WorkingSet64 / 1MB, 1)
        } catch { continue }
        # краткая сигнатура командной строки (без путей venv)
        $sig = $cmd -replace ".*python[w]?\.exe\s*", "" -replace '\s+', ' '
        if ($sig.Length -gt 120) { $sig = $sig.Substring(0, 120) }
        # дополнительные процессы-помощники текущей работы (ffmpeg/tesseract/llama-server)
        $out += [pscustomobject]@{
            pid       = $p.ProcessId
            role      = $role
            ws_mb     = $ws
            cmd       = $sig
        }
    }
    # llama-server (не python): поднимается менеджером, резидентно держит модели
    foreach ($lp in (Get-Process -Name "llama-server" -ErrorAction SilentlyContinue)) {
        $out += [pscustomobject]@{
            pid   = $lp.Id
            role  = "llama-server"
            ws_mb = [math]::Round($lp.WorkingSet64 / 1MB, 1)
            cmd   = "llama-server.exe"
        }
    }
    return $out
}

$results = @()
for ($i = 1; $i -le $Samples; $i++) {
    $procs = Get-HdsProcesses
    $total = [math]::Round(($procs | Measure-Object -Property ws_mb -Sum).Sum, 1)
    $hdsOnly = [math]::Round((($procs | Where-Object { $_.role -ne "llama-server" }) |
        Measure-Object -Property ws_mb -Sum).Sum, 1)
    $results += [pscustomobject]@{
        sample       = $i
        timestamp    = (Get-Date -Format "o")
        state        = $State
        total_mb     = $total
        python_mb    = $hdsOnly
        llama_mb     = [math]::Round((($procs | Where-Object { $_.role -eq "llama-server" }) |
            Measure-Object -Property ws_mb -Sum).Sum, 1)
        processes    = $procs
    }
    Write-Host ("[{0}/{1}] python={2} МБ, llama-server={3} МБ, всего={4} МБ ({5} процесс(ов))" -f `
        $i, $Samples, $hdsOnly,
        [math]::Round((($procs | Where-Object { $_.role -eq "llama-server" }) |
            Measure-Object -Property ws_mb -Sum).Sum, 1),
        $total, $procs.Count)
    if ($i -lt $Samples) { Start-Sleep -Seconds $IntervalSec }
}

$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
if (-not $OutFile) {
    $dir = Join-Path $PSScriptRoot "parity\measurements"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $OutFile = Join-Path $dir ("{0}-{1}.json" -f $State, $stamp)
}
@{
    state       = $State
    collected   = (Get-Date -Format "o")
    host        = $env:COMPUTERNAME
    samples_sec = $IntervalSec
    samples     = $results
} | ConvertTo-Json -Depth 6 | Set-Content -Path $OutFile -Encoding UTF8
Write-Host "Сохранено: $OutFile"