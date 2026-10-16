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

# YAML single-quoted scalar (backslashes stay literal; only ' is doubled).
yq_single() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/''/g")"; }

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

# Server config body (mirrors install_server.ps1 New-ServerConfig).
gen_config() {
    echo "profile: server"
    echo "# Generated by install_server.command (M8 T2) $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "# Authorization is controlled by access.enabled only (profile does NOT enable it)."
    echo "access:"
    if [ "$ENABLE_ACCESS" = "yes" ]; then echo "  enabled: true"; else echo "  enabled: false"; fi
    echo "  token_ttl_days: 90"
    if [ "$PARALLEL" -gt 0 ] 2>/dev/null; then echo "llm_server:"; echo "  parallel: $PARALLEL"; fi
    if [ -n "$GPU_PRESET" ]; then
        echo "gpu:"; echo "  preset: $(yq_single "$GPU_PRESET")"
    elif printf '%s' "$GPU_DEVICE" | grep -Eq '^[0-9]+$'; then
        echo "gpu:"; echo "  device_index: $GPU_DEVICE"
    fi
    GEN_MEDIA="no"
    if [ -n "$MEDIA_INBOX" ] || [ -n "$MEDIA_OUT" ]; then GEN_MEDIA="yes"; fi
    if [ -n "$ROOTS" ] || [ "$GEN_MEDIA" = "yes" ]; then
        echo "index:"
        if [ -n "$ROOTS" ]; then
            echo "  roots:"
            printf '%s\n' "$ROOTS" | tr ';,' '\n\n' | while IFS= read -r r; do
                r="$(printf '%s' "$r" | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')"
                [ -n "$r" ] && echo "    - $(yq_single "$r")"
            done
        fi
        if [ "$GEN_MEDIA" = "yes" ]; then
            echo "  exclude_paths:"
            [ -n "$MEDIA_INBOX" ] && echo "    - $(yq_single "$MEDIA_INBOX")"
            [ -n "$MEDIA_OUT" ] && echo "    - $(yq_single "$MEDIA_OUT")"
        fi
    fi
    if [ "$ENABLE_TRANSCRIBE" = "yes" ] || [ "$GEN_MEDIA" = "yes" ]; then
        echo "auto_transcribe:"
        if [ "$ENABLE_TRANSCRIBE" = "yes" ]; then echo "  enabled: true"; else echo "  enabled: false"; fi
        [ -n "$MEDIA_INBOX" ] && echo "  inbox_dir: $(yq_single "$MEDIA_INBOX")"
        [ -n "$MEDIA_OUT" ] && echo "  out_dir: $(yq_single "$MEDIA_OUT")"
        echo "  diarization: true"
        echo "  diarization_required: true"
        echo "  source_disposal: $SOURCE_DISPOSAL"
        echo "  pause_indexing: true"
    fi
    echo "mcp_http:"
    echo "  host: $(yq_single "$MCP_HOST")   # publish /mcp only via reverse proxy with TLS"
    echo "  port: $MCP_PORT"
    echo "ui:"
    echo "  host: $(yq_single "$UI_HOST")   # admin UI behind reverse proxy; do not publish directly"
    echo "  port: $UI_PORT"
}

CFG_BODY="$(gen_config)"

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
    printf '%s\n' "$CFG_BODY"
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

# --- State dir + config (never clobber an existing config). ---
mkdir -p "$STATE/data"
if [ ! -f "$CFG" ]; then
    printf '%s\n' "$CFG_BODY" > "$CFG"
    echo "[ok] config created: $CFG"
else
    echo "[ok] config exists (not overwritten): $CFG"
fi

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
        summary+=("[..] ffmpeg missing (not needed by the Rust pipeline; legacy Python paths only)")
    fi
else
    summary+=("[..] deps skipped: Homebrew not found (install tesseract tesseract-lang ffmpeg manually)")
fi

# --- Unattended fetch (fetch_*.ps1 need pwsh on macOS). ---
if [ "$FETCH" = "yes" ] && [ "$NOFETCH" != "yes" ]; then
    PWSH="$(command -v pwsh || true)"
    if [ -n "$PWSH" ]; then
        for f in fetch_engine_runtime.ps1 fetch_llm_models.ps1 fetch_whisper_model.ps1 fetch_diarization_model.ps1 fetch_clip_models.ps1; do
            s="$HERE/$f"
            if [ ! -f "$s" ]; then summary+=("[..] fetch skipped (not found): $f"); continue; fi
            if "$PWSH" -NoProfile -ExecutionPolicy Bypass -File "$s"; then summary+=("[ok] fetched: $f"); else summary+=("[!!] fetch failed: $f"); fi
        done
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
                summary+=("[ok]   HDS_CONFIG pinned in $PLIST (restart the daemon to apply)")
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

