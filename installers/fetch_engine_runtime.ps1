# fetch_engine_runtime.ps1 - download/unpack the Openresearchtools engine runtime (W4).
#
# The engine runtime is the GPU/CPU backend used by the resident llm-host (chat,
# embedding, rerank) and by ASR (whisper). llama.cpp/llama-server is NOT installed
# anymore: the engine supplies the same bridge/cluster API as a downloadable build.
#
# The asset is selected from engine-manifest.json by platform and backend, verified
# by sha256 and unpacked into the engine directory (the folder searched by
# hds-llama::engine_dir: index.whisper_engine_dir -> %APPDATA%\OpenResearchTools\
# TranscribeOffline\Engine -> next to the executable). Idempotent: a second run with
# the same manifest tag/sha256 does nothing.
#
# ASCII-only on purpose: Windows PowerShell 5.1 reads a BOM-less .ps1 as ANSI and
# mangles non-ASCII text (tools/parity/README.md section 3, item 14).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_engine_runtime.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_engine_runtime.ps1 -Backend vulkan -Force
param(
    [string]$Manifest = "",
    [string]$EngineDir = "",
    [string]$Backend = "auto",   # auto | cuda | vulkan | metal | cpu
    [switch]$Force,
    [switch]$NoUnblock,
    # HDS engine patch overlay (engine-patch/README.md): apply or roll back our
    # patched DLLs (P1/P2/P3 + KV cache type) over the stock runtime.
    [switch]$PatchEngine,
    [switch]$RollbackEnginePatch,
    [string]$PatchManifest = ""
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if (-not $Manifest) { $Manifest = Join-Path $root "runtime-manifests\engine-manifest.json" }
if (-not (Test-Path $Manifest)) { throw "engine manifest not found: $Manifest" }

# --- platform + library name ------------------------------------------------
$plat = "windows-x64"
$libName = "multi-node-server.dll"
if ($env:OS -ne "Windows_NT") {
    $plat = "macos-arm64"
    $libName = "libmulti-node-server.dylib"
}

# --- engine directory -------------------------------------------------------
if (-not $EngineDir) {
    if ($env:HDS_ENGINE_DIR) {
        $EngineDir = $env:HDS_ENGINE_DIR
    } elseif ($plat -eq "macos-arm64") {
        $EngineDir = Join-Path $HOME "Library/Application Support/OpenResearchTools/TranscribeOffline/Engine"
    } else {
        $EngineDir = Join-Path $env:APPDATA "OpenResearchTools\TranscribeOffline\Engine"
    }
}

# --- HDS engine patch (overlay over the stock runtime) -----------------------
# Our patch (engine-patch/README.md): P1 bounded slot wait, P2 model load outside
# the instance lock (the hang we hit), P3 lock order, KV cache type + flash
# attention for quantized V. Files and sha256 live in
# runtime-manifests/engine-patch.json; stock files are kept next to them as *.orig
# so that -RollbackEnginePatch restores the upstream runtime.
if ($RollbackEnginePatch -or $PatchEngine) {
    $patchMf = $PatchManifest
    if (-not $patchMf) { $patchMf = Join-Path $root "runtime-manifests\engine-patch.json" }

    if ($RollbackEnginePatch) {
        $orig = @(Get-ChildItem -LiteralPath $EngineDir -Filter "*.orig" -File -ErrorAction SilentlyContinue)
        if ($orig.Count -eq 0) {
            Write-Host "[patch] no *.orig backups in $EngineDir - nothing to roll back"
            exit 0
        }
        foreach ($o in $orig) {
            $target = Join-Path $EngineDir ($o.Name.Substring(0, $o.Name.Length - 5))
            Copy-Item -LiteralPath $o.FullName -Destination $target -Force
            Remove-Item -LiteralPath $o.FullName -Force
        }
        Write-Host "[ok] engine patch rolled back: $($orig.Count) file(s) restored from *.orig"
        exit 0
    }

    if (-not (Test-Path $patchMf)) { throw "engine patch manifest not found: $patchMf" }
    $pm = Get-Content -Raw -Path $patchMf | ConvertFrom-Json
    if (-not (Test-Path (Join-Path $EngineDir $libName))) {
        Write-Host "[patch] engine runtime is missing in $EngineDir - installing it first"
    } else {
        Write-Host "[patch] applying HDS engine patch (base $($pm.base_tag)) to $EngineDir"
        $tmpPatch = Join-Path ([System.IO.Path]::GetTempPath()) ("hds-engine-patch-" + [Guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Force -Path $tmpPatch | Out-Null
        try {
            $ProgressPreference = "SilentlyContinue"
            foreach ($f in @($pm.files)) {
                $dst = Join-Path $tmpPatch $f.file_name
                Write-Host "[..] $($f.file_name)"
                try {
                    Invoke-WebRequest -Uri $f.url -OutFile $dst -UseBasicParsing -TimeoutSec 3600
                } catch {
                    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
                        & curl.exe -L --fail -o $dst $f.url
                        if ($LASTEXITCODE -ne 0) { throw "curl failed with exit code $LASTEXITCODE" }
                    } else {
                        throw
                    }
                }
                $got = (Get-FileHash $dst -Algorithm SHA256).Hash.ToLower()
                $want = ([string]$f.sha256).Trim().ToLower()
                if ($got -ne $want) {
                    throw "sha256 mismatch for $($f.file_name): expected $want, got $got"
                }
                $target = Join-Path $EngineDir $f.file_name
                if ((Test-Path $target) -and -not (Test-Path "$target.orig")) {
                    Copy-Item -LiteralPath $target -Destination "$target.orig" -Force
                }
                Copy-Item -LiteralPath $dst -Destination $target -Force
                if (-not $NoUnblock) {
                    try { Unblock-File -LiteralPath $target -ErrorAction Stop } catch { }
                }
            }
            Write-Host "[ok] engine patch applied: $($pm.files.Count) file(s) (originals kept as *.orig)"
            Write-Host "[i] rollback: fetch_engine_runtime.ps1 -RollbackEnginePatch"
            exit 0
        } finally {
            Remove-Item -LiteralPath $tmpPatch -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

# --- backend: explicit or auto (NVIDIA -> cuda, otherwise vulkan/metal) ------
$backend = $Backend.Trim().ToLower()
if ($backend -eq "" -or $backend -eq "auto") {
    if ($plat -eq "macos-arm64") {
        $backend = "metal"
    } elseif (Get-Command nvidia-smi -ErrorAction SilentlyContinue) {
        & nvidia-smi *> $null
        if ($LASTEXITCODE -eq 0) { $backend = "cuda" } else { $backend = "vulkan" }
    } else {
        $backend = "vulkan"
    }
}

$mf = Get-Content $Manifest -Raw -Encoding UTF8 | ConvertFrom-Json
$assets = @($mf.assets | Where-Object { $_.platform -eq $plat })
if ($assets.Count -eq 0) {
    throw "manifest '$Manifest' (tag $($mf.tag)) has no assets for platform '$plat'"
}
$asset = $assets | Where-Object { $_.backend -eq $backend } | Select-Object -First 1
if (-not $asset) {
    $avail = ($assets | ForEach-Object { $_.backend }) -join ", "
    throw "backend '$backend' is not available for '$plat' (tag $($mf.tag)); available: $avail"
}

Write-Host "[..] engine runtime: tag $($mf.tag), backend $backend"
Write-Host "     target: $EngineDir"

# --- idempotency ------------------------------------------------------------
$lib = Join-Path $EngineDir $libName
$stamp = Join-Path $EngineDir ".hds-engine.json"
$haveSha = ""
if (Test-Path $stamp) {
    try { $haveSha = [string](Get-Content $stamp -Raw -Encoding UTF8 | ConvertFrom-Json).sha256 } catch { $haveSha = "" }
}
# An existing runtime without our stamp (e.g. installed by the engine UI) is trusted:
# only -Force re-fetches it. A stamp with a different sha means the manifest moved on.
if ((-not $Force) -and (Test-Path $lib)) {
    if ($haveSha -eq $asset.sha256) {
        Write-Host "[ok] engine runtime already present ($($mf.tag)/$backend)"
        exit 0
    }
    if ($haveSha -eq "") {
        Write-Host "[ok] engine runtime already present (no manifest stamp; use -Force to re-fetch)"
        exit 0
    }
    Write-Host "[..] engine runtime differs from the manifest - re-fetching"
}

# --- download ---------------------------------------------------------------
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("hds-engine-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$archive = Join-Path $tmp $asset.file_name
$ProgressPreference = "SilentlyContinue"
try {
    Write-Host "[..] downloading $($asset.file_name) ..."
    try {
        Invoke-WebRequest -Uri $asset.url -OutFile $archive -UseBasicParsing -TimeoutSec 3600
    } catch {
        if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
            & curl.exe -L --fail --progress-bar -o $archive $asset.url
            if ($LASTEXITCODE -ne 0) { throw "curl failed with exit code $LASTEXITCODE" }
        } else {
            throw
        }
    }

    # --- verify sha256 ------------------------------------------------------
    $got = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
    $want = ([string]$asset.sha256).Trim().ToLower()
    if ($got -ne $want) {
        throw "sha256 mismatch for $($asset.file_name): expected $want, got $got"
    }
    Write-Host "[ok] sha256 verified"

    # --- unpack -------------------------------------------------------------
    $stage = Join-Path $tmp "unpacked"
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    if (([string]$asset.archive -eq "tar.gz") -or ($asset.file_name -like "*.tar.gz")) {
        & tar -xzf $archive -C $stage
        if ($LASTEXITCODE -ne 0) { throw "tar failed with exit code $LASTEXITCODE" }
    } else {
        Expand-Archive -Path $archive -DestinationPath $stage -Force
    }
    # the archive may wrap everything in a single folder - flatten it
    $top = @(Get-ChildItem -Force $stage)
    if ($top.Count -eq 1 -and $top[0].PSIsContainer) {
        $nested = $top[0].FullName
        Get-ChildItem -Force $nested | ForEach-Object {
            Move-Item -LiteralPath $_.FullName -Destination (Join-Path $stage $_.Name) -Force
        }
        Remove-Item -LiteralPath $nested -Recurse -Force -ErrorAction SilentlyContinue
    }

    # --- install (replace the runtime directory) ----------------------------
    if (Test-Path $EngineDir) {
        try {
            Remove-Item -LiteralPath $EngineDir -Recurse -Force -ErrorAction Stop
        } catch {
            throw "cannot replace '$EngineDir' (files are in use). Stop the resident first: bin\llm_host.exe stop, then retry."
        }
    }
    New-Item -ItemType Directory -Force -Path $EngineDir | Out-Null
    Get-ChildItem -Force $stage | ForEach-Object {
        Move-Item -LiteralPath $_.FullName -Destination (Join-Path $EngineDir $_.Name) -Force
    }

    $stampObj = [pscustomobject]@{
        tag        = $mf.tag
        backend    = $backend
        sha256     = $asset.sha256
        file_name  = $asset.file_name
        fetched_at = (Get-Date -Format s)
    }
    [IO.File]::WriteAllText($stamp, ($stampObj | ConvertTo-Json), (New-Object System.Text.UTF8Encoding($false)))

    # --- clear Mark-of-the-Web on the unpacked runtime ----------------------
    if (-not $NoUnblock) {
        Get-ChildItem -LiteralPath $EngineDir -Recurse -File -Force | ForEach-Object {
            try { Unblock-File -LiteralPath $_.FullName -ErrorAction Stop } catch { }
        }
    }
    Write-Host "[ok] engine runtime installed: $EngineDir"
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
