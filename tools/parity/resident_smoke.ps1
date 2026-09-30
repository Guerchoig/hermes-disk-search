# resident_smoke.ps1 -- live check of the resident llm-host (A6).
#
# NOTE: ASCII-only on purpose - Windows PowerShell 5.1 reads a .ps1 without BOM as
# ANSI and mangles non-ASCII text (tools/parity/README.md section 3, item 14).
#
# What is checked (without touching the production ports 8010-8012 or the VRAM
# used by the Python roles):
#   1. `llm_host run` takes the pid file and writes the log file;
#   2. a SECOND instance refuses to start (two owners of the GPU is the whole
#      point of the pid file);
#   3. `/internal/status` answers and prints the resident report (roles, VRAM,
#      pause, last dispatcher decision);
#   4. `/internal/devices` lists the engine devices;
#   5. `load` / `unload` of a role work through the internal API;
#   6. `/internal/stop` shuts the resident down and frees the pid file.
#
# The chat role runs on CPU (`--ngl 0`) so the check does not compete for VRAM.
#
# Example:
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\parity\resident_smoke.ps1 -PortBase 8030 -HoldSec 120

param(
    [int]$PortBase = 8030,
    [int]$WaitSec = 300,
    [int]$HoldSec = 120,
    [string]$Json = "tools\parity\out\w2_resident.json"
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Push-Location $root

$exe = Join-Path $root "target\release\llm_host.exe"
$pidPath = Join-Path $env:TEMP "llm-host-smoke.pid"
$logPath = Join-Path $env:TEMP "llm-host-smoke.log"
$outLog = Join-Path $env:TEMP "llm_host_smoke.out.log"
$errLog = Join-Path $env:TEMP "llm_host_smoke.err.log"

function Say([string]$msg) { Write-Host "== $msg" }

# Call llm_host with a temporary "Continue": in PS 5.1 native stderr becomes an
# ErrorRecord, and with ErrorActionPreference=Stop the script would die.
function Run([string]$what, [string[]]$argv) {
    Say "$what"
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $exe @argv
    $code = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($code -ne 0) { Write-Host "!! exit code ${code}: $what" }
    return $code
}

try {
    Say "building llm_host (release)"
    cargo build -q -p hds-llama --release --bin llm_host
    if (-not (Test-Path $exe)) { Write-Host "!! binary not found: $exe"; exit 1 }

    Remove-Item -Force -ErrorAction SilentlyContinue $pidPath, $logPath, $outLog, $errLog

    $common = @("--port-base", "$PortBase", "--ngl", "0", "--pid", $pidPath, "--log", $logPath)
    Say "starting resident: $exe run $($common -join ' ') --hold $HoldSec"
    $proc = Start-Process -FilePath $exe `
        -ArgumentList (@("run") + $common + @("--hold", "$HoldSec")) `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog -PassThru -NoNewWindow

    function Wait-Health([string]$url, [int]$sec) {
        $deadline = (Get-Date).AddSeconds($sec)
        while ((Get-Date) -lt $deadline) {
            try {
                $r = Invoke-RestMethod -Uri $url -TimeoutSec 3
                if ($r.status -eq "ok") { return $true }
            } catch { Start-Sleep -Seconds 2 }
        }
        return $false
    }

    Say "waiting for /health on $PortBase (CPU load takes 30-90 s)"
    if (-not (Wait-Health "http://127.0.0.1:$PortBase/health" $WaitSec)) {
        Write-Host "!! resident did not come up; tails:"
        Get-Content $outLog -Tail 30 -ErrorAction SilentlyContinue
        Get-Content $errLog -Tail 30 -ErrorAction SilentlyContinue
        exit 1
    }
    Say "/health ok"

    Say "pid file: $(Get-Content $pidPath -ErrorAction SilentlyContinue) (expected $($proc.Id))"
    if (-not (Test-Path $pidPath)) { Write-Host "!! pid file was not created: $pidPath"; exit 1 }
    if (-not (Test-Path $logPath)) { Write-Host "!! log file was not created: $logPath"; exit 1 }

    Say "second instance must refuse to start (two GPU owners)"
    $secondCode = Run "llm_host run (second instance)" @("run", "--port-base", "$PortBase", "--ngl", "0", "--pid", $pidPath, "--log", $logPath, "--hold", "5")
    if ($secondCode -eq 0) { Write-Host "!! second instance started - pid guard is broken"; exit 1 }
    Say "second instance refused (exit $secondCode) - ok"

    Say "internal status"
    Run "llm_host status --port $PortBase" @("status", "--port", "$PortBase", "--json", $Json)

    Say "internal devices"
    Run "llm_host devices --port $PortBase" @("devices", "--port", "$PortBase")

    Say "load embedding through the internal API"
    Run "llm_host load embedding --port $PortBase" @("load", "embedding", "--port", "$PortBase")

    Say "unload embedding"
    Run "llm_host unload embedding --port $PortBase" @("unload", "embedding", "--port", "$PortBase")

    Say "stop the resident"
    Run "llm_host stop --port $PortBase" @("stop", "--port", "$PortBase")
    if (-not $proc.WaitForExit(120000)) { Write-Host "!! resident did not exit; killing"; $proc.Kill() }
    Say "resident exit code: $($proc.ExitCode)"

    if (Test-Path $pidPath) {
        Write-Host "!! pid file survived shutdown: $pidPath (expected: removed)"
        exit 1
    }
    Say "pid file released - ok"

    Say "log tail:"
    Get-Content $logPath -Tail 25 -ErrorAction SilentlyContinue

    if (Test-Path $Json) { Say "report: $Json" } else { Write-Host "!! report was not written: $Json" }
} finally {
    if ($proc -and -not $proc.HasExited) { $proc.Kill() }
    Pop-Location
}
