# export_gigaam_onnx.ps1 - export GigaAM-v3 to ONNX for the DML/CPU profile (P5.2).
#
# Output layout (PLAN §7):
#   <ModelsDir>\gigaam-onnx\<variant>\v3_<variant>_encoder.onnx
#   ...\v3_<variant>_decoder.onnx, ...\v3_<variant>_joint.onnx, ...\v3_<variant>.yaml
#   ...\export.json   (stamp: variant, dtype, versions, files + sha256)
#
# Names come from gigaam.to_onnx (cfg.model_name = v3_<variant>); the worker
# resolves them at run time (hds_transcribe.models.resolve_onnx_variant), so the
# short transcribe.asr.variant=e2e_rnnt keeps working.
#
# Export runs on CPU: to_onnx traces the module, and a CUDA fp16 model would bake
# a GPU-only graph (P0 report).
#
# Idempotent: a stamp with the same variant+dtype and the expected files is not
# re-exported unless -Force. -Verify loads every graph in an ONNX Runtime CPU
# session (no GPU needed).
#
# Only the rnnt variant is staged (decision 23.10.2026): GigaAM exports ctc as a
# SINGLE <model>.onnx (no _encoder/_decoder/_joint), so the file checks below do
# not describe it. A ctc request fails fast with an explanation.
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\export_gigaam_onnx.ps1 -ModelsDir D:\models -Verify
#   ... -Variant e2e_rnnt -Dtype fp32 -Force
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [string]$ModelsDir = "",
    [string]$Variant = "e2e_rnnt",
    [ValidateSet("fp32", "fp16")]
    [string]$Dtype = "fp32",
    [string]$Python = "",
    [string]$GigaamCache = "",
    [switch]$Force,
    [switch]$Verify,
    [switch]$CheckOnly
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "sidecar_common.ps1")

# rnnt only (decision 23.10.2026): `gigaam.to_onnx` writes ctc as one <model>.onnx,
# which this script does not verify/stamp.
if ($Variant -notmatch "rnnt") {
    throw ("export_gigaam_onnx: only the rnnt variant is supported (got '$Variant'). " +
        "GigaAM exports ctc as a single <model>.onnx and is not staged by this script.")
}

$root = Get-SidecarRoot -ScriptRoot $PSScriptRoot
if (-not $ModelsDir) {
    if ($env:HDS_MODELS_DIR) { $ModelsDir = $env:HDS_MODELS_DIR }
    elseif ($env:ProgramData) { $ModelsDir = Join-Path $env:ProgramData "hermes-disk-search\models" }
    else { $ModelsDir = Join-Path $root "models" }
}
if (-not [System.IO.Path]::IsPathRooted($ModelsDir)) { $ModelsDir = Join-Path $root $ModelsDir }
New-Item -ItemType Directory -Force -Path $ModelsDir | Out-Null

if (-not $Python) { $Python = $env:HDS_TRANSCRIBE_PYTHON }
if (-not $Python) {
    foreach ($name in @("venv-transcribe", "venv-transcribe-dml")) {
        $cand = Join-Path $root "sidecar\$name\Scripts\python.exe"
        if (Test-Path $cand) { $Python = $cand; break }
    }
}
if (-not $Python -or -not (Test-Path $Python)) {
    throw "no Python with gigaam/torch found: pass -Python or set HDS_TRANSCRIBE_PYTHON (build the venv first: installers\build_transcribe_sidecar.ps1)"
}
if (-not $GigaamCache) {
    if ($env:HDS_GIGAAM_CACHE) { $GigaamCache = $env:HDS_GIGAAM_CACHE }
    else { $GigaamCache = Join-Path $ModelsDir "gigaam" }
}

