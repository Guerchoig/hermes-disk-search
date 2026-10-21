#!/bin/bash
# update.command -- server UPDATE wrapper (macOS), the companion of update.cmd.
#
# Run THIS file (with sudo), not installers/update.ps1 directly:
#   1) it clears the Gatekeeper quarantine flag of this tree (a ZIP downloaded
#      from the internet keeps com.apple.quarantine on every extracted file);
#   2) it finds the INSTALL ROOT by itself: "bin/hds service root" reads the
#      ProgramArguments of the installed LaunchDaemon, so no path is typed;
#   3) it calls installers/update.ps1 -Profile server with -From <this folder>
#      and -Version from version.txt (pwsh = PowerShell on macOS is required).
#
# Usage: sudo bash installers/update.command [update.ps1 options]
#   -WhatIf                preview, no changes
#   -Force                 reinstall the same version
#   -RollbackTo 0.19.0     roll back to a kept version
#   -NoModels              skip the Nemotron-3 diarization model top-up
#   -Root /opt/hds         explicit root (no LaunchDaemon registered)
#
# The Nemotron-3 model is fetched by update.ps1 itself (F3): after the pointer
# switch it runs the NEW version's installers/fetch_nemotron3_model.ps1 into
# HDS_MODELS_DIR / the machine models dir. If it is skipped (-NoModels) or the
# download fails, the update still succeeds and `hds check` prints the command;
# to install the model manually:
#   sudo pwsh -NoProfile -File "$ROOT/app/current/installers/fetch_nemotron3_model.ps1"
#
# NOTE: keep this file ASCII-only (like the other installers).
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"     # .../installers
ARCH="$(cd "$HERE/.." && pwd)"            # archive root: bin/, installers/, version.txt
HDS="$ARCH/bin/hds"

if [ ! -x "$HDS" ]; then
    echo "[!!] $HDS not found or not executable - unpack the release archive first"
    exit 1
fi

VER="$(head -n 1 "$ARCH/version.txt" 2>/dev/null | tr -d '[:space:]')"
if [ -z "$VER" ]; then
    echo "[!!] version.txt not found in $ARCH - this is not a release archive"
    exit 1
fi

# Quarantine flags: best effort (the tree is ours; xattr may be absent).
if command -v xattr >/dev/null 2>&1; then
    echo "[..] clearing Gatekeeper quarantine flags (xattr -dr com.apple.quarantine)"
    xattr -dr com.apple.quarantine "$ARCH" >/dev/null 2>&1 || true
fi
chmod +x "$HDS" "$HERE/update.command" 2>/dev/null || true

# An explicit -Root on the command line wins: do not pass our own as well
# (PowerShell rejects the parameter twice).
ROOT_SET="no"
for a in "$@"; do
    case "$a" in
        -Root|--Root) ROOT_SET="yes" ;;
    esac
done

ROOT=""
if [ "$ROOT_SET" = "no" ]; then
    ROOT="$("$HDS" service root 2>/dev/null || true)"
    if [ -z "$ROOT" ]; then
        echo "[!!] could not detect the install root: no LaunchDaemon is registered"
        echo "     pass it explicitly:  sudo bash installers/update.command -Root /opt/hds"
        exit 1
    fi
    echo "[..] install root: $ROOT"
fi

PWSH="$(command -v pwsh || true)"
if [ -z "$PWSH" ]; then
    echo "[!!] pwsh (PowerShell) not found - update.ps1 needs it on macOS"
    echo "     install it:  brew install --cask powershell"
    exit 1
fi

ARG=()
if [ "$ROOT_SET" = "no" ]; then
    ARG=(-Root "$ROOT")
fi

echo "[..] updating to $VER from $ARCH"
# F3: the Nemotron-3 model is top-up fetched by update.ps1 after the switch
# (idempotent); -NoModels skips it. Manual path if the download failed:
echo "[..] Nemotron-3 model: installers/fetch_nemotron3_model.ps1 runs via update.ps1"
echo "     manual: sudo pwsh -NoProfile -File \"\$ROOT/app/current/installers/fetch_nemotron3_model.ps1\""
exec "$PWSH" -NoProfile -ExecutionPolicy Bypass -File "$HERE/update.ps1" \
    ${ARG[@]+"${ARG[@]}"} -Profile server -From "$ARCH" -Version "$VER" "$@"
