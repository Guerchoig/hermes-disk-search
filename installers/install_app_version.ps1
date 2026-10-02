# install_app_version.ps1 - install a staged Rust build as a versioned app dir (W4 / MIGRATION_PLAN 10.6).
#
# Layout produced under -Root:
#   app\<ver>\      this version's code + config.yaml (bin\, installers\, sidecar\, ...)
#   app\current     junction -> app\<ver>   (the active version)
#   data\           shared across versions (junction from app\<ver>\data)
#   models\         shared across versions (junction from app\<ver>\models)
#   config.yaml     carried over into the new version (active/root wins)
#
# Why: a running hds.exe/llm_host.exe cannot be overwritten on Windows, so every
# version lives in its own directory and only the `current` pointer is switched.
# The update is done by a SEPARATE process (installers\update.ps1): a process cannot
# replace itself.
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   installers\install_app_version.ps1 -From dist\hds-0.2.0-windows-x64 -Version 0.2.0 -Root D:\hds -SetCurrent
#   installers\install_app_version.ps1 -From hds-0.2.0-windows-x64.zip -Version 0.2.0 -Root D:\hds -Force -SetCurrent
param(
    [Parameter(Mandatory = $true)][string]$From,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Root,
    [switch]$SetCurrent,
    [switch]$Force
)
$ErrorActionPreference = "Stop"

if (-not (Test-Path $Root)) { New-Item -ItemType Directory -Force -Path $Root | Out-Null }
$Root = (Resolve-Path $Root).Path
$app = Join-Path $Root "app"
New-Item -ItemType Directory -Force -Path $app | Out-Null

# --- 1. source: a staged dir or a zip ---------------------------------------
$tmp = $null
if (-not (Test-Path $From)) { throw "source not found: $From" }
$src = (Resolve-Path $From).Path
if ((Get-Item $src).PSIsContainer) {
    $stage = $src
} else {
    $tmp = Join-Path $env:TEMP ("hds-app-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    Write-Host "[..] expanding $src"
    Expand-Archive -Path $src -DestinationPath $tmp -Force
    $stage = $tmp
}

try {
    if (-not (Test-Path (Join-Path $stage "bin\hds.exe"))) {
        throw "not a staged Rust build (bin\hds.exe missing): $stage"
    }

    # --- 2. version directory -----------------------------------------------
    $verDir = Join-Path $app $Version
    if (Test-Path $verDir) {
        if (-not $Force) { throw "version already installed: $verDir (use -Force to replace)" }
        Write-Host "[..] removing existing $verDir"
        Remove-Item -LiteralPath $verDir -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $verDir | Out-Null
    Write-Host "[..] copying $stage -> $verDir"
    Copy-Item (Join-Path $stage "*") $verDir -Recurse -Force

    # --- 3. shared dirs as junctions ----------------------------------------
    foreach ($d in @("data", "models")) {
        $shared = Join-Path $Root $d
        New-Item -ItemType Directory -Force -Path $shared | Out-Null
        $link = Join-Path $verDir $d
        if (Test-Path $link) { continue }
        New-Item -ItemType Junction -Path $link -Target $shared | Out-Null
        Write-Host "[ok] junction $link -> $shared"
    }

    # --- 4. carry config.yaml into the version ------------------------------
    $cfg = Join-Path $verDir "config.yaml"
    if (-not (Test-Path $cfg)) {
        foreach ($cand in @((Join-Path $app "current\config.yaml"),
                            (Join-Path $Root "config.yaml"),
                            (Join-Path $verDir "config.example.yaml"))) {
            if (Test-Path $cand) {
                Copy-Item $cand $cfg -Force
                Write-Host "[ok] config.yaml from $cand"
                break
            }
        }
    }

    # --- 4b. db_path hint for versioned installs ----------------------------
    # A relative db_path resolves against the *version* dir, so the index would be
    # recreated per version. Correct choices: an absolute path, or data\index.db
    # (data\ is the shared junction).
    if (Test-Path $cfg) {
        $text = [IO.File]::ReadAllText($cfg)
        if ($text -match '(?m)^db_path:\s*[''"]?(?![A-Za-z]:)(?!data[\\/])') {
            Write-Host "[!!] db_path is relative and outside data\ - it will be per-version; use data\index.db" -ForegroundColor Yellow
        }
    }

    # --- 5. switch the current pointer --------------------------------------
    if ($SetCurrent) {
        $curLink = Join-Path $app "current"
        if (Test-Path $curLink) {
            # rmdir removes the junction itself, never the target
            cmd /c rmdir "$curLink" | Out-Null
        }
        New-Item -ItemType Junction -Path $curLink -Target $verDir | Out-Null
        Write-Host "[ok] current -> $verDir"
    }

    Write-Host "[ok] installed version ${Version} -> $verDir"
} finally {
    if ($tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue }
}
