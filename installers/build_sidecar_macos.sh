#!/bin/bash
# build_sidecar_macos.sh - macOS variant of installers/build_sidecar.ps1 (W4).
#
# Assembles a self-contained Python sidecar tree for macOS using `uv` +
# python-build-standalone (same layout as the Windows sidecar: worker.py +
# requirements.lock + a portable interpreter + a build-time copy of the `hds\`
# extractor/lemmatizer modules). ASCII-only on purpose.
#
# Usage: bash installers/build_sidecar_macos.sh [OutDir]
set -euo pipefail

OUT="${1:-sidecar}"
PY_VER="${HDS_SIDECAR_PYTHON:-3.12}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "$OUT" in /*) ;; *) OUT="$ROOT/$OUT" ;; esac
mkdir -p "$OUT"

if ! command -v uv >/dev/null 2>&1; then
    echo "uv not found in PATH (https://docs.astral.sh/uv/)" >&2
    exit 1
fi

PYDIR="$OUT/python"
mkdir -p "$OUT/hds_extract" "$OUT/hds"

# --- 1. portable CPython ------------------------------------------------------
if [ ! -f "$PYDIR/.hds-python" ]; then
    echo "[..] uv python install $PY_VER --install-dir $PYDIR"
    uv python install "$PY_VER" --install-dir "$PYDIR"
    touch "$PYDIR/.hds-python"
fi
PY="$(find "$PYDIR" -type f \( -name 'python3*' -o -name 'python' \) 2>/dev/null | sort | head -1)"
if [ -z "$PY" ]; then
    echo "python interpreter not found under $PYDIR" >&2
    exit 1
fi

# --- 2. worker + lock + readme ------------------------------------------------
cp "$ROOT/sidecar/hds_extract/worker.py" "$OUT/hds_extract/"
cp "$ROOT/sidecar/hds_extract/requirements.lock" "$OUT/hds_extract/"
cp "$ROOT/sidecar/README.md" "$OUT/"

# --- 3. shared packages: hds_sidecar (both workers) + hds_transcribe ----------
# `hds_extract.worker` imports `hds_sidecar`; the transcription worker runs from
# the profile venv, and installers/build_transcribe_sidecar.ps1 takes the package
# AND the lock files from THIS tree (an installed archive has no repo sources).
for pkg in hds_sidecar hds_transcribe; do
    if [ ! -d "$ROOT/sidecar/$pkg" ]; then
        echo "missing package: sidecar/$pkg" >&2
        exit 1
    fi
    mkdir -p "$OUT/$pkg"
    cp -R "$ROOT/sidecar/$pkg/." "$OUT/$pkg/"
    find "$OUT/$pkg" -type d -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true
done

# --- 4. self-contained copy of the extractor/lemmatizer modules ---------------
for m in __init__.py config.py extractors.py extract_av.py extract_static.py \
         lemmatizer.py whisper_cpp.py; do
    if [ ! -f "$ROOT/hds/$m" ]; then
        echo "missing module: hds/$m" >&2
        exit 1
    fi
    cp "$ROOT/hds/$m" "$OUT/hds/"
done

# --- 5. worker dependencies into the portable interpreter ---------------------
if [ ! -f "$PYDIR/.hds-deps" ]; then
    echo "[..] uv pip install worker dependencies..."
    uv pip install --python "$PY" --break-system-packages \
        -r "$OUT/hds_extract/requirements.lock"
    touch "$PYDIR/.hds-deps"
fi

# --- 6. self-test: the copied package must import without the project core ----
echo "[..] self-test: importing the copied hds package from $OUT"
"$PY" -c "import os, sys; sys.path.insert(0, r'$OUT'); import hds.config, hds.extractors, hds.extract_av, hds.extract_static, hds.lemmatizer, hds.whisper_cpp, jpype, mpxj; import hds_sidecar, hds_extract.worker; assert os.path.isfile(os.path.join(r'$OUT', 'hds_transcribe', 'worker.py')); assert os.path.isfile(os.path.join(r'$OUT', 'hds_transcribe', 'requirements.lock')); print('sidecar-ok')"
echo "[ok] sidecar assembled: $OUT"
