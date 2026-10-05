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

# --- 2b. OCR (Tesseract): binary path + rus/eng language packs ----------------
# configure_ocr.py: finds tesseract (brew paths + standard dirs), ensures the
# rus/eng language packs in a user-writable tessdata, writes
# index.ocr_tesseract_cmd into config.yaml. Idempotent and non-blocking:
# "not found" prints the brew hint and returns 0 - Tesseract may be installed
# at ANY time and this step re-runs on every install/update.
PY="$(command -v python3 || true)"
if [ -n "$PY" ]; then
    "$PY" "$ROOT/installers/configure_ocr.py" || true
else
    echo "[i] python3 not found - skipping OCR configuration."
    echo "    Re-run later: python3 installers/configure_ocr.py"
fi

# --- 3. engine runtime (metal) by manifest ------------------------------------
MANIFEST="$ROOT/runtime-manifests/engine-manifest.json"
ENGINE_DIR="$HOME/Library/Application Support/OpenResearchTools/TranscribeOffline/Engine"
if [ -f "$MANIFEST" ]; then
    if [ -f "$ENGINE_DIR/.hds-engine.json" ]; then
        echo "[ok] engine runtime already present: $ENGINE_DIR"
    else
        # Разбор engine-manifest.json без Python (аналог ConvertFrom-Json в Windows):
        # берём url+sha256 ассета platform=macos-arm64, backend=metal.
        vals="$(awk '
            /"platform"[[:space:]]*:[[:space:]]*"macos-arm64"/ { mac=1 }
            mac && /"backend"[[:space:]]*:[[:space:]]*"metal"/ { metal=1 }
            mac && /"url"[[:space:]]*:/    { l=$0; sub(/.*"url"[[:space:]]*:[[:space:]]*"/, "", l); sub(/".*/, "", l); url=l }
            mac && /"sha256"[[:space:]]*:/ { l=$0; sub(/.*"sha256"[[:space:]]*:[[:space:]]*"/, "", l); sub(/".*/, "", l); sha=l }
            mac && metal && url != "" && sha != "" { print url " " sha; exit }
            /}/ { mac=0; metal=0; url=""; sha="" }
        ' "$MANIFEST")"
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

# --- 5. Launchers (Desktop + ~/Applications) ----------------------------------
# Аналог Windows: setup.ps1 (шаг 10) ставит ярлык «Hermes Disk Search» (run_ui.ps1).
# На macOS кладём на Desktop .command-обёртки (UI и индексация) с зашитым корнем и
# копию .app-бандла в ~/Applications с ad-hoc подписью (план §10.4).
LAUNCHER_DIR="$ROOT/shortcuts/macos"
DESKTOP="$HOME/Desktop"
mkdir -p "$DESKTOP"
# Исполняемость лаунчеров (git может не сохранить +x на некоторых платформах).
{ [ -f "$LAUNCHER_DIR/Hermes Disk Search.command" ] && chmod +x "$LAUNCHER_DIR/Hermes Disk Search.command"; } 2>/dev/null || true
{ [ -f "$LAUNCHER_DIR/Индексация дисков.command" ] && chmod +x "$LAUNCHER_DIR/Индексация дисков.command"; } 2>/dev/null || true
{ [ -f "$LAUNCHER_DIR/HermesDiskSearchIndex.app/Contents/MacOS/run_index" ] && chmod +x "$LAUNCHER_DIR/HermesDiskSearchIndex.app/Contents/MacOS/run_index"; } 2>/dev/null || true
write_desktop_launcher() {  # dest  archive-launcher  title
    dest="$1"; src="$2"; title="$3"
    cat > "$dest" <<EOS
#!/bin/bash
# Создано hermes-disk-search (install_macos.command): $title
export HDS_ROOT="$ROOT"
exec "$src"
EOS
    chmod +x "$dest"
    echo "[ok] Desktop launcher: $dest ($title)"
}
if [ -f "$LAUNCHER_DIR/Hermes Disk Search.command" ]; then
    write_desktop_launcher "$DESKTOP/Hermes Disk Search.command" "$LAUNCHER_DIR/Hermes Disk Search.command" "web UI"
else
    echo "[--] shortcuts/macos/Hermes Disk Search.command not found - desktop UI launcher skipped"
fi
if [ -f "$LAUNCHER_DIR/Индексация дисков.command" ]; then
    write_desktop_launcher "$DESKTOP/Индексация дисков.command" "$LAUNCHER_DIR/Индексация дисков.command" "index"
fi

APP_SRC="$LAUNCHER_DIR/HermesDiskSearchIndex.app"
APP_DST="$HOME/Applications/HermesDiskSearchIndex.app"
if [ -d "$APP_SRC" ]; then
    mkdir -p "$HOME/Applications"
    rm -rf "$APP_DST"
    cp -R "$APP_SRC" "$APP_DST"
    # Зашиваем корень в исполняемый файл бандла (иначе из ~/Applications он не найдёт архив).
    cat > "$APP_DST/Contents/MacOS/run_index" <<EOS
