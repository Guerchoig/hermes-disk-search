#!/bin/bash
# install_client.command - thin CLIENT installer for hermes-disk-search (M9 / Э9 / W7).
#
# Connects Cline (Desktop/CLI) to a REMOTE disk-search server: writes the MCP
# server entry (external URL + `Authorization: Bearer`) into Cline's settings and
# installs the rule + skill. NO engine / models / index / sidecar is installed -
# this is a THIN client; all heavy lifting stays on the server.
#
# Steps: locate hds -> GET <ServerUrl>/health -> hds cline-sync --server-url ...
# The token is passed via the HDS_CLINE_TOKEN environment variable (not on the
# command line) and is never printed. Idempotent; no sudo needed.
#
# Usage: bash install_client.command --server-url https://host/ --token <bearer> [--mcp-path /mcp] [--dry-run] [--skip-health]
set -u

usage() {
    cat <<'USAGE'
Usage: bash install_client.command --server-url URL --token TOKEN [--mcp-path /mcp] [--dry-run] [--skip-health]
  --server-url URL   remote disk-search server base URL (e.g. https://host/)
  --token TOKEN      Bearer token (issued on the server: hds token new --user <login>)
  --mcp-path PATH    MCP endpoint path (default: /mcp)
  --dry-run          show what would change, write nothing
  --skip-health      skip the GET <URL>/health check
USAGE
}

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_URL=""; TOKEN=""; MCP_PATH="/mcp"; DRYRUN="no"; SKIP_HEALTH="no"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --server-url) SERVER_URL="${2:-}"; shift 2 ;;
        --token) TOKEN="${2:-}"; shift 2 ;;
        --mcp-path) MCP_PATH="${2:-}"; shift 2 ;;
        --dry-run) DRYRUN="yes"; shift ;;
        --skip-health) SKIP_HEALTH="yes"; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "[!!] unknown option: $1"; usage; exit 2 ;;
    esac
done

# hds binary: bin/hds (archive) -> ~/.local/bin/hds -> PATH -> dev build.
find_hds() {
    for CAND in "$ROOT/bin/hds" "$HOME/.local/bin/hds" "$ROOT/target/release/hds" "$ROOT/target/debug/hds"; do
        [ -x "$CAND" ] && { printf '%s\n' "$CAND"; return 0; }
    done
    command -v hds 2>/dev/null && return 0
    return 1
}
HDS="$(find_hds)" || HDS=""
if [ -z "$HDS" ]; then
    echo "[!!] hds not found - unpack the client package first"
    exit 1
fi

if [ -z "$SERVER_URL" ]; then
    printf 'Server URL (e.g. https://host/): '
    read -r SERVER_URL
fi
[ -n "$SERVER_URL" ] || { echo "[!!] server URL is required"; exit 2; }
if [ -z "$TOKEN" ]; then
    printf 'Bearer token: '
    read -r TOKEN
fi
[ -n "$TOKEN" ] || { echo "[!!] token is required"; exit 2; }

BASE="${SERVER_URL%/}"
MCP_PATH="${MCP_PATH%/}"
SRV_VER=""
if [ "$SKIP_HEALTH" != "yes" ]; then
    echo "== checking connection: $BASE/health =="
    BODY="$(curl -s --max-time 10 "$BASE/health" 2>/dev/null || true)"
    CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$BASE/health" 2>/dev/null || true)"
    if [ "$CODE" = "200" ]; then
        echo "[ok] server reachable (200)"
        # W7: /health publishes the product version (M9) - read it for the check
        SRV_VER="$(printf '%s' "$BODY" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    elif [ -n "$CODE" ] && [ "$CODE" != "000" ]; then
        echo "[!!] server responded $CODE"
    else
        echo "[!!] server not reachable - check the URL/TLS/network (--skip-health to ignore)"
        exit 3
    fi
fi

# client vs server version (W7): warn and point at the matching package.
# The client package carries version.txt (written by build_client_release_macos.sh).
LOCAL_VER=""
if [ -f "$ROOT/version.txt" ]; then
    LOCAL_VER="$(tr -d ' \t\r\n' < "$ROOT/version.txt")"
fi
if [ -n "$LOCAL_VER" ] && [ -n "$SRV_VER" ]; then
    if [ "$LOCAL_VER" = "$SRV_VER" ]; then
        echo "[ok] client/server versions match ($LOCAL_VER)"
    else
        echo "[!!] version mismatch: client $LOCAL_VER, server $SRV_VER"
        echo "     get the matching package: $BASE$MCP_PATH/client-package.zip (with your token) or the release asset"
    fi
elif [ -z "$LOCAL_VER" ]; then
    echo "[i] no version.txt here (dev layout) - version check skipped"
fi

echo "== writing Cline settings (MCP + rule + skill) =="
export HDS_CLINE_TOKEN="$TOKEN"
ARGS=(cline-sync --server-url "$SERVER_URL" --mcp-path "$MCP_PATH")
[ "$DRYRUN" = "yes" ] && ARGS+=(--dry-run)
HDS_ROOT="$ROOT" "$HDS" "${ARGS[@]}" \
    || echo "[!!] cline-sync reported warnings - see the lines above"
unset HDS_CLINE_TOKEN

echo ""
echo "Restart Cline Desktop (or start a new session) for the changes to take effect."
