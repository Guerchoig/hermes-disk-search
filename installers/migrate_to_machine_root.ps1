# migrate_to_machine_root.ps1 - move an existing hermes-disk-search SERVER install
# from a per-user folder into a machine-wide location, and pull the model/engine
# assets out of the user profile (same-volume moves; the vendor engine path keeps
# a junction so the OpenResearchTools app still works). Run ELEVATED.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\migrate_to_machine_root.ps1 `
#       -From "C:\Users\<user>\hds-server" [-To "C:\Program Files\hermes-disk-search"]
#
# Why: a server install must not live in a user profile - services start without a
# logon, so profile cleanup/roaming/deletion would take the server down, other
# admins would not even be able to run bin\hds.exe, and the machine env vars would
# keep pointing into someone's AppData. Data already lives machine-wide under
# %ProgramData%\hermes-disk-search (the script never touches it).
#
# Idempotent: refuses to overwrite a non-empty target that is not an install, skips
# steps already done, disables the auto-restart before stopping the roles and
# restarts them on failure.
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [Parameter(Mandatory = $true)][string]$From,
    [string]$To = "C:\Program Files\hermes-disk-search",
    [string]$Roles = "watch,mcp-http,ui,llm-host",
    [string]$StateDir = "",
    [string]$UiUrl = "http://127.0.0.1:8765/",
    [string]$McpUrl = "http://127.0.0.1:8787/mcp",
    [string]$HealthUrl = "http://127.0.0.1:8787/health",
    [switch]$SkipAssets,      # do not move models/engine out of the user profile
    [switch]$SkipServiceEnv   # do not pin LLAMA_RUNTIME_DIR/HDS_ENGINE_DIR/HDS_EXTRACT_PYTHON
)
# Native tools (sc.exe/reg.exe/taskkill/robocopy) write diagnostics to stderr, and
# under "Stop" PowerShell turns that into a terminating NativeCommandError (the
# trap documented in tools/parity/README section 3, item 14). Keep "Continue" and
# check $LASTEXITCODE / results explicitly; cmdlets that must not fail silently
# are called with -ErrorAction Stop inside try/catch.
$ErrorActionPreference = "Continue"

# Transcript (elevated runs have no attached console output).
$logPath = Join-Path $env:TEMP "hds-migrate.log"
try { Start-Transcript -Path $logPath -Force | Out-Null } catch { }

function Say($m) { Write-Host $m }
function Fail($m) {
    Write-Host "[!!] $m" -ForegroundColor Red
    # Best effort: do not leave the server down (roles may have been stopped).
    foreach ($r in @("llm-host", "watch", "mcp-http", "ui")) {
        $n = "hermes-disk-search-$r"
        if ((ScState $n) -eq 'stopped') {
            & sc.exe start $n *> $null
            if (WaitSc $n 'running' 60) { Write-Host "[ok] restarted $n" }
        }
    }
    try { Stop-Transcript | Out-Null } catch { }
    exit 1
}
function ScState($n) {
    $o = (& sc.exe query $n 2>&1 | Out-String)
    if ($o -match '1060') { return 'absent' }
    if ($o -match 'RUNNING') { return 'running' }
    if ($o -match 'STOP_PENDING') { return 'stopping' }
    if ($o -match 'STOPPED') { return 'stopped' }
    return 'other'
}
function WaitSc($n, $want, $sec) {
    $d = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $d) { if ((ScState $n) -eq $want) { return $true }; Start-Sleep -Milliseconds 500 }
    return ((ScState $n) -eq $want)
}

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent(
    )).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { Fail "run elevated: stopping services and writing to $To needs Administrator" }
if (-not $StateDir) { $StateDir = Join-Path $env:ProgramData "hermes-disk-search" }
if (-not (Test-Path (Join-Path $From "bin\hds.exe")) -and -not (Test-Path (Join-Path $From "app\current\bin\hds.exe"))) {
    Fail "source not found: neither $From\bin\hds.exe nor $From\app\current\bin\hds.exe"
}
$cfg = Join-Path $StateDir "config.yaml"
if (-not (Test-Path $cfg)) { Fail "state config not found: $cfg" }

