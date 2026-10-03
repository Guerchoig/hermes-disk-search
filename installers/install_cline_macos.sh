#!/bin/bash
# Интеграция disk-search с Cline Desktop / Cline CLI для macOS — аналог install_cline.ps1.
# Можно запускать В ЛЮБОЙ МОМЕНТ: до установки Cline (запустить повторно после)
# или после. Регистрирует:
#   1) MCP-сервер disk-search (подключение по URL общего http-инстанса :8787)
#      в ~/.cline/data/settings/cline_mcp_settings.json
#      (Desktop/CLI) и в ~/.cline/mcp.json, если файл используется (вариант CLI);
#   2) правило disk-search: cline-rules/disk-search.md -> ~/.cline/rules/disk-search.md
#      Правила, в отличие от скиллов, попадают в системный промпт КАЖДОЙ сессии, без
#      вызова use_skill: локальная агентная модель сама скилл не активирует и
#      отвечает по одному запросу — это и выглядит как «неполные результаты»;
#   3) скилл disk-search: hermes-skill/disk-search.md ->
#      ~/.cline/skills/disk-search/SKILL.md;
#   4) окна контекста моделей в ~/.cline/data/settings/models.json: contextWindow/
#      maxInputTokens = реальный слот llm-host (ctx_per_slot). Без этого Cline сжимает
#      историю раньше заполнения слота и агент теряет контекст поиска.
# Всё это делает ОДНА команда `hds cline-sync` (тот же код, что у кнопки в UI).
# После правки моделей/MCP Cline нужно перезапустить — команда об этом сообщает.
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

# Rust-бинарник hds: bin/hds (архив) -> ~/.local/bin/hds -> PATH -> dev-сборка.
find_hds() {
    for CAND in "$ROOT/bin/hds" "$HOME/.local/bin/hds" "$ROOT/target/release/hds" "$ROOT/target/debug/hds"; do
        [ -x "$CAND" ] && { printf '%s\n' "$CAND"; return 0; }
    done
    command -v hds 2>/dev/null && return 0
    return 1
}
# Настройки Cline пишет `hds cline-sync` (тот же код, что у кнопки в UI):
# models.json + оба файла MCP + правило + скилл. Python для этого не нужен.
HDS="$(find_hds)" || HDS=""
if [ -z "$HDS" ]; then
    echo "[--] hds не найден — соберите/установите релиз и повторите; настройки Cline не изменены"
    exit 1
fi
# Сервер должен быть поднят ДО подключения агента. restart переиспользует живой
# инстанс и перезапускает устаревший (старый код на порту) — как на Windows.
MCP_RAW="$(HDS_ROOT="$ROOT" "$HDS" mcp-http restart 2>&1)" || true
if printf '%s' "$MCP_RAW" | grep -q '"state"'; then
    echo "[ok] Общий MCP-сервер: поднят/переиспользован"
else
    echo "[!!] MCP-сервер: $MCP_RAW"
fi
HDS_ROOT="$ROOT" "$HDS" cline-sync \
    || echo "[!!] cline-sync сообщил о предупреждениях — см. строки выше"

# --- Контроль глазами самого Cline (если CLI в PATH) ---
# Cline валидирует файл настроек ЦЕЛИКОМ: одна неверная запись = теряются ВСЕ
# MCP-серверы, поэтому проверяем не только форму (её контролирует `hds cline-sync`),
# но и то, что клиент принимает файл.
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

# Правило и скилл установил `hds cline-sync` выше (один код с кнопкой в UI);
# предупреждения выводятся там же.
echo "Перезапустите Cline Desktop (или начните новую сессию), чтобы изменения вступили в силу."
