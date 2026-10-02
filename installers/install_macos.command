#!/bin/bash
# hermes-disk-search: macOS installer (Rust-first).
#
# Installs the Rust binaries (bin/hds, bin/hds_mcp, bin/llm_host), the engine
# runtime (macos-arm64-metal, by runtime-manifests/engine-manifest.json) and a
# config.yaml. Run it from a Terminal (right-click -> Open, or `bash
# install_macos.command`) - a Terminal bypasses Gatekeeper quarantine.
#
# ASCII-only on purpose.
set -u

ROOT="$(cd "$(dirname "$0")" && pwd)"
echo "== hermes-disk-search: install (macOS, Rust-first) =="
echo "   root: $ROOT"

# --- 0. Gatekeeper quarantine (unsigned binaries/.command from a download) ----
if command -v xattr >/dev/null 2>&1; then
    xattr -dr com.apple.quarantine "$ROOT" 2>/dev/null || true
fi

# --- 1. Rust binaries -> ~/.local/bin ----------------------------------------
BIN_DIR="$HOME/.local/bin"
mkdir -p "$BIN_DIR"
for b in hds hds_mcp llm_host; do
    if [ -f "$ROOT/bin/$b" ]; then
        cp "$ROOT/bin/$b" "$BIN_DIR/$b"
        chmod +x "$BIN_DIR/$b"
        echo "[ok] $b -> $BIN_DIR/$b"
    fi
done
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "[i] Add to PATH:  export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

# --- 2. config.yaml -----------------------------------------------------------
if [ ! -f "$ROOT/config.yaml" ] && [ -f "$ROOT/config.example.yaml" ]; then
    cp "$ROOT/config.example.yaml" "$ROOT/config.yaml"
    echo "[ok] config.yaml created - edit roots/db_path to taste"
fi

# --- 3. engine runtime (metal) by manifest ------------------------------------
MANIFEST="$ROOT/runtime-manifests/engine-manifest.json"
ENGINE_DIR="$HOME/Library/Application Support/OpenResearchTools/TranscribeOffline/Engine"
if [ -f "$MANIFEST" ]; then
    if [ -f "$ENGINE_DIR/.hds-engine.json" ]; then
        echo "[ok] engine runtime already present: $ENGINE_DIR"
    else
        vals="$(python3 - "$MANIFEST" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
for a in m.get("assets", []):
    if a.get("platform") == "macos-arm64" and a.get("backend") == "metal":
        print(a["url"], a["sha256"]); break
PY
)"
        url="$(echo "$vals" | awk '{print $1}')"
        sha="$(echo "$vals" | awk '{print $2}')"
        if [ -n "${url:-}" ]; then
            echo "[..] downloading engine runtime (macos-arm64-metal)..."
            tmp="$(mktemp -d)"
            if curl -L --fail -o "$tmp/engine.zip" "$url"; then
                got="$(shasum -a 256 "$tmp/engine.zip" | awk '{print $1}')"
                if [ "$got" != "$sha" ]; then
                    echo "[!!] sha256 mismatch: expected $sha, got $got"
                else
                    mkdir -p "$ENGINE_DIR"
                    unzip -q -o "$tmp/engine.zip" -d "$tmp/x"
                    n="$(find "$tmp/x" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')"
                    top="$(find "$tmp/x" -mindepth 1 -maxdepth 1 | head -1)"
                    if [ "$n" = "1" ] && [ -d "$top" ]; then
                        cp -R "$top"/. "$ENGINE_DIR"/
                    else
                        cp -R "$tmp/x"/. "$ENGINE_DIR"/
                    fi
                    printf '{"tag":"v1.15","backend":"metal","sha256":"%s"}\n' "$sha" > "$ENGINE_DIR/.hds-engine.json"
                    xattr -dr com.apple.quarantine "$ENGINE_DIR" 2>/dev/null || true
                    echo "[ok] engine runtime installed: $ENGINE_DIR"
                fi
            else
                echo "[--] download failed - install the engine runtime later with installers/fetch_engine_runtime.ps1 (pwsh)"
            fi
            rm -rf "$tmp"
        else
            echo "[--] no macos-arm64-metal asset in the manifest"
        fi
    fi
else
    echo "[--] manifest not found: $MANIFEST"
fi

# --- 4. diagnostics -----------------------------------------------------------
if [ -x "$BIN_DIR/hds" ]; then
    echo "[..] hds check:"
    HDS_CONFIG="$ROOT/config.yaml" "$BIN_DIR/hds" check || true
else
    echo "[--] bin/hds missing in the archive"
fi

echo ""
echo "== Done. Index a disk:  HDS_CONFIG=\"$ROOT/config.yaml\" hds index  =="
