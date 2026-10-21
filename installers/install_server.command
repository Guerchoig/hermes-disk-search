#!/bin/bash
# install_server.command -- server-profile installer for hermes-disk-search (M2 T4, M8 T2).
#
# Installs role daemons (watch | mcp-http | ui | llm-host) as LaunchDaemons
# (/Library/LaunchDaemons, RunAtLoad + KeepAlive) that start at boot WITHOUT
# user logon, via `hds service install <role>` (writes the plist + bootstrap).
# Also prepares the server state dir with a config generated from parameters,
# optionally downloads the engine runtime / models (fetch_*.ps1 via pwsh), and
# can enable access + create the first admin user. Ad-hoc signing of the bundled
# binaries and Gatekeeper-quarantine clearing are best effort.
#
# A no-option run keeps the ORIGINAL M2 behaviour (simple template, no fetch,
# no access). Use --whatif to preview the plan + generated config WITHOUT
# changes and WITHOUT root. Use --roles "" for a config-only install.
#
# Usage: sudo bash install_server.command [root_dir] [roles] [options]
#   --statedir DIR        state dir (default: /Library/Application Support/hermes-disk-search)
#   --mcp-host H --mcp-port N --ui-host H --ui-port N
#   --parallel N          chat slots (writes llm_server.parallel)
#   --gpu-device N | --gpu-preset P
#   --roots "a;b"         index roots, separated by ; or ,
#   --enable-access --admin-login L --admin-password P
#   --fetch | --no-fetch  download runtime/models (needs pwsh)
#   --whatif | --help
set -u

usage() {
    cat <<'USAGE'
Usage: sudo bash install_server.command [root_dir] [roles] [options]
Options:
  --statedir DIR            state dir (default: /Library/Application Support/hermes-disk-search)
  --mcp-host H --mcp-port N --ui-host H --ui-port N
  --parallel N              chat slots (writes llm_server.parallel)
  --gpu-device N | --gpu-preset P
  --roots "a;b"             index roots, separated by ; or ,
  --enable-access --admin-login L --admin-password P
  --fetch | --no-fetch      download runtime/models (needs pwsh)
  --skip-deps               do NOT install machine deps (tesseract/tesseract-lang/ffmpeg)
  --no-config-pin           keep the root config.yaml and do NOT pin HDS_CONFIG for daemons
  --whatif | --help
USAGE
}

ROOT=""
ROLES="watch,mcp-http,ui,llm-host,transcribe"
STATEDIR=""
MCP_HOST="127.0.0.1"; MCP_PORT="8787"
UI_HOST="127.0.0.1"; UI_PORT="8765"
PARALLEL="0"
GPU_DEVICE=""; GPU_PRESET=""
ROOTS=""
MEDIA_INBOX=""; MEDIA_OUT=""; ENABLE_TRANSCRIBE="no"; SOURCE_DISPOSAL="delete"
ENABLE_ACCESS="no"
ADMIN_LOGIN=""; ADMIN_PASSWORD=""
FETCH="no"; NOFETCH="no"; WHATIF="no"
# 0.20.2 (incident 15.10.2026): services need their config and their deps.
SKIP_DEPS="no"; NO_CONFIG_PIN="no"
POS=()

while [ "$#" -gt 0 ]; do
    case "$1" in
        --statedir) STATEDIR="${2:-}"; shift 2 ;;
        --mcp-host) MCP_HOST="${2:-}"; shift 2 ;;
        --mcp-port) MCP_PORT="${2:-}"; shift 2 ;;
        --ui-host) UI_HOST="${2:-}"; shift 2 ;;
        --ui-port) UI_PORT="${2:-}"; shift 2 ;;
        --parallel) PARALLEL="${2:-}"; shift 2 ;;
        --gpu-device) GPU_DEVICE="${2:-}"; shift 2 ;;
        --gpu-preset) GPU_PRESET="${2:-}"; shift 2 ;;
        --roots) ROOTS="${2:-}"; shift 2 ;;
        --media-inbox) MEDIA_INBOX="${2:-}"; shift 2 ;;
        --media-out) MEDIA_OUT="${2:-}"; shift 2 ;;
        --enable-transcribe) ENABLE_TRANSCRIBE="yes"; shift ;;
        --source-disposal) SOURCE_DISPOSAL="${2:-}"; shift 2 ;;
        --roles) ROLES="${2:-}"; shift 2 ;;
        --enable-access) ENABLE_ACCESS="yes"; shift ;;
        --admin-login) ADMIN_LOGIN="${2:-}"; shift 2 ;;
        --admin-password) ADMIN_PASSWORD="${2:-}"; shift 2 ;;
        --fetch) FETCH="yes"; shift ;;
        --no-fetch) NOFETCH="yes"; shift ;;
        --skip-deps) SKIP_DEPS="yes"; shift ;;
        --no-config-pin) NO_CONFIG_PIN="yes"; shift ;;
        --whatif|--what-if) WHATIF="yes"; shift ;;
        --help|-h) usage; exit 0 ;;
        -*) echo "[!!] unknown option: $1"; usage; exit 2 ;;
        *) POS+=("$1"); shift ;;
    esac
