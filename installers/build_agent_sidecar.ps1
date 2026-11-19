# build_agent_sidecar.ps1 - assemble the Node agent sidecar (A3.5, TR-3.1).
#
# Produces the runtime tree used by `hds agent`:
#   sidecar\
#     node\                 - portable Node (no spaces in the path: the MCP `command`
#                             is not quoted, so `C:\Program Files\nodejs` is unusable)
#     hds_agent\
#       package.json, package-lock.json, tsconfig.json
#       src\, test\         - TypeScript sources and the node:test suite
#       dist\worker.mjs     - built worker (esbuild, ESM, no minification)
#       node_modules\       - lock-pinned dependencies (runtime only in a dist tree)
#
# Node comes from runtime-manifests\node-manifest.json (official nodejs.org
# archive + sha256); the archive is verified before it is unpacked. -NodeSource
# copies an existing installation instead (offline machines, `C:\Program
# Files\nodejs`): the copy is what removes the space from the path.
#
# The build needs esbuild/typescript (devDependencies). In a distribution tree
# (OutDir != repo\sidecar) they are pruned after the build, so the shipped
# node_modules holds runtime dependencies only (@cline/sdk for the worker,
# tavily-mcp for the MCP server command hds starts). The repository tree keeps
# dev dependencies by default (-Prune also prunes in place, -KeepDev keeps them
# in a distribution tree).
#
# -SelfTest speaks the section 4.3 handshake with dist\worker.mjs
# (hello -> ready {protocol:1, node, sdk}), checks that @cline/sdk is importable
# and that node_modules\tavily-mcp\build\index.js (the path hds passes to the
# MCP client) is present.
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_agent_sidecar.ps1 -SelfTest
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_agent_sidecar.ps1 `
#       -OutDir dist\hds-1.0-windows-x64\sidecar -SelfTest
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_agent_sidecar.ps1 `
#       -NodeSource "C:\Program Files\nodejs" -SkipInstall -SelfTest
param(
    [string]$OutDir = "",
    [string]$Manifest = "",
    [string]$NodeSource = "",
    [switch]$Force,
    [switch]$SelfTest,
    [switch]$KeepDev,
    [switch]$Prune,
    [switch]$SkipInstall,
    [switch]$SkipBuild
)
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $OutDir) { $OutDir = Join-Path $root "sidecar" }
if (-not [System.IO.Path]::IsPathRooted($OutDir)) { $OutDir = Join-Path $root $OutDir }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (-not $Manifest) { $Manifest = Join-Path $root "runtime-manifests\node-manifest.json" }
if (-not (Test-Path $Manifest)) { throw "node manifest not found: $Manifest" }

$repoSidecar = Join-Path $root "sidecar"
$inPlace = $false
try { $inPlace = ((Resolve-Path $OutDir).Path -eq (Resolve-Path $repoSidecar).Path) } catch { $inPlace = $false }

$plat = "windows-x64"
if ($env:OS -ne "Windows_NT") { $plat = "macos-arm64" }

$mf = Get-Content -Raw -Path $Manifest | ConvertFrom-Json
$asset = @($mf.assets | Where-Object { $_.platform -eq $plat }) | Select-Object -First 1
if (-not $asset) { throw "node manifest has no asset for platform $plat ($Manifest)" }
$tag = [string]$mf.tag
$wantSha = ([string]$asset.sha256).Trim().ToLower()
$wantFile = [string]$asset.file_name
$nodeRoot = [string]$asset.root
$exeRel = [string]$asset.exe
$npmRel = [string]$asset.npm
if ($wantSha -notmatch "^[0-9a-f]{64}$") { throw "manifest sha256 for $($asset.id) is not a sha256: $wantSha" }

$nodeDir = Join-Path $OutDir "node"
$nodeExe = Join-Path $nodeDir $exeRel
$agentDir = Join-Path $OutDir "hds_agent"
$workerScript = Join-Path $agentDir "dist\worker.mjs"
$nodeMarker = Join-Path $nodeDir ".hds-node-version"

Write-Host "[a3.5] agent sidecar: $OutDir (platform $plat, node $tag)"
Write-Host "[a3.5] repository tree: $inPlace"

