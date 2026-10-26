# build_anonymize_sidecar.ps1 - assemble the anonymization sidecar venv (P6).
#
# Creates the profile venv from the bundled CPython and installs the pinned
# requirements of sidecar\hds_anonymize\requirements.lock:
#   -Profile cpu  -> sidecar\venv-anonymize  (onnxruntime 1.29.0)
#   -Profile cuda -> sidecar\venv-anonymize  (onnxruntime-gpu 1.24.4)
#
# Only ONE onnxruntime distribution may live in the venv: `onnxruntime` and
# `onnxruntime-gpu` both provide the `onnxruntime` module. For the cuda profile
# the CPU pin is filtered out of the lock and onnxruntime-gpu is installed
# explicitly (requirements.lock documents the exact versions).
#
# The script is idempotent: a matching marker (.hds-anonymize-profile with the
# profile, python and lock hash) means "nothing to do". -Force rebuilds the venv.
#
# Heavy: the lock pulls torch (imported by gliner even for the ONNX backend),
# transformers, natasha and the Office parsers (~3-6 GB).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_anonymize_sidecar.ps1 -Profile cpu -SelfTest
#   ... -Profile cuda -OutDir dist\hds-1.0-windows-x64\sidecar
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("cpu", "cuda")]
    [string]$Profile,
    [string]$OutDir = "",
    [string]$PythonVersion = "3.12",
    [switch]$Force,
    [switch]$SelfTest,
    [switch]$NoCopy
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "sidecar_common.ps1")

$root = Get-SidecarRoot -ScriptRoot $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $root "sidecar" }
if (-not [System.IO.Path]::IsPathRooted($OutDir)) { $OutDir = Join-Path $root $OutDir }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$venvName = "venv-anonymize"
$srcSidecar = Join-Path $root "sidecar"
$lockPath = Join-Path $srcSidecar "hds_anonymize\requirements.lock"
$ortCpu = "onnxruntime==1.29.0"
$ortGpu = "onnxruntime-gpu==1.24.4"
$ortPkg = if ($Profile -eq "cuda") { $ortGpu } else { $ortCpu }

Write-Host "[p6] anonymizer profile: $Profile (venv $venvName)"
$portable = Ensure-PortablePython -SidecarDir $OutDir -PythonVersion $PythonVersion -Force:$Force
Write-Host "[p6] CPython: $portable"

# --- 1. package files in the target tree ------------------------------------
if (-not $NoCopy -and ((Resolve-Path $OutDir).Path -ne (Resolve-Path $srcSidecar).Path)) {
    Copy-SidecarTree -Source $srcSidecar -Destination $OutDir
    $srcPkg = Join-Path $srcSidecar "hds_anonymize"
    $dstPkg = Join-Path $OutDir "hds_anonymize"
    New-Item -ItemType Directory -Force -Path $dstPkg | Out-Null
    Copy-Item (Join-Path $srcPkg "*") $dstPkg -Recurse -Force
    Get-ChildItem -Path $dstPkg -Recurse -Directory -Filter "__pycache__" -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}
$venv = Join-Path $OutDir $venvName
$py = Join-Path $venv "Scripts\python.exe"

# --- 2. marker: profile + lock hash -----------------------------------------
$lockHash = (Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash.ToLower()
$marker = Join-Path $venv ".hds-anonymize-profile"
$markerText = "profile=$Profile`npython=$PythonVersion`nlock=requirements.lock`nsha256=$lockHash`nort=$ortPkg"
$upToDate = $false
if (-not $Force -and (Test-Path $py) -and (Test-Path $marker)) {
    $old = (Get-Content -LiteralPath $marker -Raw).Trim()
    $upToDate = ($old -eq $markerText.Trim())
}
if ($upToDate) {
    Write-Host "[ok] venv $venvName is up to date (marker matches)"
} else {
    $py = New-SidecarVenv -SidecarDir $OutDir -Name $venvName -PortablePython $portable -Force:$Force

    # --- 3. lock (cuda: without the CPU onnxruntime pin) --------------------
    $tmpLock = Join-Path $env:TEMP ("hds-anon-lock-" + [Guid]::NewGuid().ToString("N") + ".txt")
    try {
        $lines = Get-Content -LiteralPath $lockPath
        if ($Profile -eq "cuda") {
            $lines = $lines | Where-Object { $_ -notmatch '^\s*onnxruntime\s*==' }
        }
        [System.IO.File]::WriteAllLines($tmpLock, $lines,
            (New-Object System.Text.UTF8Encoding($false)))
        Install-SidecarRequirements -VenvPython $py -RequirementsPath $tmpLock
    } finally {
        Remove-Item -LiteralPath $tmpLock -Force -ErrorAction SilentlyContinue
    }
    if ($Profile -eq "cuda") {
        Invoke-SidecarPip -VenvPython $py -Arguments @($ortGpu)
    }
    Set-Content -LiteralPath $marker -Encoding ASCII -Value $markerText
    Write-Host "[ok] venv $venvName installed ($Profile)"
}

# --- 4. self-test -----------------------------------------------------------
if ($SelfTest) {
    $code = @"
import importlib.util, json, sys
mods = ["gliner", "torch", "transformers", "natasha", "navec", "pymorphy2",
        "setuptools", "docx", "openpyxl", "lxml", "numpy", "onnxruntime"]
out = {"python": sys.version.split()[0], "profile": "$Profile", "modules": {}}
for name in mods:
    out["modules"][name] = bool(importlib.util.find_spec(name))
try:
    import onnxruntime as rt
    out["onnxruntime"] = rt.__version__
    out["providers"] = list(rt.get_available_providers())
except Exception as e:
    out["onnxruntime_error"] = "%s: %s" % (type(e).__name__, e)
print(json.dumps(out, ensure_ascii=False))
"@
    Test-SidecarImports -VenvPython $py -Code $code
}

Write-Host "[p6] anonymizer sidecar ready: $OutDir ($venvName, $Profile)"