done
# Backward compatibility: [root_dir] [roles] positional args.
if [ "${#POS[@]}" -ge 1 ] && [ -n "${POS[0]}" ]; then ROOT="${POS[0]}"; fi
if [ "${#POS[@]}" -ge 2 ] && [ -n "${POS[1]}" ]; then ROLES="${POS[1]}"; fi

HERE="$(cd "$(dirname "$0")" && pwd)"
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "$ROOT" ]; then ROOT="$SOURCE"; fi
INSTALL_TARGET="$ROOT"

# --- 0.20.3: register the daemons against a VERSION, not against the unpacked tree.
# LaunchDaemons keep an ABSOLUTE path to hds; when they point at <root>/bin/hds (a
# plain "unpack into the install folder" flow), update.command can not replace that
# file - it installs the new version into app/<ver> and switches app/current, so the
# daemons would keep running the old binaries (incident 16.10.2026). When the target
# is a folder DIFFERENT from the unpacked archive, lay the version out first (the
# same install_app_version.ps1 step update.ps1 uses) and use app/current below.
VERSIONED="no"; PLAN_LAYOUT="no"; LAYOUT_VER=""; LAYOUT_NOTE=""
if [ -f "$INSTALL_TARGET/app/current/bin/hds" ]; then
    VERSIONED="yes"
    LAYOUT_NOTE="versioned install detected: daemons run from $INSTALL_TARGET/app/current/bin"
elif [ "$INSTALL_TARGET" != "$SOURCE" ] && [ -f "$SOURCE/bin/hds" ]; then
    if [ -f "$SOURCE/version.txt" ]; then LAYOUT_VER="$(head -n 1 "$SOURCE/version.txt" | tr -d ' \r')"; fi
    if [ -z "$LAYOUT_VER" ] && [ -f "$INSTALL_TARGET/version.txt" ]; then LAYOUT_VER="$(head -n 1 "$INSTALL_TARGET/version.txt" | tr -d ' \r')"; fi
    if [ -n "$LAYOUT_VER" ]; then
        PLAN_LAYOUT="yes"
        LAYOUT_NOTE="version $LAYOUT_VER will be laid out under $INSTALL_TARGET/app/$LAYOUT_VER; daemons run from app/current/bin (updates and rollback switch app/current)"
    else
        LAYOUT_NOTE="no version.txt in the archive: flat layout (no versions kept; update.command re-points the daemons itself)"
    fi
else
    LAYOUT_NOTE="flat layout (the archive IS the install root): unpack elsewhere and pass <machine folder> to get the version layout; update.command re-points the daemons either way"
fi
if [ "$VERSIONED" = "yes" ]; then ROOT="$INSTALL_TARGET/app/current"; fi
HDS="$ROOT/bin/hds"

if [ -z "$STATEDIR" ]; then STATEDIR="/Library/Application Support/hermes-disk-search"; fi
STATE="$STATEDIR"
CFG="$STATE/config.yaml"
# Machine-wide asset dirs: LaunchDaemons run as root, so models must live in the
# state dir, not in the invoking admin's profile. Same layout as on Windows.
MODELS_DIR="$STATE/models"
ENGINE_DIR="$STATE/engine"
LLAMA_DIR="$STATE/llama-runtime"
CLIP_DIR="$INSTALL_TARGET/models/clip_onnx"