# --- 1. sources into the target tree ---------------------------------------
if (-not $inPlace) {
    New-Item -ItemType Directory -Force -Path $agentDir | Out-Null
    foreach ($name in @("package.json", "package-lock.json", "tsconfig.json")) {
        $src = Join-Path $repoSidecar "hds_agent\$name"
        if (-not (Test-Path $src)) { throw "missing file: $src" }
        Copy-Item -LiteralPath $src -Destination (Join-Path $agentDir $name) -Force
    }
    foreach ($dir in @("src", "test")) {
        $src = Join-Path $repoSidecar "hds_agent\$dir"
        if (-not (Test-Path $src)) { throw "missing directory: $src" }
        $dst = Join-Path $agentDir $dir
        if ($Force -and (Test-Path $dst)) { Remove-Item -LiteralPath $dst -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $dst | Out-Null
        Copy-Item (Join-Path $src "*") $dst -Recurse -Force
    }
    Write-Host "[ok] sources copied to $agentDir"
}

# --- 2. portable Node ------------------------------------------------------
function Expand-NodeArchive {
    # Unpacks the official archive into $Destination (must not exist yet).
    param([string]$Archive, [string]$Destination, [string]$RootName, [string]$Kind)
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("hds-node-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    try {
        if ($Kind -eq "zip") {
            Expand-Archive -LiteralPath $Archive -DestinationPath $tmp -Force
        } else {
            & tar -xzf $Archive -C $tmp
            if ($LASTEXITCODE -ne 0) { throw "tar failed (exit $LASTEXITCODE) for $Archive" }
        }
        $src = Join-Path $tmp $RootName
        if (-not (Test-Path $src)) { throw "unexpected archive layout: $src not found" }
        Move-Item -LiteralPath $src -Destination $Destination
    } finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$haveNode = $false
if ((Test-Path $nodeExe) -and -not $Force) {
    if (Test-Path $nodeMarker) {
        $haveNode = ((Get-Content -LiteralPath $nodeMarker -Raw).Trim() -eq $tag)
    } else {
        $haveNode = $true
    }
}
if ($haveNode) {
    Write-Host "[ok] portable Node already present: $nodeExe"
} elseif ($NodeSource) {
    if (Test-Path $nodeDir) { Remove-Item -LiteralPath $nodeDir -Recurse -Force }
    if (Test-Path -PathType Container $NodeSource) {
        Write-Host "[..] copying Node from $NodeSource"
        New-Item -ItemType Directory -Force -Path $nodeDir | Out-Null
        Copy-Item (Join-Path $NodeSource "*") $nodeDir -Recurse -Force
    } else {
        Write-Host "[i] -NodeSource is a single file: only the interpreter is copied"
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $nodeExe) | Out-Null
        Copy-Item -LiteralPath $NodeSource -Destination $nodeExe -Force
    }
} else {
    if (Test-Path $nodeDir) { Remove-Item -LiteralPath $nodeDir -Recurse -Force }
    $archive = Join-Path ([System.IO.Path]::GetTempPath()) ("hds-" + $wantFile)
    Write-Host "[..] downloading $($asset.url)"
    try {
        Invoke-WebRequest -Uri $asset.url -OutFile $archive -UseBasicParsing -TimeoutSec 3600
    } catch {
        if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
            & curl.exe -L --fail -o $archive $asset.url
            if ($LASTEXITCODE -ne 0) { throw "curl failed with exit code $LASTEXITCODE" }
        } else {
            throw
        }
    }
    try {
        $got = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLower()
        if ($got -ne $wantSha) {
            throw "sha256 mismatch for ${wantFile}: expected $wantSha, got $got"
        }
        Write-Host "[ok] archive verified ($wantFile)"
        Expand-NodeArchive -Archive $archive -Destination $nodeDir -RootName $nodeRoot -Kind $asset.archive
    } finally {
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
    }
}

