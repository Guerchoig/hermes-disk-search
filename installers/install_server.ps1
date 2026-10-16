# install_server.ps1 -- server-profile installer for hermes-disk-search (M2 T4, M8 T1).
#
# Installs role services (watch | mcp-http | ui | llm-host) that start at boot
# WITHOUT user logon:
#   1. Preferred: Windows services (SCM) via `hds service install <role>`
#      (binPath -> `hds.exe service run <role>`, auto start, auto-restart on crash).
#   2. Fallback: AtStartup scheduled task (SYSTEM) when SCM creation is denied.
# Also prepares the server state dir with a config generated from parameters,
# optionally downloads the engine runtime / models unattended (fetch_*), and can
# enable access + create the first admin user.
# ASCII-only on purpose: PowerShell 5.1 reads a BOM-less .ps1 as ANSI.
#
# A no-argument run keeps the ORIGINAL M2 behaviour (simple template, no fetch,
# no access). The unattended parameters only activate what is explicitly passed.
# Use -WhatIf to preview the plan + generated config WITHOUT changes and WITHOUT
# admin. Use `-Roles ""` for a config-only install (no SCM, no admin needed).
#
# Example (unattended, run as Administrator):
#   powershell -ExecutionPolicy Bypass -File install_server.ps1 `
#     -StateDir C:\ProgramData\hermes-disk-search -McpHost 127.0.0.1 -McpPort 8787 `
#     -UiHost 127.0.0.1 -UiPort 8765 -Parallel 5 -GpuDevice 1 -Roots "D:\;E:\Archive" `
#     -EnableAccess -AdminLogin admin -AdminPassword <secret> -Fetch
param(
    [string]$Root = "",
    [string]$Roles = "watch,mcp-http,ui,llm-host,transcribe",
    [switch]$KeepAtLogOn,
    # --- M8 T1 (unattended server profile) ---
    [string]$StateDir = "",          # default %ProgramData%\hermes-disk-search
    [string]$McpHost = "127.0.0.1",
    [int]$McpPort = 8787,
    [string]$UiHost = "127.0.0.1",
    [int]$UiPort = 8765,
    [int]$Parallel = 0,              # 0 = do not write (server default = 5); >0 writes llm_server.parallel
    [string]$GpuDevice = "",         # GPU index ("1"); "" = do not write
    [string]$GpuPreset = "",         # none|nvidia-one|nvidia-n|amd-n|apple; overrides GpuDevice
    [string]$Roots = "",             # index roots, separated by ; or ,
    # --- R3.5 (PLAN_TRANSCRIBE_SERVICE): auto-transcription service ---
    [string]$MediaInbox = "",        # auto_transcribe.inbox_dir (absolute; excluded from indexing)
    [string]$MediaOut = "",          # auto_transcribe.out_dir (absolute; excluded from indexing)
    [switch]$EnableTranscribe,       # auto_transcribe.enabled: true
    [string]$SourceDisposal = "delete",  # delete|keep|move (product default: delete)
    [switch]$EnableAccess,           # access.enabled: true
    [string]$AdminLogin = "",        # create/update the first admin (with -AdminPassword)
    [string]$AdminPassword = "",
    [switch]$Fetch,                  # download runtime/models unattended (fetch_*.ps1, idempotent)
    [switch]$NoFetch,                # force-skip downloads (even with -Fetch)
    # --- 0.20.2 (incident 15.10.2026): services need their config and their deps ---
    [switch]$SkipDeps,               # do NOT install machine-wide deps (Tesseract OCR, ffmpeg)
    [switch]$NoConfigPin,            # keep the root config.yaml and do NOT pin HDS_CONFIG for services
    [switch]$WhatIf                  # print the plan + config only (no changes, no admin)
)

$ErrorActionPreference = "Stop"

# --- Locate the SOURCE (the unpacked archive: bin\hds.exe next to this script)
# and the install root (TARGET). `-Root` defaults to the source folder, which is
# the "unpack into the machine folder" flow; passing `-Root` explicitly is the
# "unpack anywhere + point at the machine folder" flow (README_SERVER_MULTIUSER,
# chapter "Installing the server on Windows").
$source = Split-Path -Parent $PSScriptRoot
if (-not $Root) { $Root = $source }
$installTarget = $Root                       # where app\, data\ and models\ live