# The server config is NOT built in this script any more (0.20.6): see config_init()
# below - both installers call `hds config init`, which renders
# server_config.example.yaml (one generator, validated as YAML).

# set_index_key FILE KEY VALUE - set/replace "  KEY: 'VALUE'" in the top-level
# `index:` section (comments preserved); creates the section when missing.
set_index_key() {
    f="$1"; k="$2"; v="$3"
    if grep -q "^  ${k}:" "$f" 2>/dev/null; then
        sed -i '' "s|^  ${k}:.*$|  ${k}: '${v}'|" "$f"
    elif grep -q "^index:" "$f" 2>/dev/null; then
        awk -v k="$k" -v v="$v" '{print} /^index:$/ {print "  " k ": \x27" v "\x27"}' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
    else
        printf "\nindex:\n  %s: '%s'\n" "$k" "$v" >> "$f"
    fi
}

# `hds config init` - the ONE generator of the server config (Windows and macOS):
# renders <root>/server_config.example.yaml, validates the result as YAML and, with
# --merge-defaults, adds missing defaults (index.exclude_dirs) to an existing config.
#   config_init <hds> [--path CFG] --print        - render only (--whatif)
#   config_init <hds> [--path CFG] --merge-defaults
config_init() {
    exe="$1"; shift
    if [ "$ENABLE_ACCESS" = "yes" ]; then access_flag="true"; else access_flag="false"; fi
    set -- config init --profile server "$@" \
        --mcp-host "$MCP_HOST" --mcp-port "$MCP_PORT" \
        --ui-host "$UI_HOST" --ui-port "$UI_PORT" \
        --access-enabled "$access_flag" \
        --source-disposal "$SOURCE_DISPOSAL"
    [ -n "$ROOTS" ] && set -- "$@" --roots "$ROOTS"
    if [ "$PARALLEL" -gt 0 ] 2>/dev/null; then set -- "$@" --parallel "$PARALLEL"; fi
    if [ -n "$GPU_PRESET" ]; then
        set -- "$@" --gpu-preset "$GPU_PRESET"
    elif printf '%s' "$GPU_DEVICE" | grep -Eq '^[0-9]+$'; then
        set -- "$@" --gpu-device "$GPU_DEVICE"
    fi
    [ "$ENABLE_TRANSCRIBE" = "yes" ] && set -- "$@" --enable-transcribe
    [ -n "$MEDIA_INBOX" ] && set -- "$@" --media-inbox "$MEDIA_INBOX"
    [ -n "$MEDIA_OUT" ] && set -- "$@" --media-out "$MEDIA_OUT"
    "$exe" "$@"
}

# --- --whatif: print the plan and stop (no root, no changes). ---
if [ "$WHATIF" = "yes" ]; then
    echo "=== install_server.command (WhatIf, no changes) ==="
    echo "Root:      $ROOT"
    echo "Layout:    $LAYOUT_NOTE"
    echo "StateDir:  $STATE"
    if [ -f "$CFG" ]; then echo "Config:    $CFG (exists - will NOT be overwritten)"; else echo "Config:    $CFG (will be created)"; fi
    echo "Roles:     $ROLES"
    if [ "$FETCH" = "yes" ] && [ "$NOFETCH" != "yes" ]; then echo "Fetch:     yes (unattended fetch_*)"; else echo "Fetch:     no"; fi
    echo "Access:    enabled=$ENABLE_ACCESS"
    if [ -n "$ADMIN_LOGIN" ]; then echo "Admin:     $ADMIN_LOGIN"; else echo "Admin:     (none)"; fi
    if [ "$SKIP_DEPS" = "yes" ]; then
        echo "Deps:      skipped (--skip-deps)"
    else
        echo "Deps:      brew tesseract + tesseract-lang + ffmpeg (machine-wide)"
    fi
    if [ "$NO_CONFIG_PIN" = "yes" ]; then
        echo "ConfigPin: off (--no-config-pin): root config.yaml is left as is"
    else
        echo "ConfigPin: root config.yaml renamed aside; HDS_CONFIG pinned in each plist"
    fi
    echo ""
    echo "--- config.yaml (body) ---"
    # Rendered by `hds config init --print`: the SAME template and generator the
    # real install uses (no YAML is built in this script any more).
    if [ -x "$HDS" ]; then
        config_init "$HDS" --path "$CFG" --print
    else
        echo "[!!] --whatif needs hds to render server_config.example.yaml (not found at $HDS)"
        exit 1
    fi
    exit 0
