#!/bin/bash
# Интеграция disk-search с Cline Desktop / Cline CLI для macOS — аналог install_cline.ps1.
# Можно запускать В ЛЮБОЙ МОМЕНТ: до установки Cline (запустить повторно после)
# или после. Регистрирует:
#   1) MCP-сервер disk-search (подключение по URL общего http-инстанса :8787)
#      в ~/.cline/data/settings/cline_mcp_settings.json
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

# Один общий http-инстанс MCP (:8787): Cline подключается по URL и НЕ запускает
# собственный процесс (при stdio каждая сессия плодила процесс, а hub — сирот).
# restart-if-stale вместо start: живой инстанс переиспользуется, но если на порту
# работает СТАРЫЙ код (проект обновили) — сервер перезапускается.
MCP_URL="$("$PY" -c "import sys; sys.path.insert(0, '$ROOT'); from hds import mcp_http; from hds.config import load; print(mcp_http.url(load()))" 2>/dev/null)"
if [ -z "$MCP_URL" ]; then
    echo "[--] Не удалось определить URL MCP-сервера — настройки Cline не изменены"
    exit 1
fi
MCP_RAW="$( cd "$ROOT" && "$PY" -m hds.cli mcp-http restart-if-stale 2>&1 )" || true
if printf '%s' "$MCP_RAW" | grep -q '"action": *"started"'; then
    echo "[ok] Общий MCP-сервер ($MCP_URL) поднят"
elif printf '%s' "$MCP_RAW" | grep -q '"action": *"restarted"'; then
    echo "[ok] Общий MCP-сервер ($MCP_URL) перезапущен (на порту был старый код)"
elif printf '%s' "$MCP_RAW" | grep -q '"action": *"reused"'; then
    echo "[ok] Общий MCP-сервер ($MCP_URL) уже актуален — переиспользован"
else
    echo "[!!] MCP-сервер: $MCP_RAW"
fi
"$PY" "$ROOT/installers/cline_mcp_merge.py" \
    --mode http --url "$MCP_URL" \
    --targets "$CLINE_DIR/data/settings/cline_mcp_settings.json" "$CLINE_DIR/mcp.json" \
    || { echo "[--] Ошибка правки настроек MCP Cline — см. сообщение выше"; exit 1; }

# --- Контроль глазами самого Cline (если CLI в PATH) ---
# Cline валидирует файл настроек ЦЕЛИКОМ: одна неверная запись = теряются ВСЕ
# MCP-серверы, поэтому проверяем не только форму (её контролирует
# cline_mcp_merge.py), но и то, что клиент принимает файл.
if command -v cline >/dev/null 2>&1; then
    CLINE_CHECK="$(cline config mcp --json 2>&1 || true)"
    if printf '%s' "$CLINE_CHECK" | grep -q 'Invalid MCP settings\|"type": *"error"'; then
        echo "[!!] Cline считает настройки MCP невалидными — он отбросит файл целиком:"
        echo "     $CLINE_CHECK"
        echo "     Файл: $CLINE_DIR/data/settings/cline_mcp_settings.json"
    elif printf '%s' "$CLINE_CHECK" | grep -q 'disk-search'; then
        echo "[ok] Cline видит сервер disk-search"
    else
        echo "[--] Cline не перечислил disk-search — проверьте MCP Servers в приложении"
    fi
fi

# --- Скилл disk-search ---
if [ -f "$ROOT/hermes-skill/disk-search.md" ]; then
    mkdir -p "$CLINE_DIR/skills/disk-search"
    cp "$ROOT/hermes-skill/disk-search.md" "$CLINE_DIR/skills/disk-search/SKILL.md"
    echo "[ok] Скилл установлен: $CLINE_DIR/skills/disk-search/SKILL.md"
else
    echo "[--] hermes-skill/disk-search.md не найден в проекте — скилл пропущен"
fi

echo "Перезапустите Cline Desktop (или начните новую сессию), чтобы изменения вступили в силу."
