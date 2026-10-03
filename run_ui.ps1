# Start the hermes-disk-search web UI (Rust build) and open the browser.
# Idempotent: if the UI server already answers on the port, the page is just opened.
#
# Server logs: %LOCALAPPDATA%\hermes-disk-search\ui.log and ui.err.log
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
$root = $PSScriptRoot
. (Join-Path $root 'hds_bin.ps1')
$uiPort = 8765
# bin\ (packaged release) or target\{release,debug}\ (source checkout).
$hdsExe = Get-HdsBinPath -Root $root -Name "hds.exe"
if (-not $hdsExe) {
    Write-Host "== hds.exe not found. Looked in: $(Get-HdsBinHint 'hds.exe') ==" -ForegroundColor Yellow
    Write-Host "   Run setup.cmd (release) or build it: cargo build --release -p hds-cli" -ForegroundColor Yellow
    Read-Host "Press Enter to exit"
    exit 1
}

$logDir = Join-Path $env:LOCALAPPDATA "hermes-disk-search"
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$outLog = Join-Path $logDir "ui.log"
$errLog = Join-Path $logDir "ui.err.log"

function Test-UiUp([int]$Port) {
    try { Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/status" -TimeoutSec 3 | Out-Null; return $true }
    catch { return $false }
}

if (Test-UiUp $uiPort) {
    Write-Host "[ok] UI server already running on port $uiPort"
    Start-Process "http://127.0.0.1:$uiPort"
    exit 0
}

Write-Host "[..] starting UI server on port $uiPort..."
# -WindowStyle Hidden: иначе у консольного hds.exe появляется отдельное окно, которое
# висит, пока жив сервер. Лаунчерское окно при успехе закрывается, при ошибке остаётся.
Start-Process $hdsExe -ArgumentList 'ui','--port',"$uiPort" `
    -WorkingDirectory $root -WindowStyle Hidden `
    -RedirectStandardOutput $outLog -RedirectStandardError $errLog

$up = $false
for ($i = 0; $i -lt 15 -and -not $up; $i++) {
    Start-Sleep 1
    $up = Test-UiUp $uiPort
}
if (-not $up) {
    Write-Host "== UI server did not start (port $uiPort) ==" -ForegroundColor Red
    Write-Host "Error log: $errLog" -ForegroundColor Yellow
    if (Test-Path $errLog) { Get-Content $errLog -Tail 30 }
    Read-Host "Press Enter to exit"
    exit 1
}
Write-Host "Logs: $errLog"
Start-Process "http://127.0.0.1:$uiPort"
