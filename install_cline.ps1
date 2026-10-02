# Connect disk-search to Cline Desktop / Cline CLI - can be run at ANY time: before
# Cline is installed (re-run later) or after. Registers:
#   1) the disk-search MCP server in the Cline settings:
#      %USERPROFILE%\.cline\data\settings\cline_mcp_settings.json (Desktop/CLI)
#      and %USERPROFILE%\.cline\mcp.json (the CLI variant from docs.cline.bot/mcp);
#   2) the disk-search skill: hermes-skill\disk-search.md ->
#      %USERPROFILE%\.cline\skills\disk-search\SKILL.md
# Idempotent: a repeated run updates the entries without duplicating them.
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$hdsExe = Join-Path $root "bin\hds.exe"

$clineDir = Join-Path $env:USERPROFILE ".cline"
$clineCmd = Get-Command cline -ErrorAction SilentlyContinue
if (-not (Test-Path $clineDir) -and -not $clineCmd) {
    Write-Host "[--] Cline not found ($clineDir missing, 'cline' not in PATH)." -ForegroundColor Yellow
    Write-Host "    This is fine if disk-search is installed before Cline."
    Write-Host "    Once Cline Desktop is installed (https://cline.bot/desktop), connect with:"
    Write-Host "      powershell -File `"$root\install_cline.ps1`""
    Write-Host "    Or manually (see README, 'Cline Desktop integration')."
    exit 0
}
Write-Host "== Connecting disk-search to Cline ($clineDir) =="

# Python for the JSON merge helper: bundled sidecar -> HDS_EXTRACT_PYTHON -> .venv
$py = $null
if ($env:HDS_EXTRACT_PYTHON -and (Test-Path $env:HDS_EXTRACT_PYTHON)) { $py = $env:HDS_EXTRACT_PYTHON }
if (-not $py) {
    $bundled = Join-Path $root "sidecar\python"
    if (Test-Path $bundled) {
        $exe = Get-ChildItem -Path $bundled -Recurse -Filter python.exe -ErrorAction SilentlyContinue |
            Sort-Object { $_.FullName.Length } | Select-Object -First 1
        if ($exe) { $py = $exe.FullName }
    }
}
if (-not $py) {
    $venv = Join-Path $root ".venv\Scripts\python.exe"
    if (Test-Path $venv) { $py = $venv }
}
if (-not $py) { throw "no Python found for the Cline settings merge (set HDS_EXTRACT_PYTHON or install the sidecar)" }

$targets = @(
    (Join-Path $clineDir "data\settings\cline_mcp_settings.json"),
    (Join-Path $clineDir "mcp.json")
)
# Shared HTTP MCP instance (:8787): Cline connects by URL and does NOT spawn its own
# process (per-session processes were left orphaned by the long-lived hub daemon).
# The URL is read from config.yaml (mcp_http.*), without Python.
$mcpUrl = ""
$cfg = Join-Path $root "config.yaml"
if (Test-Path $cfg) {
    $cfgText = [System.IO.File]::ReadAllText($cfg, (New-Object System.Text.UTF8Encoding($false)))
    $mHost = "127.0.0.1"; $mPort = "8787"; $mPath = "/mcp"
    if ($cfgText -match '(?m)^mcp_http:[^\r\n]*\r?\n((?:[ \t]+[^\r\n]*(?:\r?\n|$))*)') {
        $blk = $Matches[1]
        if ($blk -match '(?m)^[ \t]+host:[ \t]*["'']?([^"''\s#]+)') { $mHost = $Matches[1] }
        if ($blk -match '(?m)^[ \t]+port:[ \t]*(\d+)') { $mPort = $Matches[1] }
        if ($blk -match '(?m)^[ \t]+path:[ \t]*["'']?([^"''\s#]+)') { $mPath = $Matches[1] }
    }
    $mcpUrl = "http://${mHost}:${mPort}${mPath}"
}
if (-not $mcpUrl) { throw "cannot derive the MCP URL (config.yaml mcp_http missing) - Cline settings unchanged" }

if (Test-Path $hdsExe) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $mcpRaw = & $hdsExe mcp-http restart 2>$null
    $ErrorActionPreference = $eap
    $mcpInfo = $null
    try { $mcpInfo = ($mcpRaw | Out-String) | ConvertFrom-Json } catch { }
    if ($mcpInfo -and $mcpInfo.action) {
        Write-Host "[ok] shared MCP server ($mcpUrl): $($mcpInfo.action)"
    } elseif ($mcpInfo -and $mcpInfo.error) {
        Write-Host "[!!] MCP server: $($mcpInfo.error)" -ForegroundColor Yellow
    } else {
        Write-Host ($mcpRaw | Out-String).Trim() -ForegroundColor Yellow
    }
}
& $py (Join-Path $root "installers\cline_mcp_merge.py") --mode http --url $mcpUrl --targets $targets
if ($LASTEXITCODE -ne 0) { throw "Cline MCP settings update failed - see the message above" }

# --- verify the settings with the Cline CLI itself (if available) ---
# Cline validates the whole settings file: one bad entry drops ALL MCP servers, so we
# not only check the shape (done by cline_mcp_merge.py) but that the client accepts it.
if ($clineCmd) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $clineCheck = (& cline config mcp --json 2>&1 | Out-String).Trim()
    $ErrorActionPreference = $eap
    if ($clineCheck -match 'Invalid MCP settings' -or $clineCheck -match '"type"\s*:\s*"error"') {
        Write-Host "[!!] Cline considers the MCP settings invalid (it will drop the whole file):" -ForegroundColor Yellow
        Write-Host "     $clineCheck" -ForegroundColor Yellow
        Write-Host "     File: $($targets[0])" -ForegroundColor Yellow
    } elseif ($clineCheck -match 'disk-search') {
        Write-Host "[ok] Cline sees the disk-search server"
    } else {
        Write-Host "[--] Cline did not list disk-search - check MCP Servers in the app" -ForegroundColor Yellow
    }
}

# --- disk-search skill ---
$skillSrc = Join-Path $root "hermes-skill\disk-search.md"
$skillDst = Join-Path $clineDir "skills\disk-search\SKILL.md"
if (Test-Path $skillSrc) {
    New-Item (Split-Path $skillDst) -ItemType Directory -Force | Out-Null
    Copy-Item $skillSrc $skillDst -Force
    Write-Host "[ok] skill installed: $skillDst"
} else {
    Write-Host "[--] hermes-skill\disk-search.md not found - skill skipped" -ForegroundColor Yellow
}

Write-Host "Restart Cline Desktop (or start a new session) for the changes to take effect."