$outDir = Join-Path $ModelsDir ("gigaam-onnx\" + $Variant)
$stamp = Join-Path $outDir "export.json"
$modelName = if ($Variant -like "v3_*") { $Variant } else { "v3_$Variant" }
$need = @("$modelName.yaml", "${modelName}_encoder.onnx")
if ($Variant -match "rnnt") {
    $need += "${modelName}_decoder.onnx"
    $need += "${modelName}_joint.onnx"
}

function Test-ExportReady {
    if (-not (Test-Path $stamp)) { return $false }
    try { $j = Get-Content -LiteralPath $stamp -Raw | ConvertFrom-Json } catch { return $false }
    if ($j.variant -ne $Variant -or $j.dtype -ne $Dtype) { return $false }
    foreach ($n in $need) {
        if (-not (Test-Path (Join-Path $outDir $n))) { return $false }
    }
    return $true
}

if (Test-ExportReady -and -not $Force) {
    Write-Host "[ok] ONNX export already present: $outDir ($Variant, $Dtype)"
    if ($CheckOnly -or -not $Verify) { exit 0 }
} elseif ($CheckOnly) {
    Write-Host "[--] ONNX export is missing: $outDir (need: $($need -join ', '))"
    exit 1
} else {
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
    $exporter = Join-Path $env:TEMP ("hds-export-gigaam-" + [Guid]::NewGuid().ToString("N") + ".py")
    $code = @'
import argparse, hashlib, json, os, sys, time

ap = argparse.ArgumentParser()
ap.add_argument("--variant", required=True)
ap.add_argument("--dtype", default="fp32", choices=["fp32", "fp16"])
ap.add_argument("--out-dir", required=True)
ap.add_argument("--cache", required=True)
args = ap.parse_args()

import torch
import gigaam

os.makedirs(args.out_dir, exist_ok=True)
t0 = time.perf_counter()
model = gigaam.load_model(args.variant, fp16_encoder=False, device="cpu",
                          download_root=args.cache)
dtype = torch.float16 if args.dtype == "fp16" else torch.float32
model.to_onnx(dir_path=args.out_dir, dtype=dtype)

def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()

files = []
for name in sorted(os.listdir(args.out_dir)):
    if not name.endswith((".onnx", ".yaml")):
        continue
    path = os.path.join(args.out_dir, name)
    files.append({"name": name, "size": os.path.getsize(path), "sha256": sha256(path)})
variant = args.variant[3:] if args.variant.startswith("v3_") else args.variant
export = {
    "version": 1,
    "variant": variant,
    "model_name": args.variant,
    "dtype": args.dtype,
    "out_dir": os.path.abspath(args.out_dir),
    "cache": os.path.abspath(args.cache),
    "export_sec": round(time.perf_counter() - t0, 2),
    "created": time.strftime("%Y-%m-%dT%H:%M:%S"),
    "torch": torch.__version__,
    "onnx": __import__("onnx").__version__,
    "files": files,
}
with open(os.path.join(args.out_dir, "export.json"), "w", encoding="utf-8") as f:
    json.dump(export, f, ensure_ascii=False, indent=2)
print(json.dumps({"ok": True, "out_dir": export["out_dir"], "files": len(files)}, ensure_ascii=False))
'@
    try {
        Set-Content -LiteralPath $exporter -Encoding ASCII -Value $code
        Write-Host "[..] exporting GigaAM $Variant ($Dtype) -> $outDir"
        & $Python $exporter --variant $modelName --dtype $Dtype --out-dir $outDir --cache $GigaamCache
        if ($LASTEXITCODE -ne 0) { throw "ONNX export failed (exit $LASTEXITCODE)" }
    } finally {
        Remove-Item -LiteralPath $exporter -Force -ErrorAction SilentlyContinue
    }
    if (-not (Test-ExportReady)) {
        throw "export finished but the expected files are missing in $outDir (need: $($need -join ', '))"
    }
    Write-Host "[ok] exported: $outDir"
}

if ($Verify) {
    $verifier = Join-Path $env:TEMP ("hds-verify-gigaam-" + [Guid]::NewGuid().ToString("N") + ".py")
    $vcode = @'
import json, os, sys
import onnxruntime as rt
directory = sys.argv[1]
out = {}
for name in sorted(os.listdir(directory)):
    if not name.endswith(".onnx"):
        continue
    sess = rt.InferenceSession(os.path.join(directory, name), providers=["CPUExecutionProvider"])
    out[name] = [i.name for i in sess.get_inputs()]
print(json.dumps(out, ensure_ascii=False))
'@
    try {
        Set-Content -LiteralPath $verifier -Encoding ASCII -Value $vcode
        Write-Host "[..] verifying ONNX graphs with an ORT CPU session"
        & $Python $verifier $outDir
        if ($LASTEXITCODE -ne 0) { throw "ONNX verification failed (exit $LASTEXITCODE)" }
    } finally {
        Remove-Item -LiteralPath $verifier -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "[p5] ONNX ready: $outDir"
