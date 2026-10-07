# install_server.ps1 -- server-profile installer for hermes-disk-search (M2 T4).
#
# Installs role services (watch | mcp-http | ui | llm-host) that start at boot
# WITHOUT user logon:
#   1. Preferred: Windows services (SCM) via `hds service install <role>`
#      (binPath -> `hds.exe service run <role>`, auto start, auto-restart on crash).
#   2. Fallback: AtStartup scheduled task (SYSTEM) when SCM creation is denied.
# Also prepares the state dir (%ProgramData%\hermes-disk-search) with a config
# template and (optionally) removes the desktop AtLogOn tasks for the roles.
#
# Usage (run as Administrator):
#   powershell -ExecutionPolicy Bypass -File install_server.ps1 [-Root <dir>] `
#              [-Roles watch,mcp-http,ui,llm-host] [-KeepAtLogOn]
param(
    [string]$Root = "",
    [string]$Roles = "watch,mcp-http,ui,llm-host",
    [switch]$KeepAtLogOn
)

$ErrorActionPreference = "Stop"

# --- Locate the installation root (bin\hds.exe next to this script). ---
if (-not $Root) {
    $Root = Split-Path -Parent $PSScriptRoot
}
$bin = Join-Path $Root "bin"
$hds = Join-Path $bin "hds.exe"
if (-not (Test-Path $hds)) {
    Write-Host "[!!] hds.exe not found at $hds" -ForegroundColor Red
    exit 1
}

# --- Admin check (SCM service creation requires it). ---
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "[!!] Run as Administrator (services need LocalSystem rights)" -ForegroundColor Red
    exit 1
}

# --- State dir (M2 T2): %ProgramData%\hermes-disk-search + data\. ---
$state = Join-Path $env:ProgramData "hermes-disk-search"
$dataDir = Join-Path $state "data"
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null

# --- config.yaml in the state dir (used when the install root has none). ---
$cfg = Join-Path $state "config.yaml"
if (-not (Test-Path $cfg)) {
    @'
profile: server
# Server profile: state (index.db, data/) lives in %ProgramData%\hermes-disk-search.
# Authorization is controlled by access.enabled only (profile does NOT enable it):
# access:
#   enabled: true
mcp_http:
  host: "127.0.0.1"   # publish /mcp only via reverse proxy with TLS
  port: 8787
ui:
  host: "127.0.0.1"   # admin UI behind reverse proxy; do not publish directly
  port: 8765
'@ | Out-File -FilePath $cfg -Encoding utf8
    Write-Host "[ok] config template: $cfg"
} else {
    Write-Host "[ok] config exists: $cfg"
}

# --- Desktop AtLogOn tasks (roles) are superseded by services. ---
if (-not $KeepAtLogOn) {
    foreach ($task in "HermesDiskSearchWatch", "HermesDiskSearchMcp", "HermesDiskSearchTranscribe") {
        schtasks /delete /tn $task /f *> $null
        if ($LASTEXITCODE -eq 0) { Write-Host "[ok] removed AtLogOn task: $task" }
    }
    # llm-host resident task (install_llm_host_task.ps1) and optional UI task.
    foreach ($task in "HermesDiskSearchLlmHost", "HermesDiskSearchUi") {
        schtasks /delete /tn $task /f *> $null
        if ($LASTEXITCODE -eq 0) { Write-Host "[ok] removed AtLogOn task: $task" }
    }
}

# --- Install services per role. ---
$summary = @()
foreach ($role in $Roles.Split(",")) {
    $role = $role.Trim()
    if (-not $role) { continue }
    & $hds service install $role *> $null
    if ($LASTEXITCODE -eq 0) {
        $summary += "[ok] service   $role (SCM: hermes-disk-search-$role, auto start)"
        continue
    }
    # Fallback: AtStartup task as SYSTEM (plan M2 risk mitigation).
    $tn = "HermesDiskSearchSvc-$role"
    $tr = "`"$hds`" service run $role"
    schtasks /create /tn $tn /tr $tr /sc onstart /ru SYSTEM /rl HIGHEST /f *> $null
    if ($LASTEXITCODE -eq 0) {
        $summary += "[ok] scheduled $role (AtStartup task: $tn)"
    } else {
        $summary += "[!!] FAILED    $role (both SCM service and AtStartup task)"
    }
}

Write-Host ""
Write-Host "=== Summary ==="
$summary | ForEach-Object { Write-Host $_ }
Write-Host ""
Write-Host "Next steps:"
Write-Host "  - Start now: bin\hds.exe service start <role>"
Write-Host "  - Status:    bin\hds.exe service status <role>"
Write-Host "  - Put TLS reverse proxy in front of /mcp and /ui (facade 8010-8012 stays local)."