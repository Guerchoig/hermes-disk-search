"""Регистрация MCP-сервера disk-search в настройках Cline (Desktop / CLI).

УСТАРЕЛО (совместимость): актуальная синхронизация — команда `hds cline-sync`
(её же выполняет кнопка «Синхронизировать настройки Cline» в веб-интерфейсе) и
`hds_core::cline::sync` в коде. Она делает больше: кроме записи `mcpServers`
выравнивает окна контекста моделей в `models.json` по слотам `llm-host` и
устанавливает правило и скилл disk-search. Один код — один источник истины
(README, «Интеграция с Hermes / Cline»). Файл оставлен для истории/ручных сценариев.

Кросс-платформенно (Windows/macOS): пути к настройкам передаёт вызывающий
инсталлятор. Формат — общий JSON с секцией mcpServers; запись пишется в ПЛОСКОЙ
форме, как в документации Cline
(https://docs.cline.bot/mcp/configuring-mcp-servers):
  remote — {"type": "streamableHttp", "url": ...};
  stdio  — {"command": ..., "args": [...], "env": {...}}.
Устаревшая обёртка "transport" не используется: в ней схема Cline допускает
только литералы stdio|sse|streamableHttp, поэтому "transport": {"type": "http"}
приводит к «Invalid MCP settings ... mcpServers.disk-search: Invalid input» —
файл настроек отбрасывается ЦЕЛИКОМ, и клиент не получает ни одного MCP-сервера.

Два режима транспорта:
  http  — клиент подключается к ОБЩЕМУ инстансу по URL (:8787) и не запускает
          собственных процессов (рекомендуется; менеджер hds.mcp_http);
  stdio — клиент сам запускает процесс MCP-сервера: по одному на каждого
          клиента, а при нескольких сеансах/перезапусках hub'а — с сиротами.

Использование:
  python cline_mcp_merge.py --mode http --url http://127.0.0.1:8787/mcp \
      --targets <settings.json> [settings2.json ...]
  python cline_mcp_merge.py --python <venv-python> --mcp-start <mcp_start.py> \
      --targets <settings.json> [settings2.json ...]          # режим stdio

Идемпотентно: другие серверы сохраняются, запись disk-search обновляется.
Запись пишется атомарно (tmp + os.replace); при нечитаемом JSON ничего не пишется.
"""
import argparse
import json
import os
import sys

# autoApprove со всеми инструментами обязателен: с пустым списком Cline
# запрашивает подтверждение на каждый вызов, и модель избегает сервера.
TOOLS = ["search_local_files", "ask_my_files", "index_status",
         "start_indexing", "stop_indexing", "reindex_path"]
# timeout: ask_my_files делает RAG-ответ локальной LLM (поиск + генерация,
# при холодной модели — ещё и загрузка весов), 60 с по умолчанию не хватает;
# Cline поддерживает per-server "timeout" в секундах.
TIMEOUT = 300


def merge(path, entry):
    data = {}
    if os.path.exists(path):
        try:
            with open(path, encoding="utf-8-sig") as f:
                data = json.load(f)
        except (ValueError, OSError) as e:
            sys.exit("[--] %s не читается (%s) — ничего не записано, поправьте вручную" % (path, e))
        if not isinstance(data, dict):
            sys.exit("[--] %s: неожиданный формат (не JSON-объект) — ничего не записано" % path)
    servers = data.setdefault("mcpServers", {})
    if not isinstance(servers, dict):
        sys.exit("[--] %s: секция mcpServers неожиданного формата — ничего не записано" % path)
    existed = "disk-search" in servers
    servers["disk-search"] = entry
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")
    os.replace(tmp, path)
    print("[ok] %s: запись disk-search %s" % (path, "обновлена" if existed else "добавлена"))


def build_entry(args):
    """Запись disk-search для секции mcpServers (плоская форма Cline).

    Плоские поля — то, что валидирует схема Cline (Desktop/CLI/IDE): remote —
    "type": "streamableHttp" + "url"; stdio — "command" + "args" + "env".
    """
    if args.mode == "http":
        if not args.url:
            sys.exit("[--] --mode http требует --url http://<host>:<port>/mcp")
        return {"type": "streamableHttp", "url": args.url, "disabled": False,
                "autoApprove": TOOLS, "timeout": TIMEOUT}
    if not (args.python and args.mcp_start):
        sys.exit("[--] --mode stdio требует --python и --mcp-start")
    return {"command": args.python, "args": [args.mcp_start], "env": {},
            "disabled": False, "autoApprove": TOOLS, "timeout": TIMEOUT}


def check(path, args):
    """Контроль: запись на месте, файл валиден, транспорт нужного вида.

    Проверяется ровно то, что требует схема Cline: плоские url/type (remote) или
    command/args (stdio) и отсутствие устаревшей обёртки "transport".
    """
    with open(path, encoding="utf-8-sig") as f:
        data = json.load(f)
    entry = data["mcpServers"]["disk-search"]
    assert "transport" not in entry, "%s: устаревшая обёртка transport" % path
    if args.mode == "http":
        assert entry.get("type") in ("streamableHttp", "sse"), path
        assert entry.get("url") == args.url, path
    else:
        assert entry.get("command") == args.python, path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["stdio", "http"], default="stdio",
                    help="http — общий инстанс по URL (рекомендуется); "
                         "stdio — процесс на каждого клиента")
    ap.add_argument("--url", help="URL общего MCP-сервера (режим http), "
                                  "например http://127.0.0.1:8787/mcp")
    ap.add_argument("--python", help="путь к python из venv проекта (режим stdio)")
    ap.add_argument("--mcp-start", help="путь к mcp_start.py (режим stdio)")
    ap.add_argument("--targets", nargs="+", required=True,
                    help="JSON-файлы настроек MCP Cline")
    args = ap.parse_args()

    entry = build_entry(args)
    for t in args.targets:
        merge(t, entry)
    for t in args.targets:  # контроль: запись на месте и файл валиден
        check(t, args)
    print("[ok] MCP disk-search зарегистрирован в настройках Cline (mode=%s)"
          % args.mode)


if __name__ == "__main__":
    main()
