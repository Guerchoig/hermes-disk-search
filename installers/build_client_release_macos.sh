#!/bin/bash
# Build the THIN CLIENT distribution for macOS (M9 / Э9 / W7).
#
# Contains ONLY: bin/hds + Cline rule/skill sources + install_client.command + README.
# NO engine / models / index / sidecar (that is the SERVER package).
#
# Usage: bash installers/build_client_release_macos.sh <version> [--skip-build] [--skip-zip]
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VER="${1:-0.1.0}"
shift || true
SKIP_BUILD="no"; SKIP_ZIP="no"
for a in "$@"; do
    case "$a" in
        --skip-build) SKIP_BUILD="yes" ;;
        --skip-zip) SKIP_ZIP="yes" ;;
        *) echo "[!!] unknown option: $a"; exit 2 ;;
    esac
done
cd "$ROOT"

if [ "$SKIP_BUILD" != "yes" ]; then
    echo "[m9] building release hds (client only)..."
    cargo build --release -p hds-cli
else
    echo "[m9] --skip-build: reusing target/release/hds"
fi

STAGE="$ROOT/dist/hds-client-$VER-macos-arm64"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin"
[ -f target/release/hds ] || { echo "[!!] missing target/release/hds"; exit 1; }
cp -f target/release/hds "$STAGE/bin/hds"

for d in cline-rules hermes-skill; do
    [ -d "$ROOT/$d" ] && cp -R "$ROOT/$d" "$STAGE/$d"
done
[ -f "$ROOT/installers/install_client.command" ] && cp -f "$ROOT/installers/install_client.command" "$STAGE/install_client.command"
[ -f "$ROOT/installers/client_readme.md" ] && cp -f "$ROOT/installers/client_readme.md" "$STAGE/README.md"

# ad-hoc sign (best effort)
if command -v codesign >/dev/null 2>&1; then
    codesign --force -s - "$STAGE/bin/hds" >/dev/null 2>&1 || echo "[..] codesign skipped"
fi

# guard: no heavy/server parts
for bad in hds_mcp llm_host sidecar runtime-manifests; do
    if find "$STAGE" -name "*$bad*" | grep -q .; then
        echo "[!!] thin client package must not contain: $bad"; exit 1
    fi
done

# sha256 manifest
( cd "$STAGE" && shasum -a 256 bin/hds | awk '{print $1"  bin/hds"}' > sha256.txt )

echo "[m9] staged: $STAGE"
find "$STAGE" -type f | sed "s#$ROOT/##"

if [ "$SKIP_ZIP" != "yes" ]; then
    ( cd "$ROOT/dist" && rm -f "hds-client-$VER-macos-arm64.zip" \
        && zip -qr "hds-client-$VER-macos-arm64.zip" "hds-client-$VER-macos-arm64" \
        && shasum -a 256 "hds-client-$VER-macos-arm64.zip" > "hds-client-$VER-macos-arm64.zip.sha256.txt" )
    echo "[m9] package: dist/hds-client-$VER-macos-arm64.zip"
fi
