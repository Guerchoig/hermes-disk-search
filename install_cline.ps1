# Connect disk-search to Cline Desktop / Cline CLI - can be run at ANY time: before
# Cline is installed (re-run later) or after. One call of `hds cline-sync` (the same
# code as the UI button "Synchronize Cline settings") sets up everything:
#   1) the disk-search MCP server in BOTH Cline settings files:
#      %USERPROFILE%\.cline\data\settings\cline_mcp_settings.json (Desktop/CLI)
#      and %USERPROFILE%\.cline\mcp.json (the CLI variant from docs.cline.bot/mcp);
#   2) model context windows in %USERPROFILE%\.cline\data\settings\models.json:
#      contextWindow/maxInputTokens = the real llm-host slot (ctx_per_slot). Without
#      this Cline compacts the history before the slot is full and the agent loses
#      the search context;
#   3) the disk-search RULE: cline-rules\disk-search.md -> ~/.cline/rules/disk-search.md
#      (rules go into the system prompt of EVERY session);
#   4) the disk-search skill: hermes-skill\disk-search.md -> ~/.cline/skills/disk-search/SKILL.md
#      (skills load lazily - only after the model calls use_skill; a local 9B agent
#      does not, which reads as "incomplete results" - hence the rule above).
# Cline must be restarted after models/MCP changes (the report says so).
# Idempotent: a repeated run updates the entries without duplicating them.
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
. (Join-Path $root 'hds_bin.ps1')
# bin\ (packaged release) or target\{release,debug}\ (source checkout).
$hdsExe = Get-HdsBinPath -Root $root -Name "hds.exe"

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

# Settings are written by the SAME code the UI button uses (`hds cline-sync`):
# models.json context windows, both MCP settings files, the rule and the skill.
if (-not $hdsExe) {
    throw "hds.exe not found ($(Get-HdsBinHint -Name 'hds.exe')) - run setup.ps1 / build the release first"
}
# project_root() of the Rust core resolves config.yaml, cline-rules\ and hermes-skill\
# from the exe location; pin it to this root explicitly (the installer may run from
# another directory, e.g. an unpacked archive).
$env:HDS_ROOT = $root

if (Test-Path $hdsExe) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $mcpRaw = & $hdsExe mcp-http restart 2>$null
    $ErrorActionPreference = $eap
    $mcpInfo = $null
    try { $mcpInfo = ($mcpRaw | Out-String) | ConvertFrom-Json } catch { }
    if ($mcpInfo -and $mcpInfo.state) {
        Write-Host "[ok] shared MCP server ($($mcpInfo.url)): pid $($mcpInfo.pid)"
    } elseif ($mcpInfo -and $mcpInfo.error) {
        Write-Host "[!!] MCP server: $($mcpInfo.error)" -ForegroundColor Yellow
    } else {
        Write-Host ($mcpRaw | Out-String).Trim() -ForegroundColor Yellow
    }
}
# --- settings sync: models.json + both MCP files + rule + skill ---
& $hdsExe cline-sync
if ($LASTEXITCODE -ne 0) {
    Write-Host "[!!] cline-sync reported warnings - see the lines above" -ForegroundColor Yellow
}

# --- verify the settings with the Cline CLI itself (if available) ---
# Cline validates the whole settings file: one bad entry drops ALL MCP servers, so we
# do not only check the shape (done by `hds cline-sync`) but that the client accepts it.
if ($clineCmd) {
    $eap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    $clineCheck = (& cline config mcp --json 2>&1 | Out-String).Trim()
    $ErrorActionPreference = $eap
    if ($clineCheck -match 'Invalid MCP settings' -or $clineCheck -match '"type"\s*:\s*"error"') {
        Write-Host "[!!] Cline considers the MCP settings invalid (it will drop the whole file):" -ForegroundColor Yellow
        Write-Host "     $clineCheck" -ForegroundColor Yellow
        Write-Host "     File: $clineDir\data\settings\cline_mcp_settings.json" -ForegroundColor Yellow
    } elseif ($clineCheck -match 'disk-search') {
        Write-Host "[ok] Cline sees the disk-search server"
    } else {
        Write-Host "[--] Cline did not list disk-search - check MCP Servers in the app" -ForegroundColor Yellow
    }
}

# The rule and the skill were installed by `hds cline-sync` above (one source of truth
# with the UI button); failures/warnings are reported there.
Write-Host "Restart Cline Desktop (or start a new session) for the changes to take effect."
