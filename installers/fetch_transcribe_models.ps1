# fetch_transcribe_models.ps1 - pre-fetch the transcription models (P5.3).
#
# Downloads into a MACHINE directory so that the service (LocalSystem) can work
# with HF_HUB_OFFLINE=1 and no HF token (PLAN §7):
#   <ModelsDir>\gigaam\v3_<variant>.ckpt          GigaAM-v3 e2e_rnnt (Sber CDN)
#   <ModelsDir>\hf-home\hub\models--...           wav2vec2-ru, community-1,
#                                                 segmentation-3.0 (HF cache)
#   <ModelsDir>\gigaam-onnx\<variant>\            exported separately (P5.2)
#   <ModelsDir>\transcribe-models.json            manifest: revisions + sha256
#
# Gated models (community-1, segmentation-3.0) need an accepted licence and a
# token for the FIRST download; -HfToken / HF_TOKEN is only used here, never
# stored. Once cached, the runtime sets HF_HUB_OFFLINE=1 and needs nothing.
#
# Idempotent: an existing cache snapshot / .ckpt is not downloaded again;
# -Force forces a re-download. -Offline uses only the local cache (fails with a
# clear message when a model is missing).
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_transcribe_models.ps1 -ModelsDir C:\ProgramData\hermes-disk-search\models -HfToken hf_xxx
#   ... -ModelsDir D:\models -Offline
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [string]$ModelsDir = "",
    [string]$Variant = "e2e_rnnt",
    [string]$HfToken = "",
    [string]$Python = "",
    [switch]$SkipGigaam,
    [switch]$SkipAlignment,
    [switch]$SkipDiarization,
    [switch]$Offline,
    [switch]$Force
)
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "sidecar_common.ps1")

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
    throw "no transcription venv found: build it first (installers\build_transcribe_sidecar.ps1) or pass -Python"
}
if (-not $HfToken) { $HfToken = $env:HF_TOKEN }
if (-not $HfToken) { $HfToken = $env:HUGGING_FACE_HUB_TOKEN }

$hfHome = Join-Path $ModelsDir "hf-home"
$gigaamDir = Join-Path $ModelsDir "gigaam"
New-Item -ItemType Directory -Force -Path $hfHome, $gigaamDir | Out-Null

# Pins: the revisions the P0-P4 runs and the acceptance were measured on.
# They are commit hashes, not mutable branch refs (reproducible installs).
$pins = [ordered]@{
    "jonatasgrosman/wav2vec2-large-xlsr-53-russian" = "2329100508896c6d9b157019803ab5601e6f3406"
    "pyannote/speaker-diarization-community-1"      = "3533c8cf8e369892e6b79ff1bf80f7b0286a54ee"
    "pyannote/segmentation-3.0"                     = "e66f3d3b9eb0873085418a7b813d3b369bf160bb"
}
$repos = @()
if (-not $SkipAlignment) { $repos += "jonatasgrosman/wav2vec2-large-xlsr-53-russian" }
if (-not $SkipDiarization) {
    $repos += "pyannote/speaker-diarization-community-1"
    $repos += "pyannote/segmentation-3.0"
}

$job = @{
    models_dir = (Resolve-Path $ModelsDir).Path
    hf_home = $hfHome
    gigaam_dir = $gigaamDir
    variant = $Variant
    repos = @($repos | ForEach-Object { @{ id = $_; revision = $pins[$_] } })
    skip_gigaam = [bool]$SkipGigaam
    offline = [bool]$Offline
    force = [bool]$Force
    has_token = [bool]$HfToken
}
$jobPath = Join-Path $env:TEMP ("hds-fetch-models-" + [Guid]::NewGuid().ToString("N") + ".json")
# PowerShell 5.1 `Set-Content -Encoding UTF8` writes a BOM; json.load rejects it.
[System.IO.File]::WriteAllText($jobPath, ($job | ConvertTo-Json -Depth 5),
    (New-Object System.Text.UTF8Encoding($false)))

$fetcher = Join-Path $env:TEMP ("hds-fetch-models-" + [Guid]::NewGuid().ToString("N") + ".py")
$code = @'
import hashlib
import json
import os
import sys
import time

job = json.load(open(sys.argv[1], encoding="utf-8"))
models_dir = job["models_dir"]
hf_home = job["hf_home"]
gigaam_dir = job["gigaam_dir"]
variant = job["variant"]
result = {"ok": True, "gigaam": None, "repos": {}, "warnings": []}