fi

# --- root is required only when daemons will actually be installed. ---
if [ -n "$(printf '%s' "$ROLES" | tr -d ' ,')" ] && [ "$(id -u)" -ne 0 ]; then
    echo "[!!] run with sudo (LaunchDaemons require root)"
    exit 1
fi

# --- 0.20.3: lay the version out BEFORE the daemons are registered. ---
if [ "$PLAN_LAYOUT" = "yes" ]; then
    PWSH_LAYOUT="$(command -v pwsh 2>/dev/null || true)"
    if [ -z "$PWSH_LAYOUT" ]; then
        echo "[!!] the version layout needs pwsh (install PowerShell) - nothing was registered"
        exit 1
    fi
    echo "[..] laying out version $LAYOUT_VER: $SOURCE -> $INSTALL_TARGET/app/$LAYOUT_VER"
    if ! "$PWSH_LAYOUT" -NoProfile -ExecutionPolicy Bypass -File "$SOURCE/installers/install_app_version.ps1" \
            -From "$SOURCE" -Version "$LAYOUT_VER" -Root "$INSTALL_TARGET" -SetCurrent; then
        echo "[!!] version layout failed - nothing was registered"
        exit 1
    fi
    if [ -x "$INSTALL_TARGET/app/current/bin/hds" ]; then
        ROOT="$INSTALL_TARGET/app/current"
        HDS="$ROOT/bin/hds"
        echo "[ok] version layout: daemons run from $ROOT/bin"
    else
        echo "[!!] version layout not created - nothing was registered"
        exit 1
    fi
fi
if [ ! -x "$HDS" ]; then echo "[!!] hds binary not found at $HDS"; exit 1; fi

summary=()
summary+=("[ok] layout: $LAYOUT_NOTE")

# --- Gatekeeper quarantine / exec bit on the bundled binaries (best effort). ---
if command -v xattr >/dev/null 2>&1; then xattr -dr com.apple.quarantine "$ROOT" 2>/dev/null || true; fi
chmod +x "$HDS" 2>/dev/null || true

# --- State dir + config: rendered by `hds config init` (one generator for Windows
# and macOS). An existing config is never overwritten; --merge-defaults only adds
# missing defaults (index.exclude_dirs - the 0.20.5 incident: a stand without
# exclusions indexed $RECYCLE.BIN / System Volume Information). ---
mkdir -p "$STATE/data"
if ! config_init "$HDS" --path "$CFG" --merge-defaults; then
    echo "[!!] config generation failed (hds config init) - nothing was registered"
    exit 1
fi
summary+=("[ok] config: $CFG (hds config init)")

# --- 0.20.2 (incident 15.10.2026): a stray desktop config.yaml in the install
# root SHADOWS the server config (config_path() prefers <root>/config.yaml), so
# the daemons silently ran in desktop mode: no auth gate, another DB,
# "heartbeat not found". Rename it aside (reversible).
if [ "$NO_CONFIG_PIN" != "yes" ] && [ -f "$ROOT/config.yaml" ] && [ "$ROOT/config.yaml" != "$CFG" ]; then
    mv -f "$ROOT/config.yaml" "$ROOT/config.desktop.yaml.bak"
    echo "[ok] root config.yaml renamed aside: $ROOT/config.desktop.yaml.bak"
    summary+=("[ok] root config.yaml renamed aside (it shadowed the server config)")
fi

