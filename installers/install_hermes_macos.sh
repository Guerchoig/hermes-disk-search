#!/bin/bash
# Интеграция disk-search с Hermes Desktop для macOS — аналог install_hermes.ps1.
# Можно запускать В ЛЮБОЙ МОМЕНТ: до установки Hermes (запустить повторно после)
# или после. Регистрирует:
#   1) MCP-сервер disk-search (подключение по URL общего http-инстанса :8787)
#      в <Hermes>/config.yaml (секция mcp_servers)
#   2) скилл disk-search (правило «поиск файлов — через MCP, а не grep»)
#   3) tools.tool_search.enabled: "off" — все инструменты всегда в промпте
# Идемпотентно: повторный запуск обновляет блоки, не дублируя их.
# Использование: bash install_hermes_macos.sh [путь-к-каталогу-Hermes]
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HERMES_DIR="${1:-${HERMES_DIR:-}}"

if [ -z "$HERMES_DIR" ]; then
    for CAND in "$HOME/Library/Application Support/hermes" \
                "$HOME/hermes" "$HOME/.hermes"; do
        if [ -f "$CAND/config.yaml" ]; then HERMES_DIR="$CAND"; break; fi
    done
fi
CFG="$HERMES_DIR/config.yaml"

if [ -z "$HERMES_DIR" ] || [ ! -f "$CFG" ]; then
    echo "[--] Hermes Desktop не найден (config.yaml не найден)."
    echo "    Это нормально, если disk-search установлен раньше Hermes."
    echo "    Когда установите Hermes Desktop, подключение делается одной командой:"
    echo "      bash \"$ROOT/installers/install_hermes_macos.sh\""
    echo "    Либо вручную (см. README, раздел «Интеграция с Hermes»):"
    echo "      1) в <Hermes>/config.yaml в секцию mcp_servers добавить блок disk-search;"
    echo "      2) скопировать hermes-skill/SKILL.md в <Hermes>/skills/disk-search/SKILL.md;"
    echo "      3) добавить tools.tool_search.enabled: \"off\" (все инструменты в промпте)."
    exit 0
fi
echo "== Подключение disk-search к Hermes ($HERMES_DIR) =="

# --- 1. MCP-сервер + tools.tool_search (правка config.yaml с сохранением комментариев) ---
PY="$ROOT/.venv/bin/python"
[ -x "$PY" ] || PY="$(command -v python3)"
# Один общий http-инстанс MCP (:8787): Hermes подключается по URL и не запускает
# свой процесс на каждую сессию (живой инстанс переиспользуется).
# restart-if-stale вместо start — как в install_hermes.ps1: если на порту работает
# СТАРЫЙ код (проект обновили), сервер перезапускается, а не переиспользуется.
MCP_URL="$("$PY" -c "import sys; sys.path.insert(0, '$ROOT'); from hds import mcp_http; from hds.config import load; print(mcp_http.url(load()))" 2>/dev/null)"
( cd "$ROOT" && "$PY" -m hds.cli mcp-http restart-if-stale >/dev/null 2>&1 ) || true
"$PY" - "$CFG" "$ROOT/.venv/bin/python" "$ROOT/mcp_start.py" "$MCP_URL" <<'PYEOF'
import re, sys

cfg_path, venv_py, mcp_start, mcp_url = (list(sys.argv[1:5]) + [""])[:4]
with open(cfg_path, encoding="utf-8-sig") as f:
    text = f.read()

if mcp_url:
    mcp_block = ("  disk-search:\n"
                 "    url: %s\n"
                 "    timeout: 300\n" % mcp_url)
else:
    mcp_block = ("  disk-search:\n"
                 "    command: %s\n"
                 "    args:\n"
                 "      - %s\n"
                 "    timeout: 300\n" % (venv_py, mcp_start))
rx_block = re.compile(r"(?m)^  disk-search:\r?\n(?:    [^\r\n]*\r?\n?)*")
if rx_block.search(text):
    text = rx_block.sub(lambda m: mcp_block, text, count=1)
    print("[ok] MCP disk-search: блок в config.yaml обновлён")
elif re.search(r"(?m)^mcp_servers:\s*$", text):
    text = re.sub(r"(?m)^mcp_servers:\s*$",
                  lambda m: m.group(0) + "\n" + mcp_block,
                  text, count=1)
    print("[ok] MCP disk-search: добавлен в существующую секцию mcp_servers")
else:
    text = text.rstrip("\r\n") + "\n\nmcp_servers:\n" + mcp_block
    print("[ok] MCP disk-search: секция mcp_servers добавлена в конец config.yaml")

ts_inner = "  tool_search:\n    enabled: \"off\"\n"
rx_ts = re.compile(r"(?m)^  tool_search:\r?\n(?:    [^\r\n]*\r?\n?)*")
if rx_ts.search(text):
    text = rx_ts.sub(lambda m: ts_inner, text, count=1)
    print("[ok] tools.tool_search: блок обновлён (enabled: off)")
elif re.search(r"(?m)^tools:\r?$", text):
    text = re.sub(r"(?m)^(tools:\r?\n)", lambda m: m.group(0) + ts_inner, text, count=1)
    print("[ok] tools.tool_search: добавлен в существующую секцию tools")
else:
    text += ("\n# Все инструменты всегда в промпте (локальная модель не находит "
             "MCP-инструменты через discovery-протокол)\ntools:\n" + ts_inner)
    print("[ok] секция tools.tool_search добавлена в конец config.yaml")

tmp = cfg_path + ".tmp"
with open(tmp, "w", encoding="utf-8") as f:
    f.write(text)
import os
os.replace(tmp, cfg_path)

# валидация YAML
try:
    import yaml
    d = yaml.safe_load(open(cfg_path, encoding="utf-8-sig"))
    ds = (d.get("mcp_servers") or {}).get("disk-search")
    ts = ((d.get("tools") or {}).get("tool_search") or {})
    assert ds and (ds.get("url") or ds.get("command")), "mcp_servers.disk-search не на месте"
    assert ts.get("enabled") == "off", "tools.tool_search.enabled != off"
    print("[ok] config.yaml Hermes валиден, disk-search зарегистрирован")
except ImportError:
    print("[..] PyYAML недоступен — пропускаю валидацию")
except AssertionError as e:
    sys.exit("config.yaml Hermes повреждён после правки (%s) — проверьте вручную" % e)
PYEOF
if [ $? -ne 0 ]; then
    echo "[--] Ошибка правки config.yaml — см. сообщение выше"
    exit 1
fi

# --- 2. Скилл disk-search ---
if [ -f "$ROOT/hermes-skill/SKILL.md" ]; then
    mkdir -p "$HERMES_DIR/skills/disk-search"
    cp "$ROOT/hermes-skill/SKILL.md" "$HERMES_DIR/skills/disk-search/SKILL.md"
    echo "[ok] Скилл установлен: $HERMES_DIR/skills/disk-search/SKILL.md"
else
    echo "[--] hermes-skill/SKILL.md не найден в проекте — скилл пропущен"
fi

echo "Перезапустите Hermes Desktop (или начните новую сессию), чтобы изменения вступили в силу."