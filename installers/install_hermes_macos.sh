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

# --- 1. MCP-сервер disk-search + tools.tool_search (правка config.yaml с сохранением комментариев) ---
# По аналогии с Windows install_hermes.ps1: БЕЗ Python-ядра. URL читается из
# config.yaml (секция mcp_http.*) обычным разбором текста; сервер поднимает
# Rust-бинарник hds (Python в поставке остаётся только воркером-экстрактором).
# Один общий http-инстанс MCP (:8787): Hermes подключается по URL и не запускает
# свой процесс на каждую сессию (живой инстанс переиспользуется).

# Rust-бинарник hds: bin/hds (архив) -> ~/.local/bin/hds -> PATH -> dev-сборка.
find_hds() {
    for CAND in "$ROOT/bin/hds" "$HOME/.local/bin/hds" "$ROOT/target/release/hds" "$ROOT/target/debug/hds"; do
        [ -x "$CAND" ] && { printf '%s\n' "$CAND"; return 0; }
    done
    command -v hds 2>/dev/null && return 0
    return 1
}
# stdio-вариант (когда URL неизвестен): bin/hds_mcp -> ~/.local/bin/hds_mcp -> PATH.
find_hds_mcp() {
    for CAND in "$ROOT/bin/hds_mcp" "$HOME/.local/bin/hds_mcp" "$ROOT/target/release/hds_mcp" "$ROOT/target/debug/hds_mcp"; do
        [ -x "$CAND" ] && { printf '%s\n' "$CAND"; return 0; }
    done
    command -v hds_mcp 2>/dev/null && return 0
    return 1
}

# Значение ключа внутри секции верхнего уровня (без Python): cfg_key <file> <section> <key>.
cfg_key() {
    awk -v sec="$2" -v key="$3" '
        $0 ~ "^"sec":" { insec=1; next }
        insec && $0 ~ /^[^[:space:]]/ { insec=0 }
        insec {
            line=$0; sub(/^[[:space:]]+/, "", line)
            if (index(line, key ":") == 1) { print substr(line, length(key)+2); exit }
        }' "$1" \
    | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*#.*$//' \
          -e "s/^[\"']//" -e "s/[\"']$//" -e 's/[[:space:]]*$//'
}

# URL общего MCP из config.yaml (mcp_http.host/port/path); иначе — stdio.
MCP_URL=""
if [ -f "$ROOT/config.yaml" ]; then
    m_host="$(cfg_key "$ROOT/config.yaml" mcp_http host)"; [ -n "$m_host" ] || m_host="127.0.0.1"
    m_port="$(cfg_key "$ROOT/config.yaml" mcp_http port)"; [ -n "$m_port" ] || m_port="8787"
    m_path="$(cfg_key "$ROOT/config.yaml" mcp_http path)"; [ -n "$m_path" ] || m_path="/mcp"
    MCP_URL="http://${m_host}:${m_port}${m_path}"
fi
if [ -n "$MCP_URL" ]; then
    mcp_block="  disk-search:
    url: $MCP_URL
    timeout: 300"
else
    echo "[--] URL MCP неизвестен — регистрирую stdio-сервер"
    mcp_exe="$(find_hds_mcp)" || mcp_exe="hds_mcp"
    mcp_block="  disk-search:
    command: $mcp_exe
    timeout: 300"
fi
# Заменить блок disk-search, иначе вставить в mcp_servers:, иначе добавить секцию.
if grep -qE '^  disk-search:' "$CFG"; then
    awk -v block="$mcp_block" 'BEGIN{done=0;skip=0}
        { if (!done && $0 ~ /^  disk-search:/) { print block; skip=1; done=1; next }
          if (skip && $0 ~ /^    /) next
          skip=0; print }' "$CFG" > "$CFG.tmp" && mv "$CFG.tmp" "$CFG"
    echo "[ok] MCP disk-search: блок обновлён в config.yaml"
elif grep -qE '^mcp_servers:[[:space:]]*$' "$CFG"; then
    awk -v block="$mcp_block" '{ print } !done && /^mcp_servers:[[:space:]]*$/ { print block; done=1 }' "$CFG" > "$CFG.tmp" && mv "$CFG.tmp" "$CFG"
    echo "[ok] MCP disk-search: добавлен в существующую секцию mcp_servers"
else
    printf '\n\nmcp_servers:\n%s\n' "$mcp_block" >> "$CFG"
    echo "[ok] MCP disk-search: секция mcp_servers добавлена в конец config.yaml"
fi
grep -qE '^  disk-search:[[:space:]]*$' "$CFG" || { echo "[--] блок disk-search не найден в config.yaml после правки"; exit 1; }
echo "[ok] config.yaml: disk-search зарегистрирован"

# --- 1b. tools.tool_search.enabled: "off" — все инструменты всегда в промпте ---
# Локальные модели не проходят discovery-протокол tool_search и не находят MCP-инструменты.
ts_inner='  tool_search:
    enabled: "off"'
