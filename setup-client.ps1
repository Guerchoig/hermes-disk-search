# setup-client.ps1 - thin CLIENT installer for hermes-disk-search (M9 / Э9 / W7).
#
# Connects Cline (Desktop/CLI) to a REMOTE disk-search server: writes the MCP
# server entry (external URL + `Authorization: Bearer`) into Cline's settings and
# installs the rule + skill. NO engine / models / index / sidecar is installed -
# this is a THIN client; all heavy lifting stays on the server.
#
# Steps:
#   1) locate hds.exe (bin\hds.exe next to this script, or target\{debug,release});
#   2) check the connection: GET <ServerUrl>/health -> 200;
#   3) hds cline-sync --server-url <ServerUrl> --mcp-path <path>   (token via env);
#   4) tell the user to restart Cline.
#
# The token is passed to hds via the HDS_CLINE_TOKEN environment variable (not on
# the command line) and is never printed. Idempotent: a repeated run updates the
# entries without duplicating them. No Administrator rights are needed.
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   powershell -ExecutionPolicy Bypass -File setup-client.ps1 -ServerUrl https://host/ -Token <bearer>
#   powershell -ExecutionPolicy Bypass -File setup-client.ps1 -ServerUrl https://host/ -Token <bearer> -DryRun
param(
    [string]$ServerUrl = "",
    [string]$Token = "",
    [string]$McpPath = "/mcp",
    [switch]$DryRun,
    [switch]$SkipHealth
)
$ErrorActionPreference = "Stop"

# --- locate hds.exe (packaged bin\ next to this script, else a source checkout) ---
$root = $PSScriptRoot
$hds = Join-Path $root "bin\hds.exe"
if (-not (Test-Path $hds)) {
    foreach ($cand in @((Join-Path $root "target\debug\hds.exe"),
                        (Join-Path $root "target\release\hds.exe"))) {
        if (Test-Path $cand) { $hds = $cand; break }
    }
}
if (-not (Test-Path $hds)) {
    Write-Host "[!!] hds.exe not found (bin\hds.exe) - unpack the client package first" -ForegroundColor Red
    exit 1
}

# --- interactive fallbacks ---
if (-not $ServerUrl) { $ServerUrl = (Read-Host "Server URL (e.g. https://host/)").Trim() }
if (-not $ServerUrl) { Write-Host "[!!] server URL is required" -ForegroundColor Red; exit 2 }
if (-not $Token) { $Token = (Read-Host "Bearer token").Trim() }
if (-not $Token) { Write-Host "[!!] token is required" -ForegroundColor Red; exit 2 }

# --- 1. connection check: GET <ServerUrl>/health ---
$base = $ServerUrl.TrimEnd('/')
$mcpPath = if ($McpPath) { $McpPath.TrimEnd('/') } else { "/mcp" }
$srvVer = ""
if (-not $SkipHealth) {
    $health = "$base/health"
    Write-Host "== checking connection: $health =="
    try {
        $r = Invoke-WebRequest -Uri $health -UseBasicParsing -TimeoutSec 10
        if ($r.StatusCode -eq 200) {
            Write-Host "[ok] server reachable ($($r.StatusCode))"
            # W7: /health publishes the product version (M9) - read it for the check
            try { $srvVer = [string]($r.Content | ConvertFrom-Json).version } catch { $srvVer = "" }
        } else {
            Write-Host "[!!] server responded $($r.StatusCode)" -ForegroundColor Yellow
        }
    } catch {
        Write-Host "[!!] server not reachable: $($_.Exception.Message)" -ForegroundColor Yellow
        Write-Host "     check the URL, the TLS certificate and the network (-SkipHealth to ignore)" -ForegroundColor Yellow
        exit 3
    }
}

# --- 1b. client vs server version (W7): warn and point at the matching package ---
# The client package carries version.txt (written by build_client_release.ps1);
# the server reports its version in /health. A mismatch means the rule/skill or
# the tool schema may be older than the server.
$localVer = ""
$verFile = Join-Path $root "version.txt"
if (Test-Path $verFile) { $localVer = (Get-Content $verFile -Raw).Trim() }
if ($localVer -and $srvVer) {
    if ($localVer -eq $srvVer) {
        Write-Host "[ok] client/server versions match ($localVer)"
    } else {
        Write-Host "[!!] version mismatch: client $localVer, server $srvVer" -ForegroundColor Yellow
        Write-Host "     get the matching package: $base$mcpPath/client-package.zip (with your token) or the release asset" -ForegroundColor Yellow
    }
} elseif (-not $localVer) {
    Write-Host "[i] no version.txt here (dev layout) - version check skipped"
}

# --- 2. write Cline settings (MCP + rule + skill) via hds cline-sync ---
Write-Host "== writing Cline settings (MCP + rule + skill) =="
$env:HDS_CLINE_TOKEN = $Token   # not on the command line; removed below
$hdsArgs = @("cline-sync", "--server-url", $ServerUrl, "--mcp-path", $McpPath)
if ($DryRun) { $hdsArgs += "--dry-run" }
& $hds @hdsArgs
$rc = $LASTEXITCODE
Remove-Item Env:HDS_CLINE_TOKEN -ErrorAction SilentlyContinue
if ($rc -ne 0) {
    Write-Host "[!!] cline-sync reported warnings - see the lines above" -ForegroundColor Yellow
}

Write-Host ""
Write-Host "Restart Cline Desktop (or start a new session) for the changes to take effect."