$roleList = @($Roles.Split(",") | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$svc = { param($r) "hermes-disk-search-$r" }

Say "=== migrate hermes-disk-search: $From -> $To ==="

# --- 1. stop roles (dependents first, GPU owner last) ------------------------
# These services carry failure-actions (auto-restart), and our stop makes the SCM
# treat the exit as a failure - it would silently bring the role back up (and keep
# engine DLLs locked). Disable the recovery actions first; install_server.ps1
# recreates the services with their proper settings in step 7.
foreach ($r in @("ui", "mcp-http", "watch", "llm-host")) {
    if ($roleList -notcontains $r) { continue }
    $n = & $svc $r
    if ((ScState $n) -eq 'absent') { Say "[..] service not installed: $n"; continue }
    # Disable the recovery actions via the registry (sc.exe quoting for an EMPTY
    # action list is unreliable in PowerShell); step 7 recreates the services.
    $sk = "HKLM:\SYSTEM\CurrentControlSet\Services\$n"
    Remove-ItemProperty -Path $sk -Name FailureActions -ErrorAction SilentlyContinue
    Remove-ItemProperty -Path $sk -Name FailureActionsOnNonCrashFailures -ErrorAction SilentlyContinue
    & sc.exe stop $n *> $null
    if (WaitSc $n 'stopped' 40) { Say "[ok] stopped $n" } else { Say "[!!] $n did not stop in 40s" }
}
$llm = Join-Path $From "app\current\bin\llm_host.exe"
if (Test-Path $llm) { try { & $llm stop *> $null } catch { }; Start-Sleep -Seconds 2; Say "[..] graceful llm_host stop sent" }
# Release the engine DLLs: a leftover resident would lock the engine directory.
& taskkill.exe /F /T /IM llm_host.exe *> $null
Start-Sleep -Seconds 2
Say "[..] leftover llm_host.exe killed (if any)"

# --- 2. copy the tree (junctions are NOT copied; recreated below) -----------
New-Item -ItemType Directory -Force -Path $To | Out-Null
$existing = @(Get-ChildItem $To -Force -ErrorAction SilentlyContinue)
if ($existing.Count -gt 0) {
    $hasHds = (Test-Path (Join-Path $To "bin\hds.exe")) -or (Test-Path (Join-Path $To "app\current\bin\hds.exe"))
    if (-not $hasHds) { Fail "target is not empty and not a hds install: $To" }
    Say "[..] target already has an install - syncing from the source (robocopy /E, no delete)"
}
$rc = Start-Process robocopy -ArgumentList @("`"$From`"", "`"$To`"", "/E", "/XJ", "/R:1", "/W:1", "/NFL", "/NDL", "/NP") -Wait -PassThru -NoNewWindow
Say "[ok] robocopy exit=$($rc.ExitCode) (0-7 = success)"
if ($rc.ExitCode -ge 8) { Fail "robocopy failed with $($rc.ExitCode)" }

# --- 3. recreate junctions (data/models per version dir, app\current) --------
$curFrom = Join-Path $From "app\current"
if (-not (Test-Path $curFrom)) { Fail "app\current junction not found in $From" }
$verName = Split-Path ((Get-Item $curFrom).Target | Select-Object -First 1) -Leaf
if (-not $verName) { Fail "cannot detect the current version from $curFrom" }
Say "[..] current version: $verName"
foreach ($d in @("data", "models")) { New-Item -ItemType Directory -Force -Path (Join-Path $To $d) | Out-Null }
foreach ($vd in (Get-ChildItem (Join-Path $To "app") -Directory -Force | Where-Object { $_.Name -ne 'current' })) {
    foreach ($d in @("data", "models")) {
        $link = Join-Path $vd.FullName $d
        $item = Get-Item $link -Force -ErrorAction SilentlyContinue
        if ($item -and $item.LinkType) { cmd /c rmdir "$link" | Out-Null; $item = $null }
        if (-not $item) {
            try {
                New-Item -ItemType Junction -Path $link -Target (Join-Path $To $d) -ErrorAction Stop | Out-Null
                Say "[ok] junction $link -> $(Join-Path $To $d)"
            } catch { Say "[!!] junction $link failed: $($_.Exception.Message)" }
        }
        if (-not (Test-Path $link)) { Fail "junction missing: $link" }
    }
}
$curLink = Join-Path $To "app\current"
$citem = Get-Item $curLink -Force -ErrorAction SilentlyContinue
if ($citem -and $citem.LinkType) { cmd /c rmdir "$curLink" | Out-Null }
try {
    New-Item -ItemType Junction -Path $curLink -Target (Join-Path $To ("app\" + $verName)) -ErrorAction Stop | Out-Null
    Say "[ok] junction $curLink -> $To\app\$verName"
} catch { Fail "cannot create $curLink : $($_.Exception.Message)" }
if (-not (Test-Path (Join-Path $curLink "bin\hds.exe"))) { Fail "no bin\hds.exe behind $curLink" }

# --- 4. a desktop config.yaml inside the version dir is a shadow - rename it --
$verCfg = Join-Path $To ("app\" + $verName + "\config.yaml")
if (Test-Path $verCfg) {
    Move-Item $verCfg ($verCfg + ".bak") -Force
    Say "[ok] shadowing config renamed aside: $verCfg.bak"
}

# --- 5. pull models/engine out of the user profile (same-volume moves) -------
$modelsFrom = Join-Path $env:LOCALAPPDATA "llama-runtime"
$modelsTo = Join-Path $StateDir "llama-runtime"
$engineFrom = Join-Path $env:APPDATA "OpenResearchTools\TranscribeOffline\Engine"
$engineTo = Join-Path $StateDir "engine"
if (-not $SkipAssets) {
    if ((Test-Path $modelsFrom) -and -not (Test-Path $modelsTo)) {
        try {
            Move-Item $modelsFrom $modelsTo -ErrorAction Stop
            $mb = [Math]::Round(((Get-ChildItem $modelsTo -Recurse -File -Force | Measure-Object Length -Sum).Sum / 1MB), 0)
            Say "[ok] models moved to $modelsTo ($mb MB)"
        } catch {
            Say "[!!] models move failed: $($_.Exception.Message) - staying at $modelsFrom"
            $modelsTo = $modelsFrom
        }
    } elseif (Test-Path $modelsTo) { Say "[..] models already at $modelsTo" } else { Say "[..] no models at $modelsFrom" }
    if ((Test-Path $engineFrom) -and -not (Test-Path $engineTo)) {
        $moved = $false
        for ($i = 1; $i -le 6 -and -not $moved; $i++) {
            try { Move-Item $engineFrom $engineTo -ErrorAction Stop; $moved = $true }
            catch { Say "[..] engine locked (attempt $i/6): $($_.Exception.Message)"; Start-Sleep -Seconds 3 }
        }
        if ($moved) {
            try { New-Item -ItemType Junction -Path $engineFrom -Target $engineTo -ErrorAction Stop | Out-Null }
            catch { Say "[!!] could not leave a junction at $engineFrom : $($_.Exception.Message)" }
            Say "[ok] engine moved to $engineTo (junction kept at $engineFrom for the vendor app)"
        } else {
            Say "[!!] engine stays locked - leaving it at $engineFrom (server keeps working)"
            $engineTo = $engineFrom
        }
    } elseif (Test-Path $engineTo) { Say "[..] engine already at $engineTo" } else { Say "[..] no engine at $engineFrom" }
}

# --- 6. env: machine level (CLI/admins) and per-service (applied at each start) -
$py = $null
$pyDir = Join-Path $To "app\current\sidecar\python"
if (Test-Path $pyDir) {
    $cand = Get-ChildItem $pyDir -Recurse -Filter python.exe -ErrorAction SilentlyContinue |
        Sort-Object { $_.FullName.Length } | Select-Object -First 1
    if ($cand) { $py = $cand.FullName }
}
$envPairs = @("HDS_CONFIG=$cfg")
if (-not $SkipAssets) {
    $envPairs += "LLAMA_RUNTIME_DIR=$modelsTo"
    $envPairs += "HDS_ENGINE_DIR=$engineTo"
    [Environment]::SetEnvironmentVariable("LLAMA_RUNTIME_DIR", $modelsTo, "Machine")
    [Environment]::SetEnvironmentVariable("HDS_ENGINE_DIR", $engineTo, "Machine")
}
if ($py) {
    $envPairs += "HDS_EXTRACT_PYTHON=$py"
    [Environment]::SetEnvironmentVariable("HDS_EXTRACT_PYTHON", $py, "Machine")
}
Say "[ok] machine env updated"
Say "[..] per-service env: $($envPairs -join ' | ')"

# --- 7. re-register the roles from the new root (new binPath) -----------------
# `sc.exe create` FAILS when the service already exists, and hds 0.20.1 has no
# `sc config` path - so the old registration must be removed first. Both calls go
# through the binary at the NEW root (the old tree will be deleted later).
# (`install_server.ps1` cannot do this: its `service install` is not idempotent and
# its fallback would create AtStartup scheduled tasks instead.)
$svcHds = Join-Path $To "app\current\bin\hds.exe"
if (-not (Test-Path $svcHds)) { $svcHds = Join-Path $To "bin\hds.exe" }
Say "[..] re-registering roles with $svcHds"
foreach ($r in $roleList) {
    $n = "hermes-disk-search-$r"
    if ((ScState $n) -ne 'absent') {
        & $svcHds service uninstall $r *> $null
        # sc delete is async: wait until the name is really gone, otherwise
        # `sc create` fails with "marked for deletion".
        $deadline = (Get-Date).AddSeconds(15)
        while ((ScState $n) -ne 'absent' -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
        Say "[..] old registration removed: $n ($(ScState $n))"
    }
    & $svcHds service install $r *> $null
    if ($LASTEXITCODE -eq 0) { Say "[ok] service   $r (binPath -> new root, auto start)" }
    else { Say "[!!] FAILED to register service $r" }
}

# --- 8. pin the remaining env per service (SCM caches machine env at boot) ----
if (-not $SkipServiceEnv) {
    $multi = $envPairs -join '\0'
    foreach ($r in $roleList) {
        $n = "hermes-disk-search-$r"
        if (-not (Test-Path "HKLM:\SYSTEM\CurrentControlSet\Services\$n")) { continue }
        & reg.exe add "HKLM\SYSTEM\CurrentControlSet\Services\$n" /v Environment /t REG_MULTI_SZ /d $multi /f *> $null
        if ($LASTEXITCODE -eq 0) { Say "[ok] service env pinned: $n" } else { Say "[!!] could not pin env for $n" }
    }
}


# --- 9. start the roles (resident first) -------------------------------------
foreach ($r in @("llm-host", "watch", "mcp-http", "ui")) {
    if ($roleList -notcontains $r) { continue }
    $n = "hermes-disk-search-$r"
    if ((ScState $n) -eq 'absent') { continue }
    & sc.exe start $n *> $null
    if (WaitSc $n 'running' 60) { Say "[ok] started $n" } else { Say "[!!] $n not running in 60s" }
}
Start-Sleep -Seconds 3

# --- 10. acceptance ----------------------------------------------------------
function Probe($u, $method = "GET") {
    try { (Invoke-WebRequest -UseBasicParsing -Method $method -Uri $u -TimeoutSec 8).StatusCode }
    catch { $_.Exception.Response.StatusCode.value__ }
}
Say ""
Say "=== acceptance ==="
$qc = (& sc.exe qc "hermes-disk-search-ui" 2>&1 | Out-String)
if ($qc -match [regex]::Escape($To)) { Say "[ok] service binPath points to the new root" }
else { Say "[!!] service binPath does NOT point to $To"; Say $qc }
Say ("GET  {0}            -> {1}   (expect 401: auth gate on)" -f $UiUrl, (Probe $UiUrl))
Say ("GET  {0}     -> {1}   (expect 200: public probe)" -f $HealthUrl, (Probe $HealthUrl))
Say ("POST {0}     -> {1}   (expect 401: no token)" -f $McpUrl, (Probe $McpUrl "POST"))

$env:HDS_CONFIG = $cfg
$out = (& (Join-Path $To "app\current\bin\hds.exe") check --json 2>&1 | Out-String)
Remove-Item Env:HDS_CONFIG -ErrorAction SilentlyContinue
try {
    $j = $out.Substring($out.IndexOf('{')) | ConvertFrom-Json
    $j.checks | Where-Object { $_.id -in @("profile", "db", "ocr", "ffmpeg", "gpu-observability", "config-shadow") } |
        ForEach-Object { Say ("[{0}] {1}: {2}" -f $_.status, $_.id, $_.msg) }
} catch { Say "[!!] hds check output not parsed (first 200 chars): $($out.Substring(0, [Math]::Min(200, $out.Length)))" }

Say ""
Say "[..] old install left in place: $From"
Say "     delete after acceptance: Remove-Item -LiteralPath '$From' -Recurse -Force"
Say "=== done ==="
try { Stop-Transcript | Out-Null } catch { }

