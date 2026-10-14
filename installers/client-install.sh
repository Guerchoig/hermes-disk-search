#!/bin/bash
# client-install.sh - ONE-COMMAND thin CLIENT installer for macOS/Linux (M9).
#
# The user runs this on the CLIENT machine with the token issued by the server
# admin. The script:
#   1) downloads the client package from the server
#      (GET <ServerUrl><McpPath>/client-package.zip?platform=macos-arm64,
#       header `Authorization: Bearer <token>`);
#   2) unpacks it locally and clears the quarantine attribute (macOS);
#   3) runs the package master (install_client.command): checks /health, compares
#      the client/server versions and writes the Cline MCP entry + rule + skill.
#
# This FILE needs no token to download: it is served publicly (release asset, or
# <ServerUrl><McpPath>/client-install.sh) - the user passes the token as an
# argument.
#
# Usage:
#   curl -f https://host/mcp/client-install.sh -o client-install.sh && \
#     bash client-install.sh --token <TOKEN> --server-url https://host
#
# ASCII-only on purpose.
set -u

usage() {
    cat <<'USAGE'
Usage: bash client-install.sh --token TOKEN [--server-url URL] [--mcp-path /mcp]
                              [--dir DIR] [--platform macos-arm64] [--dry-run]
  --token TOKEN      Bearer token issued by the server admin (hds token new)
  --server-url URL   remote disk-search server (e.g. https://host/); asked if omitted
  --mcp-path PATH    MCP endpoint path (default: /mcp)
  --dir DIR          where to unpack the client (default: ~/hermes-disk-search-client)
  --platform NAME    package platform (default: macos-arm64; windows-x64 for Windows)
  --dry-run          show what the master would change, write nothing
USAGE
}

TOKEN=""; SERVER_URL=""; MCP_PATH="/mcp"; DIR=""; PLATFORM="macos-arm64"; DRYRUN="no"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --token) TOKEN="${2:-}"; shift 2 ;;
        --token=*) TOKEN="${1#--token=}"; shift ;;
        --server-url) SERVER_URL="${2:-}"; shift 2 ;;
        --server-url=*) SERVER_URL="${1#--server-url=}"; shift ;;
        --mcp-path) MCP_PATH="${2:-}"; shift 2 ;;
        --dir) DIR="${2:-}"; shift 2 ;;
        --platform) PLATFORM="${2:-}"; shift 2 ;;
        --dry-run) DRYRUN="yes"; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "[!!] unknown option: $1"; usage; exit 2 ;;
    esac
done

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
[ -n "$DIR" ] || DIR="$HOME/hermes-disk-search-client"

BASE="${SERVER_URL%/}"
MCP="${MCP_PATH%/}"
URL="$BASE$MCP/client-package.zip?platform=$PLATFORM"
ZIP="$DIR/hds-client.zip"

echo "== hermes-disk-search thin client =="
echo "   server : $BASE"
echo "   package: $URL"
echo "   target : $DIR"

mkdir -p "$DIR"
echo "== downloading the client package (token goes in Authorization: Bearer) =="
if ! curl -fL --max-time 120 -H "Authorization: Bearer $TOKEN" -o "$ZIP" "$URL"; then
    echo "[!!] cannot download the package - check the URL/TLS/network"
    echo "     401 = token is wrong/expired/revoked (ask the admin for a new one)"
    echo "     404 = the server has no client package yet (older server or dev build):"
    echo "           take hds-client-<version>-<platform>.zip from the release page"
    exit 3
fi

echo "== unpacking =="
UNPACKED="$DIR/package"
rm -rf "$UNPACKED"
mkdir -p "$UNPACKED"
if command -v unzip >/dev/null 2>&1; then
    unzip -q -o "$ZIP" -d "$UNPACKED"
else
    tar -xf "$ZIP" -C "$UNPACKED"
fi
if command -v xattr >/dev/null 2>&1; then
    xattr -dr com.apple.quarantine "$UNPACKED" 2>/dev/null || true
fi
rm -f "$ZIP"

MASTER="$UNPACKED/install_client.command"
if [ ! -f "$MASTER" ]; then
    echo "[!!] install_client.command not found in the package ($UNPACKED)" >&2
    echo "     the server may have served a package for another platform (check --platform)" >&2
    exit 4
fi

echo "== connecting Cline to the server =="
ARGS=(--server-url "$BASE" --mcp-path "$MCP" --token "$TOKEN")
[ "$DRYRUN" = "yes" ] && ARGS+=(--dry-run)
bash "$MASTER" "${ARGS[@]}"
RC=$?

echo ""
if [ "$RC" -eq 0 ]; then
    echo "[ok] client installed into $UNPACKED"
else
    echo "[!!] the master reported problems (exit $RC) - see the lines above"
fi
echo "Restart Cline Desktop (or start a new session) for the changes to take effect."
exit "$RC"
