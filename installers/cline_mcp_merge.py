"""Регистрация MCP-сервера disk-search в настройках Cline (Desktop / CLI).

Кросс-платформенно (Windows/macOS): пути к настройкам передаёт вызывающий
инсталлятор. Формат — общий JSON с секцией mcpServers (STDIO-сервер = command+args).

Использование:
  python cline_mcp_merge.py --python <venv-python> --mcp-start <mcp_start.py> \\
      --targets <settings.json> [settings2.json ...]

Идемпотентно: другие серверы сохраняются, запись disk-search обновляется.
Запись пишется атомарно (tmp + os.replace); при нечитаемом JSON ничего не пишется.
"""
import argparse
import json
import os
import sys


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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", required=True, help="путь к python из venv проекта")
    ap.add_argument("--mcp-start", required=True, help="путь к mcp_start.py")
    ap.add_argument("--targets", nargs="+", required=True, help="JSON-файлы настроек MCP Cline")
    args = ap.parse_args()
    entry = {"command": args.python, "args": [args.mcp_start],
             "env": {}, "disabled": False, "autoApprove": [],
             # дублируем в новом вложенном формате Cline Desktop
             # (серверы из Marketplace пишутся так); плоский формат
             # command+args понимают все клиенты Cline
             "transport": {"type": "stdio", "command": args.python,
                           "args": [args.mcp_start], "env": {}}}
    for t in args.targets:
        merge(t, entry)
    for t in args.targets:  # контроль: запись на месте и файл валиден
        with open(t, encoding="utf-8-sig") as f:
            d = json.load(f)
        assert d["mcpServers"]["disk-search"]["command"] == args.python
    print("[ok] MCP disk-search зарегистрирован в настройках Cline")


if __name__ == "__main__":
    main()