if grep -qE '^  tool_search:' "$CFG"; then
    awk -v block="$ts_inner" 'BEGIN{done=0;skip=0}
        { if (!done && $0 ~ /^  tool_search:/) { print block; skip=1; done=1; next }
          if (skip && $0 ~ /^    /) next
          skip=0; print }' "$CFG" > "$CFG.tmp" && mv "$CFG.tmp" "$CFG"
    echo "[ok] tools.tool_search: блок обновлён (enabled: off)"
elif grep -qE '^tools:[[:space:]]*$' "$CFG"; then
    awk -v block="$ts_inner" '{ print } !done && /^tools:[[:space:]]*$/ { print block; done=1 }' "$CFG" > "$CFG.tmp" && mv "$CFG.tmp" "$CFG"
    echo "[ok] tools.tool_search: добавлен в существующую секцию tools"
else
    printf '\n# Все инструменты всегда в промпте (локальная модель не находит MCP-инструменты через discovery-протокол)\ntools:\n%s\n' "$ts_inner" >> "$CFG"
    echo "[ok] секция tools.tool_search добавлена в конец config.yaml"
fi

# Сервер должен быть поднят ДО подключения агента. restart переиспользует живой
# инстанс и перезапускает устаревший (старый код на порту) — как на Windows.
if [ -n "$MCP_URL" ]; then
    HDS="$(find_hds)" || HDS=""
    if [ -n "$HDS" ]; then
        mcp_raw="$(HDS_ROOT="$ROOT" "$HDS" mcp-http restart 2>&1)" || true
        if printf '%s' "$mcp_raw" | grep -q '"state"'; then
            echo "[ok] общий MCP-сервер ($MCP_URL): поднят/переиспользован"
        else
            echo "[--] MCP-сервер: $mcp_raw"
        fi
    else
        echo "[--] hds не найден — запустите MCP вручную: hds mcp-http start"
    fi
fi

# --- 1c. Обход системного прокси для loopback (нужно HTTP-MCP) ---
# httpx2 читает системный прокси и игнорирует ProxyOverride; без NO_PROXY запросы
# к 127.0.0.1:8787 уходят в прокси и MCP отвечает 503. На macOS прокси берём из
# scutil; HTTP(S)_PROXY зеркалит его, чтобы интернет продолжал работать.
env_path="$HERMES_DIR/.env"
proxy_url=""
if command -v scutil >/dev/null 2>&1; then
    p_out="$(scutil --proxy 2>/dev/null || true)"
    p_host="$(printf '%s\n' "$p_out" | awk '/HTTPProxy[[:space:]]*:/ {print $3; exit}')"
    p_port="$(printf '%s\n' "$p_out" | awk '/HTTPPort[[:space:]]*:/ {print $3; exit}')"
    [ -n "$p_host" ] && [ -n "$p_port" ] && proxy_url="http://$p_host:$p_port"
fi
{
    echo '# >>> disk-search: system proxy bypass for loopback >>>'
    echo '# httpx2 (the Hermes HTTP engine) reads the system proxy and IGNORES ProxyOverride.'
    echo '# Without these lines requests to 127.0.0.1:8787 go through the proxy and MCP fails.'
    echo '# Managed by the hermes-disk-search project (install_hermes_macos.sh) - edit there.'
    echo 'NO_PROXY=localhost,127.0.0.1,::1'
    echo 'no_proxy=localhost,127.0.0.1,::1'
    [ -n "$proxy_url" ] && { echo "HTTP_PROXY=$proxy_url"; echo "HTTPS_PROXY=$proxy_url"; }
    echo '# <<< disk-search <<<'
} > "$env_path.dsblock"
# Идемпотентно: убрать прежний блок disk-search, затем добавить актуальный.
if [ -f "$env_path" ]; then
    awk '/^# >>> disk-search: system proxy bypass for loopback >>>$/{skip=1; next} /^# <<< disk-search <<<$/{skip=0; next} !skip{ if (NF>0) last=NR; a[NR]=$0 } END{ for (i=1;i<=last;i++) print a[i] }' \
        "$env_path" > "$env_path.base"
fi
if [ -s "$env_path.base" ]; then
    { cat "$env_path.base"; printf '\n\n'; cat "$env_path.dsblock"; } > "$env_path"
else
    cp "$env_path.dsblock" "$env_path"
fi
rm -f "$env_path.base" "$env_path.dsblock"
if [ -n "$proxy_url" ]; then
    echo "[ok] Hermes .env: NO_PROXY для loopback (127.0.0.1/localhost); системный прокси $proxy_url зеркалится в HTTP(S)_PROXY"
else
    echo "[ok] Hermes .env: NO_PROXY для loopback (127.0.0.1/localhost)"
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