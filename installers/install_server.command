#!/bin/bash
# install_server.command -- server-profile installer for hermes-disk-search (M2 T4).
#
# Installs role daemons (watch | mcp-http | ui | llm-host) as LaunchDaemons
# (/Library/LaunchDaemons, RunAtLoad + KeepAlive) that start at boot WITHOUT
# user logon, via `hds service install <role>` (writes the plist + bootstrap).
# Also prepares the state dir (/Library/Application Support/hermes-disk-search)
# with a config template and removes the per-user LaunchAgents of the desktop
# install (best effort).
#
# Usage: sudo bash install_server.command [root_dir] [roles]
set -u

ROOT="${1:-}"
ROLES="${2:-watch,mcp-http,ui,llm-host}"

if [ -z "$ROOT" ]; then
    ROOT="$(cd "$(dirname "$0")/.." && pwd)"
fi
HDS="$ROOT/bin/hds"
if [ ! -x "$HDS" ]; then
    echo "[!!] hds binary not found at $HDS"
    exit 1
fi
if [ "$(id -u)" -ne 0 ]; then
    echo "[!!] run with sudo (LaunchDaemons require root)"
    exit 1
fi

# --- State dir (M2 T2): /Library/Application Support/hermes-disk-search ---
STATE="/Library/Application Support/hermes-disk-search"
mkdir -p "$STATE/data"

CFG="$STATE/config.yaml"
if [ ! -f "$CFG" ]; then
    cat > "$CFG" <<'EOF'
profile: server
# Server profile: state (index.db, data/) lives in this directory.
# Authorization is controlled by access.enabled only (profile does NOT enable it):
# access:
#   enabled: true
mcp_http:
  host: "127.0.0.1"   # publish /mcp only via reverse proxy with TLS
  port: 8787
ui:
  host: "127.0.0.1"   # admin UI behind reverse proxy; do not publish directly
  port: 8765
EOF
    echo "[ok] config template: $CFG"
else
    echo "[ok] config exists: $CFG"
fi

# --- Desktop LaunchAgents (per-user, AtLogOn equivalent) are superseded ---
# --- by LaunchDaemons; remove ours, keep foreign ones untouched.         ---
for u in /Users/*; do
    for f in "$u/Library/LaunchAgents/"com.hermes-disk-search.*.plist; do
        [ -f "$f" ] || continue
        launchctl bootout "gui/$(basename "$u")/$(basename "$f" .plist)" 2>/dev/null
        rm -f "$f"
        echo "[ok] removed LaunchAgent: $f"
    done
done

# --- Reverse-proxy samples (M3 T1): copy when caddy/nginx is found, else hint. ---
PROXY_SRC="$(cd "$(dirname "$0")" && pwd)/proxy"
for px in caddy nginx; do
    file="nginx.conf"; [ "$px" = "caddy" ] && file="Caddyfile"
    if command -v "$px" >/dev/null 2>&1; then
        mkdir -p "$STATE/proxy"
        cp -f "$PROXY_SRC/$file" "$STATE/proxy/$file"
        echo "[ok] proxy sample copied: $STATE/proxy/$file"
        echo "     edit domain/certs, then run it and set trusted_proxy: true in config.yaml"
    else
        echo "[..] $px not found - publish /mcp and /ui via TLS using samples in installers/proxy/"
    fi
done

# --- Install LaunchDaemons per role. ---
IFS=',' ; for role in $ROLES; do
    role="$(echo "$role" | xargs)"
    [ -n "$role" ] || continue
    "$HDS" service install "$role" && echo "[ok] daemon  $role (LaunchDaemon: com.hermes-disk-search.$role)" \
        || echo "[!!] FAILED  $role (see messages above)"
done

echo
echo "Next steps:"
echo "  - Start now: $HDS service start <role>"
echo "  - Status:    $HDS service status <role>"
echo "  - Put TLS reverse proxy in front of /mcp and /ui (facade 8010-8012 stays local):"
echo "    samples in installers/proxy/ (Caddyfile, nginx.conf); then set trusted_proxy: true."