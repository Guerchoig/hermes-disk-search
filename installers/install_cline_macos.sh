#!/bin/bash
# Интеграция disk-search с Cline Desktop / Cline CLI для macOS — аналог install_cline.ps1.
# Можно запускать В ЛЮБОЙ МОМЕНТ: до установки Cline (запустить повторно после)
# или после. Регистрирует:
#   1) MCP-сервер disk-search в ~/.cline/data/settings/cline_mcp_settings.json
#      (Desktop/CLI) и в ~/.cline/mcp.json, если файл используется (вариант CLI);
#   2) скилл disk-search: hermes-skill/disk-search.md ->
#      ~/.cline/skills/disk-search/SKILL.md
# Идемпотентно: повторный запуск обновляет записи, не дублируя их.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLINE_DIR="$HOME/.cline"

if [ ! -d "$CLINE_DIR" ] && ! command -v cline >/dev/null 2>&1; then
    echo "[--] Cline не найден ($CLINE_DIR отсутствует, 'cline' в PATH нет)."
    echo "    Это нормально, если disk-search установлен раньше Cline."
    echo "    Когда установите Cline Desktop (https://cline.bot/desktop), подключение"
    echo "    делается одной командой:"
    echo "      bash \"$ROOT/installers/install_cline_macos.sh\""
    echo "    Либо вручную (см. README, раздел «Интеграция с Cline Desktop»)."
    exit 0
fi
echo "== Подключение disk-search к Cline ($CLINE_DIR) =="

PY="$ROOT/.venv/bin/python"
[ -x "$PY" ] || { echo "[--] venv не найден ($PY) — сначала запустите install_macos.command"; exit 1; }

"$PY" "$ROOT/installers/cline_mcp_merge.py" \
    --python "$PY" --mcp-start "$ROOT/mcp_start.py" \
    --targets "$CLINE_DIR/data/settings/cline_mcp_settings.json" "$CLINE_DIR/mcp.json" \
    || { echo "[--] Ошибка правки настроек MCP Cline — см. сообщение выше"; exit 1; }

# --- Скилл disk-search ---
if [ -f "$ROOT/hermes-skill/disk-search.md" ]; then
    mkdir -p "$CLINE_DIR/skills/disk-search"
    cp "$ROOT/hermes-skill/disk-search.md" "$CLINE_DIR/skills/disk-search/SKILL.md"
    echo "[ok] Скилл установлен: $CLINE_DIR/skills/disk-search/SKILL.md"
else
    echo "[--] hermes-skill/disk-search.md не найден в проекте — скилл пропущен"
fi

echo "Перезапустите Cline Desktop (или начните новую сессию), чтобы изменения вступили в силу."