def fail(kind, message):
    result["ok"] = False
    result[kind] = message
    print(json.dumps(result, ensure_ascii=False, indent=2))
    sys.exit(2)


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(4 * 1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


HASH_EXT = (".bin", ".safetensors", ".onnx", ".pt", ".pth", ".ckpt", ".model", ".yaml")


def file_manifest(directory):
    rows = []
    for root, dirs, files in os.walk(directory, followlinks=True):
        # HF cache keeps the same bytes twice (blobs + snapshot links) and lock
        # files that change on every run - the manifest lists the snapshot only.
        dirs[:] = [d for d in dirs if d not in ("blobs", ".locks")]
        for name in sorted(files):
            path = os.path.join(root, name)
            size = os.path.getsize(path)
            rel = os.path.relpath(path, directory)
            row = {"name": rel, "size": size}
            if name.endswith(HASH_EXT) or name == "config.json":
                row["sha256"] = sha256(path)
            rows.append(row)
    return rows


# --- 1. GigaAM-v3 checkpoint -------------------------------------------------
if not job["skip_gigaam"]:
    model_name = variant if variant.startswith("v3_") else "v3_" + variant
    ckpt = os.path.join(gigaam_dir, model_name + ".ckpt")
    tokenizer = os.path.join(gigaam_dir, model_name + "_tokenizer.model")
    if os.path.isfile(ckpt) and os.path.isfile(tokenizer) and not job["force"]:
        result["gigaam"] = {"variant": variant, "path": ckpt, "status": "cached"}
    else:
        try:
            import gigaam
        except Exception as e:  # noqa: BLE001
            fail("gigaam_error", "gigaam is not installed in this venv: %s: %s" % (type(e).__name__, e))
        if job["offline"]:
            fail("gigaam_error", "GigaAM %s is not cached in %s (-Offline)" % (model_name, gigaam_dir))
        print("[fetch] GigaAM %s -> %s" % (model_name, gigaam_dir), flush=True)
        model = gigaam.load_model(model_name, fp16_encoder=False, device="cpu",
                                  download_root=gigaam_dir)
        del model
        result["gigaam"] = {"variant": variant, "path": ckpt, "status": "downloaded"}
    result["gigaam"]["files"] = file_manifest(gigaam_dir)
else:
    result["gigaam"] = {"status": "skipped"}

# --- 2. Hugging Face repos ---------------------------------------------------
token = os.environ.get("HF_TOKEN") or None
from huggingface_hub import snapshot_download
from huggingface_hub.utils import GatedRepoError, HfHubHTTPError

for repo in job["repos"]:
    repo_id = repo["id"]
    revision = repo["revision"]
    cache_dir = os.path.join(hf_home, "hub", "models--" + repo_id.replace("/", "--"))
    snapshot = os.path.join(cache_dir, "snapshots", revision)
    cached = os.path.isdir(snapshot) and any(os.scandir(snapshot))
    if cached and not job["force"]:
        result["repos"][repo_id] = {"revision": revision, "status": "cached"}
    else:
        if job["offline"]:
            result["warnings"].append("%s: not cached (-Offline)" % repo_id)
            result["ok"] = False
            continue
        print("[fetch] %s @ %s" % (repo_id, revision[:12]), flush=True)
        try:
            snapshot_download(repo_id=repo_id, revision=revision, cache_dir=None,
                              token=token, force_download=job["force"])
        except GatedRepoError as e:
            fail("hf_token_missing", "%s: licence not accepted / token missing (%s)" % (repo_id, e))
        except HfHubHTTPError as e:
            fail("hf_download_failed", "%s: %s" % (repo_id, e))
        result["repos"][repo_id] = {"revision": revision, "status": "downloaded"}
    result["repos"][repo_id]["snapshot"] = snapshot
    result["repos"][repo_id]["files"] = file_manifest(snapshot) if os.path.isdir(snapshot) else []

# --- 3. manifest -------------------------------------------------------------
manifest = {
    "version": 1,
    "generated": time.strftime("%Y-%m-%dT%H:%M:%S"),
    "models_dir": models_dir,
    "hf_home": hf_home,
    "variant": variant,
    "gigaam": result["gigaam"],
    "repos": result["repos"],
    "warnings": result["warnings"],
}
with open(os.path.join(models_dir, "transcribe-models.json"), "w", encoding="utf-8") as f:
    json.dump(manifest, f, ensure_ascii=False, indent=2)
print(json.dumps(result, ensure_ascii=False, indent=2))
'@

$env:HF_HOME = $hfHome
$env:HF_HUB_DISABLE_TELEMETRY = "1"
$env:PYANNOTE_METRICS_ENABLED = "0"
if ($HfToken) { $env:HF_TOKEN = $HfToken }
if ($Offline) { $env:HF_HUB_OFFLINE = "1" } else { Remove-Item Env:HF_HUB_OFFLINE -ErrorAction SilentlyContinue }
try {
    [System.IO.File]::WriteAllText($fetcher, $code, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "[..] fetching transcription models into $ModelsDir"
    & $Python $fetcher $jobPath
    $rc = $LASTEXITCODE
} finally {
    Remove-Item -LiteralPath $fetcher -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $jobPath -Force -ErrorAction SilentlyContinue
    if ($HfToken) { Remove-Item Env:HF_TOKEN -ErrorAction SilentlyContinue }
}
if ($rc -ne 0) { throw "fetch_transcribe_models: python exited with code $rc" }
Write-Host "[p5] models ready (manifest: $(Join-Path $ModelsDir 'transcribe-models.json'))"