# --- 0.20.3: register the roles against a VERSION, not against the unpacked tree.
# Why: launch points (SCM binPath, logon tasks, the desktop shortcut) keep an
# ABSOLUTE path to hds.exe. When they point at <root>\bin\hds.exe (what a plain
# "unpack into the install folder + install-server.cmd" produces), update.ps1
# cannot replace that file: it installs the new version into app\<ver> and switches
# app\current, so the roles would keep running the OLD binaries and -RollbackTo
# would switch a pointer nothing reads (incident 16.10.2026). Therefore, when the
# target is a machine folder DIFFERENT from the unpacked archive, the version is
# laid out first (install_app_version.ps1 - the very same step update.ps1 uses) and
# the roles are registered against app\current\bin\hds.exe.
$versioned = Test-Path (Join-Path $installTarget "app\current\bin\hds.exe")
$planLayout = $false                         # fresh version layout to install
$layoutVer = ""
$layoutNote = ""
if ($versioned) {
    $layoutNote = "versioned install detected: roles run from $installTarget\app\current\bin"
} elseif (($installTarget -ne $source) -and (Test-Path (Join-Path $source "bin\hds.exe"))) {
    foreach ($vf in @((Join-Path $source "version.txt"), (Join-Path $installTarget "version.txt"))) {
        if (Test-Path $vf) { $layoutVer = (Get-Content $vf -TotalCount 1).Trim(); if ($layoutVer) { break } }
    }
    if ($layoutVer) {
        $planLayout = $true
        $layoutNote = "version $layoutVer will be laid out under $installTarget\app\$layoutVer; roles run from app\current\bin (updates and -RollbackTo switch app\current)"
    } else {
        $layoutNote = "no version.txt in the archive: flat layout (no versions kept; update.cmd re-points the roles itself)"
    }
} else {
    $layoutNote = "flat layout (the archive IS the install root): unpack the archive elsewhere and pass -Root <machine folder> to get the version layout; update.cmd re-points the roles either way"
}
if ($versioned) { $Root = Join-Path $installTarget "app\current" }
$bin = Join-Path $Root "bin"
$hds = Join-Path $bin "hds.exe"

# --- State dir (server profile; %ProgramData% by default). ---
if (-not $StateDir) { $StateDir = Join-Path $env:ProgramData "hermes-disk-search" }
$state = $StateDir
$dataDir = Join-Path $state "data"
$cfg = Join-Path $state "config.yaml"

