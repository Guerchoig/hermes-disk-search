# client-install.ps1 - ONE-COMMAND thin CLIENT installer for Windows (M9).
#
# The user runs this on the CLIENT machine with the token issued by the server
# admin. The script:
#   1) downloads the client package from the server
#      (GET <ServerUrl><McpPath>/client-package.zip?platform=windows-x64,
#       header `Authorization: Bearer <token>`);
#   2) unpacks it locally and clears Mark-of-the-Web;
#   3) runs the package master (setup-client.ps1): checks /health, compares the
#      client/server versions and writes the Cline MCP entry + rule + skill.
#
# This FILE needs no token to download: it is served publicly (release asset, or
# <ServerUrl><McpPath>/client-install.ps1) - the user passes the token as an
# argument.
#
# Usage:
#   iwr -UseBasicParsing https://host/mcp/client-install.ps1 -OutFile $env:TEMP\client-install.ps1
#   powershell -ExecutionPolicy Bypass -File $env:TEMP\client-install.ps1 -Token <TOKEN> -ServerUrl https://host
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [Parameter(Mandatory = $true)][string]$Token,
    [string]$ServerUrl = "",
    [string]$McpPath = "/mcp",
    [string]$Dir = "",
    [string]$Platform = "windows-x64",
    [switch]$DryRun
)
$ErrorActionPreference = "Stop"

if (-not $ServerUrl) {
    $ServerUrl = (Read-Host "Server URL (e.g. https://host/ or http://192.168.0.101:8787)").Trim()
}
if (-not $ServerUrl) { Write-Host "[!!] server URL is required" -ForegroundColor Red; exit 2 }
if (-not $Token) { $Token = (Read-Host "Bearer token").Trim() }
if (-not $Token) { Write-Host "[!!] token is required" -ForegroundColor Red; exit 2 }
if (-not $Dir) {
    $local = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { $env:TEMP }
    $Dir = Join-Path $local "hermes-disk-search-client"
}

$base = $ServerUrl.TrimEnd('/')
$mcp = if ($McpPath) { $McpPath.TrimEnd('/') } else { "/mcp" }
$url = "$base$mcp/client-package.zip?platform=$Platform"
$zip = Join-Path $Dir "hds-client.zip"

Write-Host "== hermes-disk-search thin client =="
Write-Host "   server : $base"
Write-Host "   package: $url"
Write-Host "   target : $Dir"

New-Item -ItemType Directory -Force -Path $Dir | Out-Null
Write-Host "== downloading the client package (token goes in Authorization: Bearer) =="
try {
    Invoke-WebRequest -Uri $url -Headers @{ Authorization = "Bearer $Token" } `
        -OutFile $zip -UseBasicParsing -TimeoutSec 120
} catch {
    Write-Host "[!!] cannot download the package: $($_.Exception.Message)" -ForegroundColor Yellow
    Write-Host "     - check the URL, the TLS certificate and the network" -ForegroundColor Yellow
    Write-Host "     - 401 = token is wrong/expired/revoked: ask the admin for a new one" -ForegroundColor Yellow
    Write-Host "     - 404 = the server has no client package yet (older server or dev build):" -ForegroundColor Yellow
    Write-Host "       take hds-client-<version>-<platform>.zip from the release page" -ForegroundColor Yellow
    exit 3
}

Write-Host "== unpacking =="
$unpacked = Join-Path $Dir "package"
if (Test-Path $unpacked) { Remove-Item -Recurse -Force $unpacked }
Expand-Archive -Path $zip -DestinationPath $unpacked -Force
Get-ChildItem -Path $unpacked -Recurse -File | ForEach-Object {
    try { Unblock-File -LiteralPath $_.FullName -ErrorAction Stop } catch { }
}
Remove-Item -Force $zip -ErrorAction SilentlyContinue

$master = Join-Path $unpacked "setup-client.ps1"
if (-not (Test-Path $master)) {
    Write-Host "[!!] setup-client.ps1 not found in the package ($unpacked)" -ForegroundColor Red
    Write-Host "     the server may have served a package for another platform (check ?platform=)" -ForegroundColor Yellow
    exit 4
}

Write-Host "== connecting Cline to the server =="
$masterArgs = @("-ServerUrl", $base, "-Token", $Token, "-McpPath", $mcp)
if ($DryRun) { $masterArgs += "-DryRun" }
& powershell -NoProfile -ExecutionPolicy Bypass -File $master @masterArgs
$rc = $LASTEXITCODE

Write-Host ""
if ($rc -eq 0) {
    Write-Host "[ok] client installed into $unpacked" -ForegroundColor Green
} else {
    Write-Host "[!!] the master reported problems (exit $rc) - see the lines above" -ForegroundColor Yellow
}
Write-Host "Restart Cline Desktop (or start a new session) for the changes to take effect."
exit $rc
