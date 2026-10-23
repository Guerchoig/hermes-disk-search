# build_transcribe_sidecar.ps1 - assemble the transcription sidecar tree (P5.1).
#
# Creates the profile venv from the bundled CPython and installs the pinned
# requirements:
#   -Profile cuda -> sidecar\venv-transcribe      (torch cu128)
#   -Profile cpu  -> sidecar\venv-transcribe      (torch cpu)
#   -Profile dml  -> sidecar\venv-transcribe-dml  (torch cpu + onnxruntime-directml)
#
# Only ONE profile is installed per tree: onnxruntime and onnxruntime-directml
# cannot live in the same venv (PLAN §4.7/§7).
#
# The script is idempotent: a matching marker (.hds-transcribe-profile with the
# profile and the lock hash) means "nothing to do". -Force rebuilds the venv.
#
# Order (from the P0 recipe, tools/parity/out/py_transcribe/setup_envs.ps1):
#   1) torch/torchaudio from the pytorch index FIRST - a bare torch==2.8.0 from
#      PyPI is the CPU build even for the cuda profile;
#   2) the rest of the lock file;
#   3) dml only: gigaam with --no-deps (its onnxruntime==1.23.* would replace
#      onnxruntime-directml).
#
# Requires uv (preferred) or a portable CPython venv with pip. Heavy: 1.5 GB
# (dml) - 6 GB (cuda).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_transcribe_sidecar.ps1 -Profile cuda -SelfTest
#   ... -Profile dml -OutDir dist\hds-1.0-windows-x64\sidecar
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("cuda", "cpu", "dml")]
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

$venvName = if ($Profile -eq "dml") { "venv-transcribe-dml" } else { "venv-transcribe" }
$lockName = if ($Profile -eq "dml") { "requirements-dml.lock" } else { "requirements.lock" }
$torchIndex = if ($Profile -eq "cuda") { "https://download.pytorch.org/whl/cu128" } else { "https://download.pytorch.org/whl/cpu" }
$srcSidecar = Join-Path $root "sidecar"
$lockPath = Join-Path $srcSidecar "hds_transcribe\$lockName"

Write-Host "[p5] transcriber profile: $Profile (venv $venvName)"
$portable = Ensure-PortablePython -SidecarDir $OutDir -PythonVersion $PythonVersion -Force:$Force
Write-Host "[p5] CPython: $portable"

# --- 1. package files in the target tree ------------------------------------
if (-not $NoCopy -and ((Resolve-Path $OutDir).Path -ne (Resolve-Path $srcSidecar).Path)) {
    Copy-SidecarTree -Source $srcSidecar -Destination $OutDir
    Copy-Item (Join-Path $srcSidecar "hds_transcribe\*.lock") (Join-Path $OutDir "hds_transcribe") -Force
}
$venv = Join-Path $OutDir $venvName
$py = Join-Path $venv "Scripts\python.exe"

# --- 2. marker: profile + lock hash -----------------------------------------
$lockHash = (Get-FileHash -LiteralPath $lockPath -Algorithm SHA256).Hash.ToLower()
$marker = Join-Path $venv ".hds-transcribe-profile"
$markerText = "profile=$Profile`npython=$PythonVersion`nlock=$lockName`nsha256=$lockHash`ntorch_index=$torchIndex"
$upToDate = $false
if (-not $Force -and (Test-Path $py) -and (Test-Path $marker)) {
    $old = (Get-Content -LiteralPath $marker -Raw).Trim()
    $upToDate = ($old -eq $markerText.Trim())
}
if ($upToDate) {
    Write-Host "[ok] venv $venvName is up to date (marker matches)"
} else {
    $py = New-SidecarVenv -SidecarDir $OutDir -Name $venvName -PortablePython $portable -Force:$Force

    # --- 3. torch first, from the profile index -----------------------------
    Invoke-SidecarPip -VenvPython $py -IndexArguments @("--index-url", $torchIndex) `
        -Arguments @("torch==2.8.0", "torchaudio==2.8.0")

    # --- 4. the rest of the lock --------------------------------------------
    if ($Profile -eq "dml") {
        # gigaam is installed separately (--no-deps): the lock has no gigaam line.
        Install-SidecarRequirements -VenvPython $py -RequirementsPath $lockPath
        Invoke-SidecarPip -VenvPython $py -Arguments @("--no-deps",
            "gigaam @ git+https://github.com/salute-developers/GigaAM.git@7447938d791c4f3e643386ee22c33777004293a5")
    } else {
        Install-SidecarRequirements -VenvPython $py -RequirementsPath $lockPath
    }
    Set-Content -LiteralPath $marker -Encoding ASCII -Value $markerText
    Write-Host "[ok] venv $venvName installed"
}

# --- 5. self-test ------------------------------------------------------------
if ($SelfTest) {
    $code = @"
import importlib.util, json, sys
mods = ["torch", "onnxruntime", "gigaam", "pyannote.audio", "transformers", "silero_vad"]
out = {"python": sys.version.split()[0], "profile": "$Profile", "modules": {}}
for name in mods:
    out["modules"][name] = bool(importlib.util.find_spec(name))
import torch
out["torch"] = torch.__version__
out["cuda"] = bool(torch.cuda.is_available())
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

Write-Host "[p5] transcriber sidecar ready: $OutDir ($venvName)"
