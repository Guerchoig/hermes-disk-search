# build_sidecar.ps1 - assemble a self-contained Python sidecar tree (W4).
#
# Produces a `sidecar\` directory that works WITHOUT the project's Python core:
#   sidecar\
#     hds_extract\worker.py            - the stdio JSON-RPC worker
#     hds_extract\requirements.lock    - worker dependencies
#     python\                          - portable CPython (python-build-standalone) + deps
#     hds\                             - extractor/lemmatizer modules the worker needs
#     README.md
#
# `hds\` is a BUILD-TIME COPY of the source-of-truth modules in the repo `hds\`
# (the Python version stays the source of truth - no sources are moved). The copy is
# self-contained: intra-package imports are relative, and all heavy libraries are
# imported lazily inside functions. worker.py puts this directory on sys.path, so
# `import hds.*` resolves here in a shipped install and falls back to the project
# root in development.
#
# Requires `uv` (https://docs.astral.sh/uv/). Heavy: downloads CPython (~30 MB) and
# the worker wheels (~250 MB).
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir sidecar
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_sidecar.ps1 -OutDir dist\sidecar -SelfTest
param(
    [string]$OutDir = "",
    [string]$PythonVersion = "3.12",
    [switch]$Force,
    [switch]$SelfTest
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $OutDir) { $OutDir = Join-Path $root "sidecar" }
if (-not [System.IO.Path]::IsPathRooted($OutDir)) { $OutDir = Join-Path $root $OutDir }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    throw "uv not found in PATH (https://docs.astral.sh/uv/)"
}

$pyDir = Join-Path $OutDir "python"
$pkgDir = Join-Path $OutDir "hds_extract"
$hdsDir = Join-Path $OutDir "hds"
New-Item -ItemType Directory -Force -Path $pkgDir, $hdsDir | Out-Null

# --- 1. portable CPython ----------------------------------------------------
$pyMarker = Join-Path $pyDir ".hds-python"
if ($Force -or -not (Test-Path $pyMarker)) {
    Write-Host "[..] uv python install $PythonVersion --install-dir $pyDir"
    & uv python install $PythonVersion --install-dir $pyDir
    if ($LASTEXITCODE -ne 0) { throw "uv python install failed" }
    New-Item -ItemType File -Force -Path $pyMarker | Out-Null
}
$py = Get-ChildItem -Path $pyDir -Recurse -Filter python.exe -ErrorAction SilentlyContinue |
    Sort-Object { $_.FullName.Length } | Select-Object -First 1
if (-not $py) { throw "python.exe not found under $pyDir" }

# --- 2. worker + lock + readme ---------------------------------------------
Copy-Item (Join-Path $root "sidecar\hds_extract\worker.py") $pkgDir -Force
Copy-Item (Join-Path $root "sidecar\hds_extract\requirements.lock") $pkgDir -Force
Copy-Item (Join-Path $root "sidecar\README.md") $OutDir -Force

# --- 3. self-contained copy of the extractor/lemmatizer modules -------------
$mods = @("__init__.py", "config.py", "extractors.py", "extract_av.py",
    "extract_static.py", "lemmatizer.py", "whisper_cpp.py")
foreach ($m in $mods) {
    $src = Join-Path $root "hds\$m"
    if (-not (Test-Path $src)) { throw "missing module: $src" }
    Copy-Item $src $hdsDir -Force
}

# --- 4. worker dependencies into the portable interpreter -------------------
$depsMarker = Join-Path $pyDir ".hds-deps"
if ($Force -or -not (Test-Path $depsMarker)) {
    Write-Host "[..] uv pip install worker dependencies..."
    # uv-управляемый CPython помечен как "externally managed" - без этого флага
    # `uv pip install --python <portable>` отказывается ставить пакеты.
    & uv pip install --python $py.FullName --break-system-packages -r (Join-Path $pkgDir "requirements.lock")
    if ($LASTEXITCODE -ne 0) { throw "uv pip install failed" }
    New-Item -ItemType File -Force -Path $depsMarker | Out-Null
}

# --- 5. self-test: the copied package must import without the project core ---
if ($SelfTest) {
    $code = "import sys; sys.path.insert(0, r'$OutDir'); " +
        "import hds.config, hds.extractors, hds.extract_av, hds.extract_static, " +
        "hds.lemmatizer, hds.whisper_cpp; print('sidecar-ok')"
    Write-Host "[..] self-test: importing the copied hds package from $OutDir"
    & $py.FullName -c $code
    if ($LASTEXITCODE -ne 0) { throw "sidecar self-test failed" }
}

Write-Host "[ok] sidecar assembled: $OutDir"