#!/bin/bash
# Создано hermes-disk-search (install_macos.command).
export HDS_ROOT="$ROOT"
exec "$APP_SRC/Contents/MacOS/run_index"
EOS
    chmod +x "$APP_DST/Contents/MacOS/run_index"
    if command -v codesign >/dev/null 2>&1; then
        if codesign --force --deep -s - "$APP_DST" >/dev/null 2>&1; then
            echo "[ok] app installed & ad-hoc signed: $APP_DST"
        else
            echo "[--] app copied, but codesign failed: $APP_DST"
        fi
    else
        echo "[ok] app installed: $APP_DST (codesign unavailable)"
    fi
else
    echo "[--] shortcuts/macos/HermesDiskSearchIndex.app not found - app skipped"
fi

# --- 6. LaunchAgents (watcher + MCP + llm-host) -------------------------------
# Аналог Windows-автозапуска (install_autostart.ps1 + install_llm_host_task.ps1):
# LaunchAgent-ы поднимают watcher, общий MCP (:8787) и резидент llm-host при входе.
# Отключить автозапуск: HDS_NO_AUTOSTART=1 перед запуском установщика.
AGENTS_DIR="$HOME/Library/LaunchAgents"
LOG_DIR="$HOME/Library/Logs/hermes-disk-search"
if [ "${HDS_NO_AUTOSTART:-}" = "1" ]; then
    echo "[--] HDS_NO_AUTOSTART=1 - LaunchAgents skipped"
else
    mkdir -p "$AGENTS_DIR" "$LOG_DIR"
    write_agent() {  # label  logname  program  args...
        label="$1"; logname="$2"; shift 2
        plist="$AGENTS_DIR/$label.plist"
        {
            echo '<?xml version="1.0" encoding="UTF-8"?>'
            echo '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
            echo '<plist version="1.0">'
            echo '<dict>'
            echo "  <key>Label</key><string>$label</string>"
            echo '  <key>ProgramArguments</key>'
            echo '  <array>'
            for a in "$@"; do printf '    <string>%s</string>\n' "$a"; done
            echo '  </array>'
            echo '  <key>EnvironmentVariables</key>'
            echo "  <dict><key>HDS_ROOT</key><string>$ROOT</string></dict>"
            echo "  <key>WorkingDirectory</key><string>$ROOT</string>"
            echo '  <key>RunAtLoad</key><true/>'
            echo '  <key>KeepAlive</key><true/>'
            echo "  <key>StandardOutPath</key><string>$LOG_DIR/$logname.log</string>"
            echo "  <key>StandardErrorPath</key><string>$LOG_DIR/$logname.err.log</string>"
            echo '</dict>'
            echo '</plist>'
        } > "$plist"
        if command -v launchctl >/dev/null 2>&1; then
            launchctl unload "$plist" >/dev/null 2>&1 || true
            if launchctl load -w "$plist" >/dev/null 2>&1; then
                echo "[ok] LaunchAgent loaded: $label"
            else
                echo "[--] LaunchAgent written (load failed): $plist"
            fi
        else
            echo "[ok] LaunchAgent written: $plist"
        fi
    }
    if [ -x "$BIN_DIR/hds" ]; then
        write_agent local.hds.watch watch "$BIN_DIR/hds" watch
        write_agent local.hds.mcp mcp "$BIN_DIR/hds" mcp-http run
    else
        echo "[--] $BIN_DIR/hds missing - watcher/MCP LaunchAgents skipped"
    fi
    if [ -x "$BIN_DIR/llm_host" ]; then
        write_agent local.hds.llmhost llmhost "$BIN_DIR/llm_host" run
    else
        echo "[--] $BIN_DIR/llm_host missing - llm-host LaunchAgent skipped"
    fi
fi

echo ""
# --- 7. Cline integration: MCP + model context windows + rule + skill ----------
# Windows does this from setup.ps1 -> install_cline.ps1. On macOS the same single
# code path (`hds cline-sync`, also the UI button) is run by install_cline_macos.sh:
# models.json contextWindow/maxInputTokens are aligned with llm-host ctx_per_slot,
# both MCP settings files get the disk-search server, the always-on rule and the
# skill are installed. Cline must be restarted afterwards (the command says so).
if [ -f "$ROOT/installers/install_cline_macos.sh" ]; then
    bash "$ROOT/installers/install_cline_macos.sh" || echo "[--] Cline integration skipped/failed"
fi

echo ""
echo "== Done. =="
echo "   Web UI:  double-click 'Hermes Disk Search' on the Desktop (or: HDS_ROOT=\"$ROOT\" hds ui)"
echo "   Index:   'Индексация дисков' on the Desktop (or: HDS_ROOT=\"$ROOT\" hds index)"
echo "   Check:   HDS_CONFIG=\"$ROOT/config.yaml\" hds check"
