#!/bin/bash
# build_rust_release_macos.sh - macOS variant of installers/build_rust_release.ps1 (W4).
#
# Builds the Rust release binaries and stages a macOS arm64 distribution folder:
#   dist/hds-<Version>-macos-arm64/            full install layout
#   dist/hds-<Version>-macos-arm64.zip         the same tree as a zip
#   dist/hds-<Version>-macos-arm64.zip.sha256.txt
#
# Usage: bash installers/build_rust_release_macos.sh <version> [SidecarDir]
# ASCII-only on purpose.
set -euo pipefail

VERSION="${1:-0.1.0}"
SIDECAR_DIR="${2:-}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "[w4] building release binaries..."
cargo build --release -p hds-cli -p hds-mcp -p hds-llama

STAGE="$ROOT/dist/hds-$VERSION-macos-arm64"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin"

# --- binaries -----------------------------------------------------------------
for b in hds hds_mcp llm_host; do
    if [ ! -f "$ROOT/target/release/$b" ]; then
        echo "missing target/release/$b" >&2
        exit 1
    fi
    cp "$ROOT/target/release/$b" "$STAGE/bin/$b"
    chmod +x "$STAGE/bin/$b"
done

# --- mac installer at the archive root ---------------------------------------
if [ -f "$ROOT/installers/install_macos.command" ]; then
    cp "$ROOT/installers/install_macos.command" "$STAGE/install_macos.command"
    chmod +x "$STAGE/install_macos.command"
fi

# --- directories copied as a whole -------------------------------------------
for d in installers runtime-manifests assets hermes-skill cline-rules shortcuts; do
    if [ ! -d "$ROOT/$d" ]; then
        continue
    fi
    cp -R "$ROOT/$d" "$STAGE/$d"
done
# macOS launchers must be executable (the git checkout may not carry the +x bit on
# every platform): force it in the staged tree.
for f in "shortcuts/macos/Hermes Disk Search.command" \
         "shortcuts/macos/Индексация дисков.command" \
         "shortcuts/macos/HermesDiskSearchIndex.app/Contents/MacOS/run_index"; do
    [ -f "$STAGE/$f" ] && chmod +x "$STAGE/$f"
done
find "$STAGE" -type d -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true

# --- sidecar: prebuilt tree (build-sidecar job) or the repo copy --------------
if [ -n "$SIDECAR_DIR" ]; then
    case "$SIDECAR_DIR" in
        /*) SRC_SIDECAR="$SIDECAR_DIR" ;;
        *)  SRC_SIDECAR="$ROOT/$SIDECAR_DIR" ;;
    esac
    if [ -d "$SRC_SIDECAR" ]; then
        cp -R "$SRC_SIDECAR" "$STAGE/sidecar"
        find "$STAGE/sidecar" -type d -name __pycache__ -prune -exec rm -rf {} + 2>/dev/null || true
    else
        echo "[--] sidecar dir not found: $SRC_SIDECAR (packaging without sidecar)"
    fi
fi

# --- docs ---------------------------------------------------------------------
for f in config.example.yaml README.md NOTICE.md; do
    if [ -f "$ROOT/$f" ]; then
        cp "$ROOT/$f" "$STAGE/$f"
    fi
done

# --- sha256 manifest for bin/* -------------------------------------------------
( cd "$STAGE" && shasum -a 256 bin/* > sha256.txt )

echo "[w4] staged: $STAGE"

# --- zip + checksum ------------------------------------------------------------
ZIP="$ROOT/dist/hds-$VERSION-macos-arm64.zip"
rm -f "$ZIP"
( cd "$STAGE" && zip -r -X "$ZIP" . >/dev/null )
zh="$(shasum -a 256 "$ZIP" | awk '{print $1}')"
echo "$zh  hds-$VERSION-macos-arm64.zip" > "$ZIP.sha256.txt"
echo "[w4] package: $ZIP"
echo "[w4] sha256:  $zh"