# --- Desktop LaunchAgents (per-user) are superseded by LaunchDaemons. ---
for u in /Users/*; do
    for f in "$u/Library/LaunchAgents/"com.hermes-disk-search.*.plist; do
        [ -f "$f" ] || continue
        launchctl bootout "gui/$(basename "$u")/$(basename "$f" .plist)" 2>/dev/null
        rm -f "$f"
        echo "[ok] removed LaunchAgent: $f"
    done
done

# --- Reverse-proxy samples (M3 T1): copy when caddy/nginx is found, else hint. ---
for px in caddy nginx; do
    file="nginx.conf"; [ "$px" = "caddy" ] && file="Caddyfile"
    if command -v "$px" >/dev/null 2>&1; then
        mkdir -p "$STATE/proxy"
        cp -f "$HERE/proxy/$file" "$STATE/proxy/$file"
        echo "[ok] proxy sample copied: $STATE/proxy/$file"
    else
        echo "[..] $px not found - publish /mcp and /ui via TLS using samples in installers/proxy/"
    fi
done

# --- 0.20.2: machine deps for the daemons. They start with a minimal PATH, so a
# user-scope install is invisible to them; Homebrew is machine-wide and the
# tesseract/tessdata paths are written into config.yaml (index.ocr_* keys).
if [ "$SKIP_DEPS" = "yes" ]; then
    summary+=("[..] deps skipped (--skip-deps)")
elif [ "$(id -u)" -ne 0 ]; then
    summary+=("[!!] deps: run with sudo to install tesseract/tesseract-lang/ffmpeg")
elif command -v brew >/dev/null 2>&1; then
    brew list tesseract >/dev/null 2>&1 || brew install tesseract >/dev/null 2>&1 || true
    brew list tesseract-lang >/dev/null 2>&1 || brew install tesseract-lang >/dev/null 2>&1 || true
    brew list ffmpeg >/dev/null 2>&1 || brew install ffmpeg >/dev/null 2>&1 || true
    TESS="$(command -v tesseract || true)"
    if [ -n "$TESS" ]; then
        TD=""
        for cand in "$(dirname "$TESS")/../share/tessdata" /opt/homebrew/share/tessdata /usr/local/share/tessdata; do
            if [ -d "$cand" ]; then TD="$cand"; break; fi
        done
        set_index_key "$CFG" ocr_tesseract_cmd "$TESS"
        [ -n "$TD" ] && set_index_key "$CFG" ocr_tessdata "$TD"
        summary+=("[ok] Tesseract: $TESS (tessdata: ${TD:-next to the binary}, written into config.yaml)")
    else
        summary+=("[..] Tesseract missing: OCR of images/scans stays off (brew install tesseract tesseract-lang)")
    fi
    if command -v ffmpeg >/dev/null 2>&1; then
        summary+=("[ok] ffmpeg: $(command -v ffmpeg)")
    else
        summary+=("[!!] ffmpeg missing: diarization (nemotron3) cannot decode audio - brew install ffmpeg, then run 'hds check'")
    fi
else
    summary+=("[..] deps skipped: Homebrew not found (install tesseract tesseract-lang ffmpeg manually)")
fi

# --- Unattended fetch (fetch_*.ps1 need pwsh on macOS). ---
if [ "$FETCH" = "yes" ] && [ "$NOFETCH" != "yes" ]; then
    PWSH="$(command -v pwsh || true)"
    if [ -n "$PWSH" ]; then
        mkdir -p "$MODELS_DIR" "$ENGINE_DIR" "$LLAMA_DIR" "$CLIP_DIR"
        fetch_one() {
            f="$1"; shift
            s="$HERE/$f"
            if [ ! -f "$s" ]; then summary+=("[..] fetch skipped (not found): $f"); return 0; fi
            if "$PWSH" -NoProfile -ExecutionPolicy Bypass -File "$s" "$@"; then
                summary+=("[ok] fetched: $f")
            else
                summary+=("[!!] fetch failed: $f")
            fi
        }
        fetch_one fetch_engine_runtime.ps1 -EngineDir "$ENGINE_DIR"
        fetch_one fetch_llm_models.ps1 -RuntimeDir "$LLAMA_DIR"
        fetch_one fetch_whisper_model.ps1 -ModelsDir "$MODELS_DIR"
        fetch_one fetch_diarization_model.ps1 -ModelsDir "$MODELS_DIR"
        # W6: Nemotron-3 ONNX — the default diarization backend (works on macOS via
        # the CoreML/CPU execution providers of ONNX Runtime, W8).
        fetch_one fetch_nemotron3_model.ps1 -ModelsDir "$MODELS_DIR"
        fetch_one fetch_clip_models.ps1 -ModelsDir "$CLIP_DIR"
    else
        summary+=("[..] fetch skipped - pwsh (PowerShell) not found on macOS")
    fi
fi

# --- First admin user (access.enabled): hashed password, plain not logged. ---
if [ "$ENABLE_ACCESS" = "yes" ] && [ -n "$ADMIN_LOGIN" ] && [ -n "$ADMIN_PASSWORD" ]; then
    if HDS_CONFIG="$CFG" HDS_ROOT="$STATE" "$HDS" user add --login "$ADMIN_LOGIN" --password "$ADMIN_PASSWORD" --admin; then
        summary+=("[ok] admin user: $ADMIN_LOGIN (issue a token: $HDS token new --user $ADMIN_LOGIN)")
    elif HDS_CONFIG="$CFG" HDS_ROOT="$STATE" "$HDS" user passwd --login "$ADMIN_LOGIN" --password "$ADMIN_PASSWORD"; then
        summary+=("[ok] admin user updated: $ADMIN_LOGIN")
    else
        summary+=("[!!] admin user failed: $ADMIN_LOGIN")
    fi
fi

# --- Ad-hoc sign the bundled binaries (best effort; unsigned = Gatekeeper risk). ---
if command -v codesign >/dev/null 2>&1; then
    for b in "$ROOT"/bin/*; do
        [ -f "$b" ] || continue
        if codesign --force -s - "$b" >/dev/null 2>&1; then echo "[ok] codesign (ad-hoc): $b"; else echo "[..] codesign skipped: $b"; fi
    done
else
    echo "[..] codesign unavailable - binaries stay unsigned"
fi

# --- Install LaunchDaemons per role. ---
IFS=',' ; for role in $ROLES; do
    role="$(printf '%s' "$role" | xargs)"
    [ -n "$role" ] || continue
    if "$HDS" service install "$role"; then
        summary+=("[ok] daemon  $role (LaunchDaemon: com.hermes-disk-search.$role)")
        if [ "$NO_CONFIG_PIN" != "yes" ]; then
            # Pin the state config for THIS daemon: launchd applies the plist
            # EnvironmentVariables, so a stray root config.yaml can never flip
            # the role into desktop mode again.
            PLIST="/Library/LaunchDaemons/com.hermes-disk-search.$role.plist"
            if [ -f "$PLIST" ]; then
                /usr/libexec/PlistBuddy -c "Set :EnvironmentVariables:HDS_CONFIG $CFG" "$PLIST" 2>/dev/null \
                    || /usr/libexec/PlistBuddy -c "Add :EnvironmentVariables:HDS_CONFIG string $CFG" "$PLIST" 2>/dev/null \
                    || true
                for pair in "HDS_MODELS_DIR=$MODELS_DIR" "HDS_ENGINE_DIR=$ENGINE_DIR" "LLAMA_RUNTIME_DIR=$LLAMA_DIR"; do
                    k="${pair%%=*}"; v="${pair#*=}"
                    /usr/libexec/PlistBuddy -c "Set :EnvironmentVariables:$k $v" "$PLIST" 2>/dev/null \
                        || /usr/libexec/PlistBuddy -c "Add :EnvironmentVariables:$k string $v" "$PLIST" 2>/dev/null \
                        || true
                done
                summary+=("[ok]   config + model dirs pinned in $PLIST (restart the daemon to apply)")
            fi
        fi
    else
        summary+=("[!!] FAILED  $role (see messages above)")
    fi
done
unset IFS

echo ""
echo "=== Summary ==="
if [ "${#summary[@]}" -gt 0 ]; then
    for s in "${summary[@]}"; do echo "$s"; done
fi
echo ""
echo "Next steps:"
if [ "$ENABLE_ACCESS" = "yes" ] && [ -n "$ADMIN_LOGIN" ]; then
    echo "  - Issue an admin token:  $HDS token new --user $ADMIN_LOGIN"
fi
echo "  - Start now: $HDS service start <role>"
echo "  - The daemons read the state config (HDS_CONFIG is pinned in the plist)."
echo "  - Restart a role that was already running: $HDS service stop <role> + service start <role>"
echo "  - Status:    $HDS service status <role>"
echo "  - Put TLS reverse proxy in front of /mcp and /ui (facade 8010-8012 stays local):"
echo "    samples in installers/proxy/ (Caddyfile, nginx.conf); then set trusted_proxy: true."

