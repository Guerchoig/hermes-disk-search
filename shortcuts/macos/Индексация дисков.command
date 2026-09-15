#!/bin/bash
# hermes-disk-search: запуск индексации дисков (macOS)
# Скрипт находит корень проекта: рядом с собой (2 уровня вверх) или ~/hermes-disk-search
SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT=""
for CAND in "$SELF_DIR/../.." "$HOME/hermes-disk-search"; do
    if [ -f "$CAND/config.yaml" ]; then ROOT="$(cd "$CAND" && pwd)"; break; fi
done
if [ -z "$ROOT" ]; then
    echo "== Проект не найден. Запустите install_macos.command или клонируйте в ~/hermes-disk-search =="
    read -n 1 -s -r -p "Нажмите любую клавишу..."
    exit 1
fi
PY="$ROOT/.venv/bin/python"
if [ ! -x "$PY" ]; then
    echo "== venv не найден. Сначала запустите install_macos.command =="
    read -n 1 -s -r -p "Нажмите любую клавишу..."
    exit 1
fi
echo "== hermes-disk-search: индексация (инкрементальная) =="
cd "$ROOT" && "$PY" -m hds.cli index
read -n 1 -s -r -p "Готово. Нажмите любую клавишу..."