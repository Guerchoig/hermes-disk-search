# Connect disk-search to Hermes Desktop - can be run at ANY time:
#   - before Hermes is installed (just re-run later);
#   - after installation (registers the MCP server, the skill and the tool settings
#     in a single run).
# Registers:
#   1) the disk-search MCP server in config.yaml (mcp_servers section)
#   2) the disk-search skill (rule "search files via MCP, not grep")
#   3) tools.tool_search.enabled: "off" - all tools always in the prompt
#      (a local 9B model cannot discover MCP tools via the discovery protocol)
# Idempotent: a repeated run updates the blocks without duplicating them.
# ASCII-only on purpose (Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param([string]$HermesDir = "")
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$hdsExe = Join-Path $root "bin\hds.exe"
$mcpExe = Join-Path $root "bin\hds_mcp.exe"

if (-not $HermesDir) {
    $HermesDir = Join-Path $env:LOCALAPPDATA "hermes"
}
$cfgPath = Join-Path $HermesDir "config.yaml"
if (-not (Test-Path $cfgPath)) {
    Write-Host "[--] Hermes Desktop not found ($cfgPath)." -ForegroundColor Yellow
    Write-Host "    This is fine if disk-search is installed before Hermes."
    Write-Host "    Once Hermes Desktop is installed, connect with one command:"
    Write-Host "      powershell -File `"$root\install_hermes.ps1`""
    Write-Host "    Or manually (see README, 'Hermes integration'):"
    Write-Host "      1) add a disk-search block to <Hermes>\config.yaml (mcp_servers);"
    Write-Host "      2) copy hermes-skill\SKILL.md to <Hermes>\skills\disk-search\SKILL.md;"
    Write-Host "      3) add tools.tool_search.enabled: \`"off\`" (all tools in the prompt);"
    Write-Host "      4) if a system HTTP proxy is enabled (xray/Clash), add to <Hermes>\.env:"
    Write-Host "         NO_PROXY=localhost,127.0.0.1,::1 : httpx2 ignores ProxyOverride and"
    Write-Host "         otherwise sends 127.0.0.1 requests to the proxy, and MCP returns 503."
    exit 0
}
Write-Host "== Connecting disk-search to Hermes ($HermesDir) =="

# --- 1. MCP server disk-search in config.yaml (text edit, comments preserved) ---
# The shared HTTP MCP instance (:8787) is preferred: Hermes connects by URL and does
# not spawn a process per session. The URL is read from config.yaml (mcp_http.*).
$mcpUrl = ""
$cfgEarly = Join-Path $root "config.yaml"
if (Test-Path $cfgEarly) {
    $cfgText = [System.IO.File]::ReadAllText($cfgEarly, (New-Object System.Text.UTF8Encoding($false)))
    $mHost = "127.0.0.1"; $mPort = "8787"; $mPath = "/mcp"
    if ($cfgText -match '(?m)^mcp_http:[^\r\n]*\r?\n((?:[ \t]+[^\r\n]*(?:\r?\n|$))*)') {
        $blk = $Matches[1]
        if ($blk -match '(?m)^[ \t]+host:[ \t]*["'']?([^"''\s#]+)') { $mHost = $Matches[1] }
        if ($blk -match '(?m)^[ \t]+port:[ \t]*(\d+)') { $mPort = $Matches[1] }
        if ($blk -match '(?m)^[ \t]+path:[ \t]*["'']?([^"''\s#]+)') { $mPath = $Matches[1] }
    }
    $mcpUrl = "http://${mHost}:${mPort}${mPath}"
}
if ($mcpUrl) {
    $mcpBlock = @(
        '  disk-search:'
        "    url: $mcpUrl"
        '    timeout: 300'
    ) -join "`r`n"
} else {
    Write-Host "[--] MCP URL unknown - registering the stdio MCP server" -ForegroundColor Yellow
    $mcpBlock = @(
        '  disk-search:'
        "    command: $mcpExe"
        '    timeout: 300'
    ) -join "`r`n"
}
$enc = New-Object System.Text.UTF8Encoding($false)
$text = [System.IO.File]::ReadAllText($cfgPath, $enc)
# whole block: header + lines indented 4+ (arguments); neighbouring keys untouched
$rxBlock = [regex]::new('(?m)^  disk-search:\r?\n(?:    [^\r\n]*\r?\n?)*')
if ($rxBlock.IsMatch($text)) {
    $text = $rxBlock.Replace($text, { param($m) $mcpBlock + "`r`n" }, 1)
    Write-Host "[ok] MCP disk-search: block updated in config.yaml"
} elseif ($text -match '(?m)^mcp_servers:\s*$') {
    $text = [regex]::new('(?m)^mcp_servers:\s*$').Replace($text, { param($m) $m.Value + "`r`n" + $mcpBlock }, 1)
    Write-Host "[ok] MCP disk-search: added to the existing mcp_servers section"
} else {
    $text = $text.TrimEnd() + "`r`n`r`nmcp_servers:`r`n" + $mcpBlock + "`r`n"
    Write-Host "[ok] MCP disk-search: mcp_servers section appended"
}
$tmp = "$cfgPath.tmp"
[System.IO.File]::WriteAllText($tmp, $text, $enc)
Move-Item $tmp $cfgPath -Force

# check that the disk-search block is present (comments/neighbouring keys preserved)
if ($text -notmatch '(?m)^  disk-search:\s*$') { throw "disk-search block missing in config.yaml after edit" }
Write-Host "[ok] config.yaml: disk-search registered"

# --- 1b. tools.tool_search.enabled: "off" - all tools always in the prompt ---
# Local models (Qwen3.5-9B) cannot handle the tool_search/tool_describe/tool_call
# discovery protocol and do not find MCP tools; cloud models are unaffected.
$tsSearch = [regex]::new('(?m)^  tool_search:\r?\n(?:    [^\r\n]*\r?\n?)*')
$tsInner = @(
    '  tool_search:'
    '    enabled: "off"'
) -join "`r`n"
if ($tsSearch.IsMatch($text)) {
    $text = $tsSearch.Replace($text, { param($m) $tsInner + "`r`n" }, 1)
    Write-Host "[ok] tools.tool_search: block updated (enabled: off)"
} elseif ($text -match '(?m)^tools:\r?$') {
    $text = [regex]::new('(?m)^(tools:\r?\n)').Replace($text, { param($m) $m.Value + $tsInner + "`r`n" }, 1)
    Write-Host "[ok] tools.tool_search: added to the existing tools section"
} else {
    $text = $text.TrimEnd() + "`r`n`r`n# All tools always in the prompt (a local 9B model cannot discover MCP tools)`r`ntools:`r`n" + $tsInner + "`r`n"
    Write-Host "[ok] tools.tool_search section appended"
}
$tmp2 = "$cfgPath.tmp2"
[System.IO.File]::WriteAllText($tmp2, $text, $enc)
Move-Item $tmp2 $cfgPath -Force

# The server must be up BEFORE the agent connects: Hermes uses the URL and does not
# spawn a process. `restart` reuses a running instance but restarts a stale one
# (old code on the port), so Hermes never talks to a previous version.
if ($mcpUrl) {
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
    } else {
        Write-Host "[--] bin\hds.exe not found - start MCP manually: hds mcp-http start" -ForegroundColor Yellow
    }
}
if ($text -match '(?ms)^  tool_search:.*?enabled:\s*["'']?off') {
    Write-Host "[ok] tools.tool_search.enabled = off (all tools in the prompt)"
}

# --- 1c. System proxy bypass for loopback (required by the HTTP MCP mode) ---
# httpx2 (the Hermes HTTP engine) resolves proxies via urllib.request.getproxies(),
# which reads ProxyServer from the Windows registry and IGNORES ProxyOverride. Without
# these lines requests to the local MCP (127.0.0.1:8787) go through the system proxy
# (xray/Clash), the server answers 503 and Hermes reports
# "MCPError: Server returned an error response". We write a NO_PROXY block into
# <Hermes>\.env and mirror the system proxy into HTTP(S)_PROXY so external traffic
# keeps flowing. Idempotent: a repeated run replaces the block.
$envPath = Join-Path $HermesDir ".env"
$proxyUrl = ""
try {
    $inet = Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' -ErrorAction Stop
    if ($inet.ProxyEnable -and $inet.ProxyServer) {
        $first = ("$($inet.ProxyServer)" -split ';')[0]
        if ($first -match '=') { $first = ($first -split '=')[-1] }
        if ($first) { $proxyUrl = "http://$first" }
    }
} catch { }
$envBlock = @(
    '# >>> disk-search: system proxy bypass for loopback >>>'
    '# httpx2 (the Hermes HTTP engine) reads the Windows registry proxy and IGNORES'
    '# ProxyOverride. Without these lines, requests to local addresses (the MCP server'
    '# on 127.0.0.1:8787) go through the system proxy and MCP fails with'
    '# "MCPError: Server returned an error response".'
    '# HTTP(S)_PROXY mirror the system proxy so the internet keeps working.'
    '# Managed by the hermes-disk-search project (install_hermes.ps1) - edit there.'
    'NO_PROXY=localhost,127.0.0.1,::1'
    'no_proxy=localhost,127.0.0.1,::1'
)
if ($proxyUrl) { $envBlock += @("HTTP_PROXY=$proxyUrl", "HTTPS_PROXY=$proxyUrl") }
$envBlock += '# <<< disk-search <<<'
$envText = if (Test-Path $envPath) { [System.IO.File]::ReadAllText($envPath, $enc) } else { "" }
$rxEnv = [regex]::new('(?ms)^# >>> disk-search:.*?^# <<< disk-search <<<\r?\n?')
$envText = $rxEnv.Replace($envText, "").TrimEnd() + "`r`n`r`n" + ($envBlock -join "`r`n") + "`r`n"
[System.IO.File]::WriteAllText($envPath, $envText, $enc)
$proxyNote = if ($proxyUrl) { "; system proxy $proxyUrl mirrored into HTTP(S)_PROXY" } else { "" }
Write-Host "[ok] Hermes .env: NO_PROXY for loopback (127.0.0.1/localhost)$proxyNote"

# --- 2. disk-search skill ---
$skillSrc = Join-Path $root "hermes-skill\SKILL.md"
$skillDst = Join-Path $HermesDir "skills\disk-search\SKILL.md"
if (Test-Path $skillSrc) {
    New-Item (Split-Path $skillDst) -ItemType Directory -Force | Out-Null
    Copy-Item $skillSrc $skillDst -Force
    Write-Host "[ok] skill installed: $skillDst"
} else {
    Write-Host "[--] hermes-skill\SKILL.md not found - skill skipped" -ForegroundColor Yellow
}

Write-Host "Restart Hermes Desktop (or start a new session) for the changes to take effect."