function Test-IsAdmin {
    ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
        ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# YAML single-quoted scalar (backslashes stay literal; only ' is doubled).
function ConvertTo-YamlSingle([string]$s) { "'" + ($s -replace "'", "''") + "'" }

# Set/replace "  <key>: '<value>'" inside the top-level `index:` section.
# Line-based: comments are preserved and list/block values are never clobbered;
# the section is created when it is missing.
function Set-IndexKey([string]$Path, [string]$Key, [string]$Value) {
    $enc = New-Object System.Text.UTF8Encoding($false)
    $lines = [IO.File]::ReadAllLines($Path, $enc)
    $quoted = "'" + ($Value -replace "'", "''") + "'"
    $hasSection = [bool]($lines | Where-Object { $_ -match '^index:[ \t]*(#.*)?$' })
    $out = New-Object System.Collections.Generic.List[string]
    $inIndex = $false; $done = $false; $locked = $false
    foreach ($line in $lines) {
        if ($line -match '^[^ \t#]') {
            if ($inIndex -and -not $done -and -not $locked) { $out.Add("  ${Key}: $quoted"); $done = $true }
            $inIndex = [bool]($line -match '^index:[ \t]*(#.*)?$')
        } elseif ($inIndex -and -not $done -and -not $locked) {
            if ($line -match ("^ {2}" + [regex]::Escape($Key) + ":[ \t]*(.*)$")) {
                if ($Matches[1].Trim() -eq '') {
                    $locked = $true      # block value (a list) - leave it alone
                } else {
                    $out.Add("  ${Key}: $quoted"); $done = $true; continue
                }
            }
        }
        $out.Add($line)
    }
    if ($inIndex -and -not $done -and -not $locked) { $out.Add("  ${Key}: $quoted"); $done = $true }
    if (-not $hasSection -and -not $done) { $out.Add("index:"); $out.Add("  ${Key}: $quoted") }
    [IO.File]::WriteAllLines($Path, $out, $enc)
}

# Server config body. Backward-compatible sections; optional parts only when set.
function New-ServerConfig {
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add("profile: server")
    $lines.Add("# Generated by install_server.ps1 (M8 T1) $(Get-Date -Format s)")
    $lines.Add("# Authorization is controlled by access.enabled only (profile does NOT enable it).")
    $lines.Add("access:")
    if ($EnableAccess) { $lines.Add("  enabled: true") } else { $lines.Add("  enabled: false") }
    $lines.Add("  token_ttl_days: 90")
    if ($Parallel -gt 0) { $lines.Add("llm_server:"); $lines.Add("  parallel: $Parallel") }
    if ($GpuPreset) {
        $lines.Add("gpu:"); $lines.Add("  preset: " + (ConvertTo-YamlSingle $GpuPreset))
    } elseif ($GpuDevice -match '^\d+$') {
        $lines.Add("gpu:"); $lines.Add("  device_index: $GpuDevice")
    }
    $rootList = @()
    if ($Roots) { $rootList = @($Roots -split '[;,]' | ForEach-Object { $_.Trim() } | Where-Object { $_ }) }
    if ($rootList.Count -gt 0 -or $MediaInbox -or $MediaOut) {
        $lines.Add("index:")
        if ($rootList.Count -gt 0) {
            $lines.Add("  roots:")
            foreach ($r in $rootList) { $lines.Add("    - " + (ConvertTo-YamlSingle $r)) }
        }
        if ($MediaInbox -or $MediaOut) {
            # auto-transcribe dirs are excluded from indexing (implicitly too; kept explicit for clarity)
            $lines.Add("  exclude_paths:")
            if ($MediaInbox) { $lines.Add("    - " + (ConvertTo-YamlSingle $MediaInbox)) }
            if ($MediaOut) { $lines.Add("    - " + (ConvertTo-YamlSingle $MediaOut)) }
        }
    }
    if ($EnableTranscribe -or $MediaInbox -or $MediaOut) {
        # R3.5: auto-transcription+diarization service (hds transcribe-watch).
        $lines.Add("auto_transcribe:")
        if ($EnableTranscribe) { $lines.Add("  enabled: true") } else { $lines.Add("  enabled: false") }
        if ($MediaInbox) { $lines.Add("  inbox_dir: " + (ConvertTo-YamlSingle $MediaInbox)) }
        if ($MediaOut) { $lines.Add("  out_dir: " + (ConvertTo-YamlSingle $MediaOut)) }
        $lines.Add("  diarization: true")
        $lines.Add("  diarization_required: true")
        $sd = if ($SourceDisposal) { $SourceDisposal } else { "delete" }
        $lines.Add("  source_disposal: $sd")
        $lines.Add("  pause_indexing: true")
    }
    $lines.Add("mcp_http:")
    $lines.Add("  host: " + (ConvertTo-YamlSingle $McpHost) + "   # publish /mcp only via reverse proxy with TLS")
    $lines.Add("  port: $McpPort")
    $lines.Add("ui:")
    $lines.Add("  host: " + (ConvertTo-YamlSingle $UiHost) + "   # admin UI behind reverse proxy; do not publish directly")
    $lines.Add("  port: $UiPort")
    return ($lines -join "`r`n") + "`r`n"
}

$cfgBody = New-ServerConfig

# --- -WhatIf: print the plan and stop (no admin, no changes). ---
if ($WhatIf) {
    Write-Host "=== install_server.ps1 (WhatIf, no changes) ==="
    Write-Host "Root:      $Root"
    Write-Host "Layout:    $layoutNote"
    Write-Host "StateDir:  $state"
    if (Test-Path $cfg) { Write-Host "Config:    $cfg (exists - will NOT be overwritten)" } else { Write-Host "Config:    $cfg (will be created)" }
    Write-Host "Roles:     $Roles"
    if ($Fetch -and -not $NoFetch) { Write-Host "Fetch:     yes (unattended fetch_*)" } else { Write-Host "Fetch:     no" }
    Write-Host "Access:    enabled=$EnableAccess"
    if ($AdminLogin) { Write-Host "Admin:     $AdminLogin" } else { Write-Host "Admin:     (none)" }
    if ($SkipDeps) {
        Write-Host "Deps:      skipped (-SkipDeps)"
    } else {
        Write-Host "Deps:      machine-wide Tesseract OCR (+rus/eng packs) and ffmpeg (winget --scope machine)"
    }
    if ($NoConfigPin) {
        Write-Host "ConfigPin: off (-NoConfigPin): root config.yaml is left as is"
    } else {
        Write-Host "ConfigPin: root config.yaml renamed to config.desktop.yaml.bak; HDS_CONFIG pinned per service"
    }
    Write-Host ""
    Write-Host "--- config.yaml (body) ---"
    Write-Host $cfgBody
    exit 0
}

# --- Admin check (only when services will actually be installed). ---
if ($Roles.Trim() -and -not (Test-IsAdmin)) {
    Write-Host "[!!] Run as Administrator (services need LocalSystem rights)" -ForegroundColor Red
    exit 1
}

# --- 0.20.3: lay the version out BEFORE the roles are registered, so that their
# binPath (and the logon tasks of a desktop install) point at app\current\bin and
# an update/rollback really replaces the running binaries.
if ($planLayout) {
    Write-Host "[..] laying out version ${layoutVer}: ${source} -> ${installTarget}\app\${layoutVer}"
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $source "installers\install_app_version.ps1") `
        -From $source -Version $layoutVer -Root $installTarget -SetCurrent
    if ($LASTEXITCODE -eq 0 -and (Test-Path (Join-Path $installTarget "app\current\bin\hds.exe"))) {
        $Root = Join-Path $installTarget "app\current"
        $bin = Join-Path $Root "bin"
        $hds = Join-Path $bin "hds.exe"
        Write-Host "[ok] version layout: roles run from $bin"
    } else {
        Write-Host "[!!] version layout failed (exit $LASTEXITCODE) - nothing was registered" -ForegroundColor Red
        exit 1
    }
}
if (-not (Test-Path $hds)) { Write-Host "[!!] hds.exe not found at $hds" -ForegroundColor Red; exit 1 }

$summary = @()
$summary += "[ok] layout: $layoutNote"

# --- State dir + data\. ---
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null

# --- config.yaml (only when missing; never clobber an existing config). ---
if (-not (Test-Path $cfg)) {
    [IO.File]::WriteAllText($cfg, $cfgBody, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "[ok] config created: $cfg"
} else {
    Write-Host "[ok] config exists (not overwritten): $cfg"
}

# --- 0.20.2 (incident 15.10.2026): a stray desktop config.yaml in the install
# root SHADOWS the server config. hds-core::config::config_path() prefers
# <root>\config.yaml over the state dir, so every role silently ran in desktop
# mode: the UI had no auth gate, the watcher wrote another DB, and diagnostics
# reported "heartbeat not found". Rename it aside (reversible).
if (-not $NoConfigPin) {
    # Two places to look for: <root>\config.yaml (no-version install) and
    # <root>\app\current\config.yaml (versioned install: project_root resolves to
    # the `current` junction, so THAT file is the effective one).
    $cands = @(
        (Join-Path $Root "config.yaml"),                     # flat root / app\current
        (Join-Path $installTarget "config.yaml"),            # unpacked tree left in the machine folder
        (Join-Path $installTarget "app\current\config.yaml") # versioned install (project_root = the junction)
    )
    foreach ($rootCfg in $cands) {
        if (-not (Test-Path $rootCfg)) { continue }
        $same = $false
        try { $same = ((Resolve-Path $rootCfg).Path -eq (Resolve-Path $cfg).Path) } catch { $same = $false }
        if ($same) { continue }
        $aside = $rootCfg + ".bak"
        Move-Item $rootCfg $aside -Force
        Write-Host "[ok] shadowing config renamed aside: $aside" -ForegroundColor Yellow
        $summary += "[ok] shadowing config renamed aside (it shadowed the state config): $aside"
    }
}

# --- 0.20.2: machine-wide deps for the SERVICES. They start with a minimal PATH,
# so a per-user winget install of Tesseract/ffmpeg is invisible to them (the UI
# diagnostic then says "not found" even though the component is installed).
if ($SkipDeps) {
    $summary += "[..] deps skipped (-SkipDeps): Tesseract/ffmpeg were not installed"
} elseif (-not (Test-IsAdmin)) {
    $summary += "[!!] deps: run as Administrator to install Tesseract/ffmpeg machine-wide"
} else {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    $haveWinget = [bool](Get-Command winget -ErrorAction SilentlyContinue)
    # Tesseract OCR: machine scope + rus/eng next to the binary. Paths are also
    # written into config.yaml, so the service finds both the binary and the
    # language packs without PATH/TESSDATA_PREFIX (index.ocr_* keys).
    $tess = Join-Path $env:ProgramFiles "Tesseract-OCR\tesseract.exe"
    if (-not (Test-Path $tess) -and $haveWinget) {
        winget install --scope machine -e --id UB-Mannheim.TesseractOCR --accept-source-agreements --accept-package-agreements
    }
    if (Test-Path $tess) {
        $td = Join-Path (Split-Path -Parent $tess) "tessdata"
        New-Item -ItemType Directory -Force -Path $td | Out-Null
        foreach ($lang in @("rus", "eng")) {
            $f = Join-Path $td ("$lang.traineddata")
            if (Test-Path $f) { continue }
            $url = "https://raw.githubusercontent.com/tesseract-ocr/tessdata/main/$lang.traineddata"
            try {
                Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $f -TimeoutSec 300
                $summary += "[ok] tessdata: $lang"
            } catch {
                $summary += "[!!] tessdata ${lang}: $($_.Exception.Message)"
            }
        }
        Set-IndexKey -Path $cfg -Key "ocr_tesseract_cmd" -Value $tess
        Set-IndexKey -Path $cfg -Key "ocr_tessdata" -Value $td
        $summary += "[ok] Tesseract: $tess (tessdata: $td, written into config.yaml)"
    } else {
        $summary += "[..] Tesseract missing: OCR of images/scans stays off (winget --scope machine -e --id UB-Mannheim.TesseractOCR)"
    }
    # ffmpeg: only the legacy Python paths need the CLI (the engine decodes media
    # itself), but installing it machine-wide keeps `hds check` clean everywhere.
    if (Get-Command ffmpeg -ErrorAction SilentlyContinue) {
        $summary += "[ok] ffmpeg: already available"
    } elseif ($haveWinget) {
        winget install --scope machine -e --id Gyan.FFmpeg --accept-source-agreements --accept-package-agreements
        $summary += "[ok] ffmpeg: installed (machine scope)"
    } else {
        $summary += "[..] ffmpeg missing (not needed by the Rust pipeline; legacy Python paths only)"
    }
}

# --- Desktop AtLogOn tasks (roles) are superseded by services. ---
if (-not $KeepAtLogOn) {
    foreach ($task in "HermesDiskSearchWatch", "HermesDiskSearchMcp", "HermesDiskSearchTranscribe", "HermesDiskSearchLlmHost", "HermesDiskSearchUi") {
        schtasks /delete /tn $task /f *> $null
        if ($LASTEXITCODE -eq 0) { Write-Host "[ok] removed AtLogOn task: $task" }
    }
}

# --- Reverse-proxy samples (M3 T1): copy when caddy/nginx is found, else hint. ---
$proxySrc = Join-Path $PSScriptRoot "proxy"
foreach ($px in @("caddy", "nginx")) {
    $file = if ($px -eq "caddy") { "Caddyfile" } else { "nginx.conf" }
    if (Get-Command $px -ErrorAction SilentlyContinue) {
        $dst = Join-Path $state "proxy"
        New-Item -ItemType Directory -Force -Path $dst | Out-Null
        Copy-Item (Join-Path $proxySrc $file) (Join-Path $dst $file) -Force
        Write-Host "[ok] proxy sample copied: $(Join-Path $dst $file)"
    } else {
        Write-Host "[..] $px not found - publish /mcp and /ui via TLS using samples in installers\proxy\"
    }
}

# --- Unattended fetch of runtime/models (idempotent; skip with -NoFetch). ---
if ($Fetch -and -not $NoFetch) {
    foreach ($name in @('fetch_engine_runtime.ps1', 'fetch_llm_models.ps1', 'fetch_whisper_model.ps1', 'fetch_diarization_model.ps1', 'fetch_clip_models.ps1')) {
        $fs = Join-Path $PSScriptRoot $name
        if (-not (Test-Path $fs)) { $summary += "[..] fetch skipped (not found): $name"; continue }
        try {
            & $fs
            if ($null -eq $LASTEXITCODE -or $LASTEXITCODE -eq 0) { $summary += "[ok] fetched: $name" } else { $summary += "[!!] fetch failed ($LASTEXITCODE): $name" }
        } catch {
            $summary += "[!!] fetch error: $name - $($_.Exception.Message)"
        }
    }
}

# --- First admin user (access.enabled): hashed password, plain not logged. ---
if ($EnableAccess -and $AdminLogin -and $AdminPassword) {
    $env:HDS_CONFIG = $cfg
    $env:HDS_ROOT = $state
    & $hds user add --login $AdminLogin --password $AdminPassword --admin *> $null
    $rc = $LASTEXITCODE
    if ($rc -ne 0) { & $hds user passwd --login $AdminLogin --password $AdminPassword *> $null; $rc = $LASTEXITCODE }
    Remove-Item Env:HDS_CONFIG -ErrorAction SilentlyContinue
    Remove-Item Env:HDS_ROOT -ErrorAction SilentlyContinue
    if ($rc -eq 0) { $summary += "[ok] admin user: $AdminLogin (issue a token: bin\hds.exe token new --user $AdminLogin)" } else { $summary += "[!!] admin user failed: $AdminLogin" }
}

# --- Install services per role. ---
foreach ($role in $Roles.Split(",")) {
    $role = $role.Trim()
    if (-not $role) { continue }
    & $hds service install $role *> $null
    if ($LASTEXITCODE -eq 0) {
        $summary += "[ok] service   $role (SCM: hermes-disk-search-$role, auto start)"
        if (-not $NoConfigPin) {
            # Pin the state config AND the runtime locations for THIS service: SCM
            # merges the per-service Environment value on start (machine env is
            # cached by the SCM at boot, so new machine variables do not reach a
            # freshly started service until a reboot). Missing values are skipped.
            $pinPairs = @("HDS_CONFIG=$cfg")
            foreach ($v in @("LLAMA_RUNTIME_DIR", "HDS_ENGINE_DIR", "HDS_EXTRACT_PYTHON", "HDS_MODELS_DIR")) {
                $val = [Environment]::GetEnvironmentVariable($v, "Machine")
                if ($val) { $pinPairs += "$v=$val" }
            }
            $svcName = "hermes-disk-search-$role"
            reg.exe add "HKLM\SYSTEM\CurrentControlSet\Services\$svcName" /v Environment /t REG_MULTI_SZ /d ($pinPairs -join '\0') /f *> $null
            if ($LASTEXITCODE -eq 0) {
                $summary += "[ok]   env pinned for $svcName ($($pinPairs.Count) vars; restart the service to apply)"
            } else {
                $summary += "[!!]   could not pin env for $svcName"
            }
        }
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
if ($EnableAccess -and $AdminLogin) {
    Write-Host "  - Issue an admin token:  `"$hds`" token new --user $AdminLogin"
}
Write-Host "  - Start services now:    `"$hds`" service start <role>"
Write-Host "  - After this install the roles read the state config (HDS_CONFIG is pinned per"
Write-Host "    service); restart a role if it was started earlier: hds service restart <role>"
Write-Host "  - Diagnostics flag a shadowing config root: `"$hds`" check (items config-shadow, layout)"
Write-Host "  - Status:                `"$hds`" service status <role>"
Write-Host "  - Put a TLS reverse proxy in front of /mcp and /ui (facade 8010-8012 stays local):"
Write-Host "    samples in installers\proxy\ (Caddyfile, nginx.conf); then set trusted_proxy: true."

