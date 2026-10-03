#!/bin/bash
# hermes-disk-search: веб-интерфейс (macOS, Rust-бинарник hds).
# Аналог Windows run_ui.ps1: если сервер уже отвечает на порту — открыть страницу,
# иначе поднять `hds ui`, дождаться /api/status и открыть браузер.
# Python-ядро в поставке отсутствует; Python остаётся только воркером-экстрактором.
SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
# Корень можно задать заранее через HDS_ROOT (так делают обёртки установщика).
ROOT="${HDS_ROOT:-}"
if [ -z "$ROOT" ]; then
    for CAND in "$SELF_DIR/../.." "$HOME/hermes-disk-search"; do
        if [ -f "$CAND/config.yaml" ] || [ -x "$CAND/bin/hds" ] || [ -f "$CAND/config.example.yaml" ]; then
            ROOT="$(cd "$CAND" && pwd)"; break
        fi
    done
fi
if [ -z "$ROOT" ]; then
    osascript -e 'display dialog "Проект hermes-disk-search не найден. Распакуйте релизный архив или установите в ~/hermes-disk-search" buttons {"OK"}'
    exit 1
fi

# Rust-бинарник: bin/hds (архив) -> ~/.local/bin/hds (установка) -> PATH -> dev-сборка.
find_hds() {
    for CAND in "$ROOT/bin/hds" "$HOME/.local/bin/hds" "$ROOT/target/release/hds" "$ROOT/target/debug/hds"; do
        [ -x "$CAND" ] && { printf '%s\n' "$CAND"; return 0; }
    done
    command -v hds 2>/dev/null && return 0
    return 1
}
HDS="$(find_hds)" || {
    osascript -e 'display dialog "bin/hds не найден. Сначала запустите install_macos.command" buttons {"OK"}'
    exit 1
}

# Порт как в Windows run_ui.ps1.
PORT=8765
URL="http://127.0.0.1:$PORT"
LOG_DIR="$HOME/Library/Logs/hermes-disk-search"
mkdir -p "$LOG_DIR"

ui_up() { curl -fsS --max-time 3 "$URL/api/status" >/dev/null 2>&1; }

if ui_up; then
    open "$URL"
    exit 0
fi

echo "[..] запускаю веб-интерфейс на порту $PORT..."
( cd "$ROOT" && HDS_ROOT="$ROOT" nohup "$HDS" ui --port "$PORT" >"$LOG_DIR/ui.log" 2>"$LOG_DIR/ui.err.log" & )
up=""
for _ in $(seq 1 15); do
    sleep 1
    if ui_up; then up=1; break; fi
done
if [ -z "$up" ]; then
    osascript -e 'display dialog "Веб-интерфейс не запустился (порт 8765). См. ~/Library/Logs/hermes-disk-search/ui.err.log" buttons {"OK"}'
    exit 1
fi
open "$URL"
