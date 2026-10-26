# fetch_anonymize_models.ps1 - pre-fetch the anonymization models (P6).
#
# Downloads into a MACHINE directory so that the sidecar can work offline
# (HF_HUB_OFFLINE=1) without network access at runtime:
#   <ModelsDir>\anonymize\hf-home\hub\models--knowledgator--gliner-pii-large-v1.0\
#                                            GLiNER PII (HF cache layout)
#   <ModelsDir>\anonymize\navec_news_v1_1B_250K_300d_100q.tar
#   <ModelsDir>\anonymize\slovnet_ner_news_v1.tar
#   <ModelsDir>\anonymize-models.json       manifest: revisions + sha256
#
# The GLiNER PII model is PUBLIC: no HF token is required. -HfToken / HF_TOKEN
# is optional (rate limits only) and is never stored.
#
# Idempotent: an existing cache snapshot / tarball is not downloaded again;
# -Force re-downloads. -Offline uses only the local cache and fails with a
# clear message when a model is missing.
#
# Examples:
#   powershell -NoProfile -ExecutionPolicy Bypass -File installers\fetch_anonymize_models.ps1
#   ... -ModelsDir D:\models -Offline
#
# ASCII-only on purpose (PowerShell 5.1 reads a BOM-less .ps1 as ANSI).
param(
    [string]$ModelsDir = "",
    [string]$Python = "",
    [string]$Revision = "main",
    [string]$HfToken = "",
    [switch]$SkipGliner,
    [switch]$SkipNatasha,
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

if (-not $Python) { $Python = $env:HDS_ANONYMIZE_PYTHON }
if (-not $Python) {
    $cand = Join-Path $root "sidecar\venv-anonymize\Scripts\python.exe"
    if (Test-Path $cand) { $Python = $cand }
}
if (-not $Python -or -not (Test-Path $Python)) {
    throw "no anonymize venv found: build it first (installers\build_anonymize_sidecar.ps1) or pass -Python"
}
if (-not $HfToken) { $HfToken = $env:HF_TOKEN }
if (-not $HfToken) { $HfToken = $env:HUGGING_FACE_HUB_TOKEN }

$anonDir = Join-Path $ModelsDir "anonymize"
$hfHome = Join-Path $anonDir "hf-home"
New-Item -ItemType Directory -Force -Path $anonDir, $hfHome | Out-Null

# Natasha models: fixed release assets (not on HF).
$tarballs = @()
if (-not $SkipNatasha) {
    $tarballs = @(
        @{ name = "navec_news_v1_1B_250K_300d_100q.tar"
           url = "https://github.com/natasha/navec/releases/download/v1.0.0/navec_news_v1_1B_250K_300d_100q.tar" },
        @{ name = "slovnet_ner_news_v1.tar"
           url = "https://github.com/natasha/slovnet/releases/download/v0.5.0/slovnet_ner_news_v1.tar" }
    )
}
foreach ($t in $tarballs) {
    $dst = Join-Path $anonDir $t.name
    $need = $Force -or -not (Test-Path $dst) -or ((Get-Item -LiteralPath $dst).Length -eq 0)
    if ($need) {
        if ($Offline) { throw ("{0} is not cached in {1} (-Offline)" -f $t.name, $anonDir) }
        Write-Host ("[fetch] {0}" -f $t.name)
        Invoke-WebRequest -UseBasicParsing -Uri $t.url -OutFile $dst
    }
    $t.path = $dst
    $t.size = (Get-Item -LiteralPath $dst).Length
    $t.sha256 = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash.ToLower()
}

$job = @{
    models_dir = (Resolve-Path $ModelsDir).Path
    anonymize_dir = $anonDir
    hf_home = $hfHome
    repo = "knowledgator/gliner-pii-large-v1.0"
    revision = $Revision
    skip_gliner = [bool]$SkipGliner
    offline = [bool]$Offline
    force = [bool]$Force
    tarballs = @($tarballs)
}
$jobPath = Join-Path $env:TEMP ("hds-fetch-anon-" + [Guid]::NewGuid().ToString("N") + ".json")
# PowerShell 5.1 `Set-Content -Encoding UTF8` writes a BOM; json.load rejects it.
[System.IO.File]::WriteAllText($jobPath, ($job | ConvertTo-Json -Depth 5),
    (New-Object System.Text.UTF8Encoding($false)))

$fetcher = Join-Path $env:TEMP ("hds-fetch-anon-" + [Guid]::NewGuid().ToString("N") + ".py")
$code = @'
import hashlib
import json
import os
import sys
import time

job = json.load(open(sys.argv[1], encoding="utf-8"))
models_dir = job["models_dir"]
anon_dir = job["anonymize_dir"]
hf_home = job["hf_home"]
result = {"ok": True, "gliner": None, "natasha": job.get("tarballs") or [], "warnings": []}


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


HASH_EXT = (".bin", ".safetensors", ".onnx", ".pt", ".pth", ".model", ".yaml", ".json")
SKIP_DIRS = ("blobs", ".locks")


def file_manifest(directory):
    # The HF cache keeps the same bytes twice (blobs + snapshot links) and lock
    # files that change on every run - the manifest lists the snapshot only.
    rows = []
    for root, dirs, files in os.walk(directory, followlinks=True):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for name in sorted(files):
            path = os.path.join(root, name)
            size = os.path.getsize(path)
            rel = os.path.relpath(path, directory)
            row = {"name": rel, "size": size}
            if name.endswith(HASH_EXT):
                row["sha256"] = sha256(path)
            rows.append(row)
    return rows


def find_onnx(directory):
    for root, dirs, files in os.walk(directory, followlinks=True):
        for name in files:
            if name.endswith(".onnx"):
                return os.path.join(root, name)
    return None


if not job.get("skip_gliner"):
    repo = job["repo"]
    revision = job["revision"]
    os.environ.setdefault("HF_HOME", hf_home)
    cache = os.path.join(hf_home, "hub", "models--" + repo.replace("/", "--"))
    snapshots = os.path.join(cache, "snapshots")
    snap = None
    if os.path.isdir(snapshots):
        found = sorted(d for d in os.listdir(snapshots)
                       if os.path.isdir(os.path.join(snapshots, d)))
        if found:
            snap = os.path.join(snapshots, found[-1])
    if snap and not job.get("force"):
        result["gliner"] = {"status": "cached", "snapshot": snap,
                            "resolved": os.path.basename(snap)}
    elif job.get("offline"):
        result["ok"] = False
        result["warnings"].append("%s: not cached (-Offline)" % repo)
        result["gliner"] = {"status": "missing", "snapshot": None}
    else:
        try:
            from huggingface_hub import snapshot_download
        except Exception as e:  # noqa: BLE001
            fail("hf_error", "huggingface_hub is not installed: %s: %s" % (type(e).__name__, e))
        print("[fetch] %s @ %s" % (repo, revision), flush=True)
        token = os.environ.get("HF_TOKEN") or None
        try:
            snap = snapshot_download(repo_id=repo, revision=revision,
                                     token=token, force_download=job.get("force", False))
        except Exception as e:  # noqa: BLE001
            fail("hf_download_failed", "%s: %s: %s" % (repo, type(e).__name__, e))
        result["gliner"] = {"status": "downloaded", "snapshot": snap,
                            "resolved": os.path.basename(snap)}
    g = result["gliner"] or {}
    g["repo"] = repo
    g["revision_requested"] = revision
    if g.get("snapshot") and os.path.isdir(g["snapshot"]):
        g["files"] = file_manifest(g["snapshot"])
        g["onnx"] = find_onnx(g["snapshot"])
        if not g["onnx"]:
            result["warnings"].append(
                "no .onnx weights in the snapshot - export GLiNER to ONNX "
                "(the worker needs model.onnx)")
    result["gliner"] = g
else:
    result["gliner"] = {"status": "skipped"}

manifest = {
    "version": 1,
    "generated": time.strftime("%Y-%m-%dT%H:%M:%S"),
    "models_dir": models_dir,
    "anonymize_dir": anon_dir,
    "hf_home": hf_home,
    "gliner": result["gliner"],
    "natasha": result["natasha"],
    "warnings": result["warnings"],
}
with open(os.path.join(models_dir, "anonymize-models.json"), "w", encoding="utf-8") as f:
    json.dump(manifest, f, ensure_ascii=False, indent=2)
print(json.dumps(result, ensure_ascii=False, indent=2))
'@

$env:HF_HOME = $hfHome
$env:HF_HUB_DISABLE_TELEMETRY = "1"
if ($HfToken) { $env:HF_TOKEN = $HfToken }
if ($Offline) { $env:HF_HUB_OFFLINE = "1" } else { Remove-Item Env:HF_HUB_OFFLINE -ErrorAction SilentlyContinue }
try {
    [System.IO.File]::WriteAllText($fetcher, $code, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "[..] fetching anonymization models into $ModelsDir"
    & $Python $fetcher $jobPath
    $rc = $LASTEXITCODE
} finally {
    Remove-Item -LiteralPath $fetcher -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $jobPath -Force -ErrorAction SilentlyContinue
    if ($HfToken) { Remove-Item Env:HF_TOKEN -ErrorAction SilentlyContinue }
}
if ($rc -ne 0) { throw "fetch_anonymize_models: python exited with code $rc" }
Write-Host "[p6] models ready (manifest: $(Join-Path $ModelsDir 'anonymize-models.json'))"

