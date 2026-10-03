#!/bin/bash
# hermes-disk-search: запуск индексации дисков (macOS, Rust-бинарник hds).
# Python-ядро в поставке отсутствует; Python остаётся ТОЛЬКО в воркере-экстракторе (sidecar/).
# Корень проекта ищется рядом (2 уровня вверх) или в ~/hermes-disk-search.
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
    echo "== Проект не найден. Запустите install_macos.command или клонируйте в ~/hermes-disk-search =="
    read -n 1 -s -r -p "Нажмите любую клавишу..."
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
    echo "== bin/hds не найден. Сначала запустите install_macos.command =="
    read -n 1 -s -r -p "Нажмите любую клавишу..."
    exit 1
}
echo "== hermes-disk-search: индексация (инкрементальная) =="
cd "$ROOT" && HDS_ROOT="$ROOT" "$HDS" index
read -n 1 -s -r -p "Готово. Нажмите любую клавишу..."