if (-not (Test-Path $nodeExe)) { throw "node executable not found: $nodeExe" }
$nodeVersion = (& $nodeExe --version).Trim()
$nodeMajor = 0
if ($nodeVersion -match "^v(\d+)\.") { $nodeMajor = [int]$Matches[1] }
if ($nodeMajor -lt 22) { throw "Node $nodeVersion is too old: package.json engines require >=22.18" }
Set-Content -LiteralPath $nodeMarker -Encoding ASCII -Value $tag
if ($haveNode) {
    Write-Host "[ok] Node $nodeVersion -> $nodeDir (already installed)"
} else {
    Write-Host "[ok] Node $nodeVersion -> $nodeDir"
}


# --- 3. dependencies (npm ci, lock-pinned) ---------------------------------
$npmCli = Join-Path $nodeDir $npmRel
$lock = Join-Path $agentDir "package-lock.json"
if (-not (Test-Path $lock)) {
    throw "package-lock.json not found: $lock (the lock file must be committed, TR-3.1)"
}
$lockHash = (Get-FileHash -LiteralPath $lock -Algorithm SHA256).Hash.ToLower()

# A distribution tree ships runtime dependencies only; the repository tree keeps
# its dev dependencies (npm run build / npm test keep working after installing).
$shouldPrune = $false
if (-not $KeepDev) { $shouldPrune = ($Prune -or (-not $inPlace)) }
$mode = if ($shouldPrune) { "prod" } else { "dev" }
$markerText = ($lockHash + "`n" + $mode).Trim()

$depsMarker = Join-Path $agentDir "node_modules\.hds-agent-lock"
$sdkPkg = Join-Path $agentDir "node_modules\@cline\sdk\package.json"
$esbuildDir = Join-Path $agentDir "node_modules\esbuild"
$needBuild = (-not $SkipBuild) -and ($Force -or -not (Test-Path $workerScript))
$needInstall = $Force -or -not (Test-Path $sdkPkg)
if (-not $needInstall) {
    $old = ""
    if (Test-Path $depsMarker) { $old = (Get-Content -LiteralPath $depsMarker -Raw).Trim() }
    if ($old -ne $markerText) { $needInstall = $true }
}
if ($needBuild -and -not (Test-Path $esbuildDir) -and -not $SkipInstall) {
    # A pruned tree cannot build: esbuild/typescript are dev dependencies.
    $needInstall = $true
}

$oldPath = $env:PATH
$env:PATH = $nodeDir + [System.IO.Path]::PathSeparator + $env:PATH
try {
    if ($needInstall) {
        if ($SkipInstall) {
            throw "node_modules is missing or stale in ${agentDir} (-SkipInstall was given)"
        }
        if (-not (Test-Path $npmCli)) {
            throw "npm not found in the portable Node tree ($npmCli): use the full archive or -NodeSource <node dir>"
        }
        Write-Host "[..] npm ci (lock $($lockHash.Substring(0, 12)), $mode)"
        Push-Location $agentDir
        try { & $nodeExe $npmCli ci --no-audit --no-fund } finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw "npm ci failed (exit $LASTEXITCODE)" }
        Write-Host "[ok] dependencies installed"
    } else {
        Write-Host "[ok] node_modules matches the lock (marker $mode)"
    }

    # --- 4. build ----------------------------------------------------------
    if ($needBuild) {
        if (-not (Test-Path $esbuildDir)) {
            throw "esbuild is missing in ${agentDir}\node_modules (dev dependencies pruned): rerun with -Force"
        }
        Write-Host "[..] npm run build (esbuild -> dist\worker.mjs)"
        Push-Location $agentDir
        try { & $nodeExe $npmCli run build } finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw "npm run build failed (exit $LASTEXITCODE)" }
    }
    if (-not (Test-Path $workerScript)) { throw "worker not built: $workerScript" }
    $workerSize = (Get-Item -LiteralPath $workerScript).Length
    if ($needBuild) {
        Write-Host "[ok] worker built: $workerScript ($([int]($workerSize / 1024)) KiB)"
    } else {
        Write-Host "[ok] worker already built: $workerScript (-Force rebuilds)"
    }

    # --- 5. prune (distribution tree) --------------------------------------
    if ($shouldPrune -and ($needInstall -or $Force)) {
        Write-Host "[..] npm prune --omit=dev (runtime dependencies only)"
        Push-Location $agentDir
        try { & $nodeExe $npmCli prune --omit=dev --no-audit --no-fund } finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw "npm prune failed (exit $LASTEXITCODE)" }
        Write-Host "[ok] dev dependencies pruned"
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $depsMarker) | Out-Null
    Set-Content -LiteralPath $depsMarker -Encoding ASCII -Value $markerText
} finally {
    $env:PATH = $oldPath
}


# --- 6. self-test: hello over the section 4.3 protocol ---------------------
function Test-AgentWorker {
    param(
        [Parameter(Mandatory = $true)][string]$NodeExe,
        [Parameter(Mandatory = $true)][string]$AgentDir,
        [Parameter(Mandatory = $true)][string]$NodeDir
    )
    $worker = Join-Path $AgentDir "dist\worker.mjs"
    if (-not (Test-Path $worker)) { throw "self-test: worker not found: $worker" }
    if ($NodeDir -match "\s") {
        throw "self-test: the portable Node path contains a space (${NodeDir}): the MCP command is not quoted"
    }

    # @cline/sdk must be importable, not merely present (a prune may cut it).
    Push-Location $AgentDir
    try {
        & $NodeExe -e "import('@cline/sdk').then((m) => { if (!m.ClineCore) { console.error('ClineCore export missing'); process.exit(2); } console.log('sdk-ok'); }).catch((e) => { console.error(String((e && e.message) || e)); process.exit(3); })"
        if ($LASTEXITCODE -ne 0) { throw "self-test: import('@cline/sdk') failed (exit $LASTEXITCODE)" }
    } finally {
        Pop-Location
    }
    $tavily = Join-Path $AgentDir "node_modules\tavily-mcp\build\index.js"
    if (-not (Test-Path $tavily)) {
        throw "self-test: tavily-mcp is missing: $tavily (hds passes this script to the MCP client)"
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $NodeExe
    $psi.Arguments = '"' + $worker + '"'
    $psi.WorkingDirectory = $AgentDir
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables["CLINE_DATA_DIR"] = (Join-Path $AgentDir ".cline\data")
    $proc = [System.Diagnostics.Process]::Start($psi)
    try {
        $proc.StandardInput.WriteLine('{"type":"hello","id":"1","protocol":1}')
        $proc.StandardInput.Flush()
        $read = $proc.StandardOutput.ReadLineAsync()
        if (-not $read.Wait(60000)) { throw "self-test: hello got no answer in 60 s" }
        $line = $read.Result
        if (-not $line) { throw "self-test: worker closed the channel without an answer" }
        $msg = $line | ConvertFrom-Json
        if ($msg.type -ne "ready") { throw "self-test: expected ready, got: $line" }
        if ([int]$msg.protocol -ne 1) { throw "self-test: unexpected protocol $($msg.protocol): $line" }
        if (-not $msg.sdk) {
            throw "self-test: ready.sdk is empty (SDK package not found next to dist\worker.mjs)"
        }
        Write-Host "[ok] hello: ready {protocol=1, node=$($msg.node), sdk=$($msg.sdk)}"
        $proc.StandardInput.WriteLine('{"type":"shutdown","id":"2"}')
        $proc.StandardInput.Flush()
        if (-not $proc.WaitForExit(15000)) { throw "self-test: worker did not exit after shutdown" }
    } finally {
        if (-not $proc.HasExited) { try { $proc.Kill() } catch { } }
        $proc.Dispose()
    }
}

if ($SelfTest) {
    Write-Host "[..] self-test: hello over the protocol (section 4.3)"
    Test-AgentWorker -NodeExe $nodeExe -AgentDir $agentDir -NodeDir $nodeDir
}

Write-Host ""
Write-Host "[a3.5] agent sidecar ready"
Write-Host "  node:   $nodeExe"
Write-Host "  worker: $workerScript"
Write-Host "  deps:   $agentDir\node_modules ($mode)"
Write-Host "  hds resolves agent.node.path -> <root>\sidecar\node\node.exe -> PATH"

