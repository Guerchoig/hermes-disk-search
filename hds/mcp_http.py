"""Менеджер MCP-сервера disk-search в режиме streamable-http.

Зачем отдельный режим. При stdio-транспорте КАЖДЫЙ клиент (Cline Desktop,
Cline CLI, Hermes Desktop, ...) по протоколу MCP запускает СВОЙ процесс
MCP-сервера (~250 МБ: эмбеддер + БД). Несколько агентов и их сеансов — несколько
процессов, а долгоживущие hub-демоны клиентов (Cline code-sidecar) оставляют
ещё и сирот: stdin такого процесса не закрывается, и старый сервер не выходит.
В http-режиме сервер живёт РОВНО ОДИН (отвязанный от агентов), а клиенты только
подключаются по URL — сколько бы их ни было.

Состояния probe: STATE_MCP («наш» живой сервер), STATE_FOREIGN (порт занят чужим
сервисом), STATE_DOWN (никто не слушает).

Конфигурация — секция mcp_http в config.yaml (дефолты в коде):

    mcp_http:
      host: "127.0.0.1"
      port: 8787               # ОДИН http-инстанс MCP на машину
      path: "/mcp"             # streamable-http endpoint
      autostart: true          # поднимать при старте UI/MCP/cli
      start_timeout: 30        # сек ожидания /health при старте

CLI:
    python -m hds.cli mcp-http check|start|stop|stop-stdio|status|restart|restart-if-stale|run
    python -m hds.mcp_http     check|start|stop|stop-stdio|status|restart|restart-if-stale|run

    restart-if-stale — штатная точка ОБНОВЛЕНИЯ проекта: поднимает сервер, если
    его нет, и перезапускает, если на порту работает старый код (другая версия
    или исходники новее запущенного процесса). Простой `start` живой инстанс
    переиспользует — обновлённый код остался бы неприменённым.

Только стандартная библиотека (urllib/subprocess/threading).
"""
import json
import os
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

from . import __version__, build_stamp
from .config import APP_NAME, PROJECT_ROOT

STATE_MCP = "mcp"          # живой НАШ инстанс на порту
STATE_FOREIGN = "foreign"  # порт занят чужим сервисом
STATE_DOWN = "down"        # никто не слушает

# Дефолты (перекрываются секцией mcp_http в config.yaml)
_DEFAULTS = {
    "host": "127.0.0.1",
    "port": 8787,
    "path": "/mcp",
    "autostart": True,
    "start_timeout": 30,
}

_PROBE_TIMEOUT = 3.0
_POLL_INTERVAL = 0.5
# Маркер cmdline stdio-инстансов — для разовой чистки сирот (stop-stdio)
_STDIO_MARKER = "mcp_start.py"


# ==================== Конфигурация ====================

def _sec(cfg):
    sec = (cfg or {}).get("mcp_http")
    return sec if isinstance(sec, dict) else {}


def host(cfg):
    return str(_sec(cfg).get("host", _DEFAULTS["host"]))


def port(cfg):
    return int(_sec(cfg).get("port", _DEFAULTS["port"]))


def path(cfg):
    p = str(_sec(cfg).get("path", _DEFAULTS["path"]) or _DEFAULTS["path"]).strip()
    return p if p.startswith("/") else "/" + p


def autostart_on(cfg):
    return bool(_sec(cfg).get("autostart", _DEFAULTS["autostart"]))


def start_timeout(cfg):
    return float(_sec(cfg).get("start_timeout", _DEFAULTS["start_timeout"]))


def base_url(cfg=None):
    cfg = cfg or {}
    return "http://%s:%d" % (host(cfg), port(cfg))


def url(cfg=None):
    """URL streamable-http endpoint — его прописывают клиентам (Cline/Hermes)."""
    return base_url(cfg) + path(cfg)


def _pid_file():
    return os.path.join(PROJECT_ROOT, "data", "mcp_http.pid")


def _log_file():
    return os.path.join(PROJECT_ROOT, "data", "logs", "mcp_http.log")


# ==================== HTTP-проба ====================

def probe(cfg=None, timeout=_PROBE_TIMEOUT):
    """Кто слушает порт: наш MCP, чужой сервис или никто.

    Наш сервер опознаётся по GET /health — кастомному маршруту MCP-сервера
    (hds.mcp_server: {"app": "disk-search"}). Любой другой HTTP-ответ
    (не-JSON, чужой JSON, ошибка) = STATE_FOREIGN: переиспользовать нельзя.
    """
    url_ = base_url(cfg) + "/health"
    req = urllib.request.Request(url_, headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            if not 200 <= resp.status < 300:
                return {"state": STATE_FOREIGN, "info": {}, "pid": None}
            body = resp.read()
    except urllib.error.HTTPError:
        # HTTP-сервер на порту есть, но маршрута /health у него нет
        return {"state": STATE_FOREIGN, "info": {}, "pid": None}
    except Exception:  # noqa: BLE001 — нет соединения/таймаут
        return {"state": STATE_DOWN, "info": {}, "pid": _live_pid()}
    try:
        data = json.loads(body.decode("utf-8", errors="replace"))
    except ValueError:
        return {"state": STATE_FOREIGN, "info": {}, "pid": None}
    if isinstance(data, dict) and data.get("app") == APP_NAME:
        return {"state": STATE_MCP, "info": data, "pid": _live_pid()}
    return {"state": STATE_FOREIGN,
            "info": data if isinstance(data, dict) else {}, "pid": None}


# ==================== Процессы ====================

def _no_window():
    """Флаги subprocess: консольные утилиты (tasklist/taskkill/powershell/pgrep)
    не должны вспыхивать окнами при вызове из UI/автозапуска (Windows)."""
    return {"creationflags": subprocess.CREATE_NO_WINDOW} \
        if sys.platform == "win32" else {}


def _popen_kwargs():
    """Отвязанный запуск: MCP-сервер переживает перезапуск UI и агентов.

    CREATE_NO_WINDOW в дополнение к DETACHED_PROCESS — старт из UI
    (pythonw/автозапуск ОС) не рождает мигающее консольное окно (Windows).
    """
    if sys.platform == "win32":
        return {"creationflags": (subprocess.DETACHED_PROCESS
                                  | subprocess.CREATE_NEW_PROCESS_GROUP
                                  | subprocess.CREATE_NO_WINDOW)}
    return {"start_new_session": True}


def _python():
    """Интерпретатор для запуска сервера.

    Windows: pythonw.exe (GUI-подсистема) — у процесса НЕТ консоли вообще.
    Иначе python.exe (консольная подсистема) получает консоль даже с
    CREATE_NO_WINDOW, и при «терминале по умолчанию» Windows Terminal она
    всплывает видимым окном (наблюдалось: окно «...\\.venv\\Scripts\\python.exe»
    висело всё время работы MCP-сервера). Под pythonw print() — no-op
    (sys.stdout = None), поэтому вывод сервера идёт только в лог-файл.
    """
    if sys.platform == "win32":
        w = os.path.join(PROJECT_ROOT, ".venv", "Scripts", "pythonw.exe")
        if os.path.isfile(w):
            return w
        return sys.executable
    cand = os.path.join(PROJECT_ROOT, ".venv", "bin", "python")
    return cand if os.path.isfile(cand) else sys.executable


def _read_pid():
    try:
        with open(_pid_file(), "r", encoding="utf-8") as f:
            return int(f.read().strip())
    except (ValueError, OSError):
        return None


def _live_pid():
    """PID из PID-файла, если процесс с ним ещё жив."""
    pid = _read_pid()
    if pid is None:
        return None
    if sys.platform == "win32":
        try:
            res = subprocess.run(["tasklist", "/FI", "PID eq %d" % pid],
                                 capture_output=True, text=True, check=False,
                                 **_no_window())
            return pid if str(pid) in (res.stdout or "") else None
        except OSError:
            return None
    try:
        os.kill(pid, 0)
        return pid
    except OSError:
        return None


def _remove_pid_file():
    """Убрать PID-файл (его отсутствие = «инстанс не под нашим управлением»)."""
    try:
        os.remove(_pid_file())
    except OSError:
        pass


def _kill_pid(pid):
    """Убить процесс (Windows — дерево целиком: лаунчер + интерпретатор)."""
    try:
        if sys.platform == "win32":
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"],
                           capture_output=True, check=False, **_no_window())
        else:
            os.kill(pid, 15)  # SIGTERM
    except (ProcessLookupError, OSError):
        pass


def stdio_pids(exclude=()):
    """PID'ы stdio-инстансов MCP-сервера (cmdline содержит mcp_start.py).

    Нужны для разовой чистки: при stdio каждая сессия клиента рождала свой
    процесс, и за долгоживущими hub-демонами (Cline code-sidecar) накопились
    сироты. В http-режиме такие процессы не нужны вовсе.
    """
    if sys.platform == "win32":
        script = ("Get-CimInstance Win32_Process | Where-Object "
                  "{ $_.CommandLine -like '*%s*' } | ForEach-Object "
                  "{ $_.ProcessId }" % _STDIO_MARKER)
        cmd = ["powershell", "-NoProfile", "-NonInteractive", "-Command", script]
        timeout = 40
    else:
        cmd = ["pgrep", "-f", _STDIO_MARKER]
        timeout = 10
    try:
        res = subprocess.run(cmd, capture_output=True, text=True,
                             timeout=timeout, **_no_window())
    except (OSError, subprocess.SubprocessError):
        return []
    me = os.getpid()
    out = []
    for token in (res.stdout or "").split():
        if not token.strip().isdigit():
            continue
        pid = int(token)
        if pid == me or pid in exclude or pid in out:
            continue
        out.append(pid)
    return out


def stop_stdio():
    """Остановить все stdio-инстансы (разовая чистка накопленных сирот).

    Возвращает список убитых PID'ов. Свой процесс и родителя не трогает.
    """
    pids = stdio_pids(exclude=(os.getppid(),))
    for pid in pids:
        _kill_pid(pid)
    return pids


def port_pids(port_num):
    """PID'ы процессов, слушающих TCP-порт (Windows — powershell, иначе lsof).

    Нужны stop()/restart'у для инстанса БЕЗ PID-файла: `mcp-http run` (его
    ставит автозапуск ОС) и ручной запуск PID-файла не пишут, и раньше
    `restart` молча ничего не делал — порт оставался у старого процесса.
    """
    if sys.platform == "win32":
        script = ("Get-NetTCPConnection -LocalPort %d -State Listen "
                  "-ErrorAction SilentlyContinue | "
                  "ForEach-Object { $_.OwningProcess }" % int(port_num))
        cmd = ["powershell", "-NoProfile", "-NonInteractive", "-Command", script]
        timeout = 40
    else:
        cmd = ["lsof", "-ti", "tcp:%d" % int(port_num), "-sTCP:LISTEN"]
        timeout = 10
    try:
        res = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout,
                             **_no_window())
    except (OSError, subprocess.SubprocessError):
        return []
    me = os.getpid()
    out = []
    for token in (res.stdout or "").split():
        if not token.strip().isdigit():
            continue
        pid = int(token)
        if pid == me or pid in out:
            continue
        out.append(pid)
    return out


# ==================== Запуск / остановка ====================

def build_command(cfg, host_=None, port_=None, path_=None):
    """Командная строка запуска сервера (foreground-режим `run`)."""
    return [_python(), "-m", "hds.mcp_http", "run",
            "--host", str(host_ or host(cfg)),
            "--port", str(int(port_ or port(cfg))),
            "--path", str(path_ or path(cfg))]


def _log_tail(lines=15):
    try:
        with open(_log_file(), "r", encoding="utf-8", errors="replace") as f:
            return "".join(f.readlines()[-lines:]).strip()
    except OSError:
        return ""


def _redirect_to_log():
    """Под pythonw stdout/stderr = None (консоли нет) — пишем в лог.

    Нужно для инстанса, поднятого автозагрузкой ОС (`pythonw -m hds.cli mcp-http
    run`): иначе вывод сервера (в т.ч. ошибки uvicorn) никуда не попадает и
    диагностика невозможна. Если консоль есть (ручной запуск) — не трогаем.
    """
    if sys.stdout is not None and sys.stderr is not None:
        return
    try:
        os.makedirs(os.path.dirname(_log_file()), exist_ok=True)
        f = open(_log_file(), "a", encoding="utf-8", buffering=1)
    except OSError:
        return
    if sys.stdout is None:
        sys.stdout = f
    if sys.stderr is None:
        sys.stderr = f


def _wait_ready(cfg, proc):
    """Поллинг /health до готовности сервера (импорт mcp+uvicorn — секунды)."""
    timeout = start_timeout(cfg)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(
                "MCP-сервер завершился с кодом %s при старте — см. лог %s\n%s"
                % (proc.returncode, _log_file(), _log_tail()))
        det = probe(cfg)
        if det["state"] == STATE_MCP:
            return det
        time.sleep(_POLL_INTERVAL)
    raise RuntimeError(
        "MCP-сервер не ответил на /health за %d c — см. лог %s\n%s"
        % (int(timeout), _log_file(), _log_tail()))


def start(cfg, wait=True):
    """Проверить порт и при необходимости поднять ОДИН http-инстанс.

    Живой наш инстанс переиспользуется (в т.ч. поднятый автозапуском ОС) —
    второго процесса не появляется. Посторонний сервис на порту — ошибка:
    молча менять порт нельзя, на этот адрес смотрят клиенты (Cline/Hermes).
    Возвращает {"started", "reused", "state", "pid", "url", "command"}.
    """
    det = probe(cfg)
    if det["state"] == STATE_MCP:
        return {"started": False, "reused": True, "state": det["state"],
                "pid": det["pid"], "url": url(cfg), "command": []}
    if det["state"] == STATE_FOREIGN:
        raise RuntimeError(
            "Порт %s занят посторонним сервисом (не MCP disk-search). "
            "Освободите порт или смените mcp_http.port в config.yaml"
            % base_url(cfg))
    cmd = build_command(cfg)
    log_path = _log_file()
    os.makedirs(os.path.dirname(log_path), exist_ok=True)
    env = dict(os.environ)
    env["PYTHONPATH"] = PROJECT_ROOT + (
        os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    with open(log_path, "ab") as log:
        try:
            proc = subprocess.Popen(cmd, cwd=PROJECT_ROOT,
                                    stdin=subprocess.DEVNULL,
                                    stdout=log, stderr=subprocess.STDOUT,
                                    env=env, **_popen_kwargs())
        except OSError as exc:
            raise RuntimeError("Не удалось запустить MCP-сервер (%s): %s"
                               % (cmd[0], exc)) from exc
    with open(_pid_file(), "w", encoding="utf-8") as f:
        f.write(str(proc.pid))
    if not wait:
        return {"started": True, "reused": False, "state": STATE_DOWN,
                "pid": proc.pid, "url": url(cfg), "command": cmd}
    det = _wait_ready(cfg, proc)
    return {"started": True, "reused": False, "state": det["state"],
            "pid": proc.pid, "url": url(cfg), "command": cmd}


def stop(cfg=None):
    """Остановить инстанс: по PID-файлу, иначе — по владельцу порта.

    True — процесс был и убит. Инстанс, запущенный вне менеджера
    (`mcp-http run` из автозапуска ОС или руками), PID-файла не имеет: его
    находим по порту — но ТОЛЬКО когда на порту отвечает НАШ /health (иначе
    можно убить чужой сервис).
    """
    cfg = cfg or {}
    pid = _read_pid()
    if pid is None:
        if probe(cfg)["state"] != STATE_MCP:
            return False
        pids = port_pids(port(cfg))
        if not pids:
            return False
        for p in pids:
            _kill_pid(p)
        _remove_pid_file()
        return True
    _kill_pid(pid)
    _remove_pid_file()
    return True


def wait_down(cfg, timeout=10.0):
    """Дождаться, пока порт перестанет отвечать нашим /health (для restart)."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if probe(cfg, timeout=1.0)["state"] != STATE_MCP:
            return True
        time.sleep(_POLL_INTERVAL)
    return False


def status(cfg=None, scan_stdio=False):
    """Сводный статус (для CLI и диагностики UI)."""
    cfg = cfg or {}
    det = probe(cfg)
    reason = _stale_reason(det) if det["state"] == STATE_MCP else ""
    out = {
        "url": url(cfg),
        "base_url": base_url(cfg),
        "state": det["state"],
        "running": det["state"] == STATE_MCP,
        "pid": det["pid"],
        "version": (det["info"] or {}).get("version", ""),
        "code_version": __version__,
        "stale": bool(reason),
        "stale_reason": reason,
        "autostart": autostart_on(cfg),
        "python": _python(),
        "log": _log_file(),
    }
    if scan_stdio:
        out["stdio_processes"] = stdio_pids(exclude=(os.getppid(), os.getpid()))
    return out


def _stale_reason(det):
    """Почему работающий инстанс считается старым ("" — код актуален).

    Признаки обновления проекта: в /health другая версия, отсутствует метка
    сборки (инстанс старше этого механизма) или метка сборки (mtime исходников
    на момент старта сервера) разошлась с меткой ТЕКУЩЕГО кода — так ловятся и
    правки без поднятия версии. Допуск в 1 с гасит округление mtime на разных ФС.
    """
    info = det.get("info") or {}
    running_version = str(info.get("version") or "")
    if running_version and running_version != __version__:
        return "на порту версия %s, в папке проекта %s" % (running_version, __version__)
    running_build = info.get("build")
    code_build = build_stamp()
    if running_build is None:
        # Инстанс не умеет отдавать метку сборки — значит его код старше того,
        # что лежит в папке проекта (метка появилась вместе с этим механизмом).
        return "инстанс не сообщает метку сборки — его код старше текущего"
    if code_build and abs(float(running_build) - code_build) > 1.0:
        return "исходники новее запущенного процесса (код обновлён)"
    return ""


def staleness(cfg=None):
    """Актуален ли код работающего инстанса (проверка после обновления)."""
    det = probe(cfg or {})
    running = det["state"] == STATE_MCP
    reason = _stale_reason(det) if running else ""
    return {
        "running": running,
        "pid": det["pid"],
        "running_version": str((det.get("info") or {}).get("version") or ""),
        "code_version": __version__,
        "stale": bool(reason),
        "reason": reason,
    }


def restart_if_stale(cfg):
    """Поднять/перезапустить инстанс, если на порту работает старый код.

    Штатная точка ОБНОВЛЕНИЯ проекта (её вызывают install_hermes.ps1 /
    install_cline.ps1): клиенты ходят по тому же URL, поэтому перезапуск для
    них незаметен — они переподключаются к новому процессу. Если сервер не
    поднят — просто запускаем; если код актуален — переиспользуем живой
    (второго процесса не появляется).

    Возвращает {"action": "started"|"restarted"|"reused", ...} или {"error": ...}.
    """
    try:
        st = staleness(cfg)
        if not st["running"]:
            info = start(cfg, wait=True)
            return {"action": "started", "reason": "инстанс не был поднят", **info}
        if not st["stale"]:
            return {"action": "reused", "reason": "код актуален",
                    "pid": st["pid"], "version": st["running_version"],
                    "url": url(cfg)}
        stop(cfg)
        if not wait_down(cfg):
            raise RuntimeError(
                "прежний инстанс не освободил порт за 10 с — остановите его "
                "вручную (PID/порт), лог: %s" % _log_file())
        info = start(cfg, wait=True)
        return {"action": "restarted", "reason": st["reason"], **info}
    except Exception as e:  # noqa: BLE001 — вызывающий печатает/логирует
        return {"error": str(e)}


def ensure(cfg):
    """Гарантировать живой http-инстанс (блокирующе). Ошибка — в результате."""
    try:
        return start(cfg, wait=True)
    except Exception as e:  # noqa: BLE001 — вызывающий логирует
        return {"error": str(e)}


def ensure_async(cfg):
    """ensure() в фоновом потоке — для старта UI/MCP/CLI (неблокирующе)."""

    def job():
        info = ensure(cfg)
        if "error" in info:
            print("[mcp_http] %s" % info["error"], file=sys.stderr, flush=True)

    threading.Thread(target=job, daemon=True).start()


# ==================== CLI ====================

def _with_overrides(cfg, host_, port_, path_):
    """Копия конфига с перекрытием mcp_http из аргументов CLI."""
    if not (host_ or port_ or path_):
        return cfg
    out = dict(cfg)
    sec = dict(_sec(cfg))
    if host_:
        sec["host"] = host_
    if port_:
        sec["port"] = int(port_)
    if path_:
        sec["path"] = path_
    out["mcp_http"] = sec
    return out


def main(argv=None):
    import argparse

    ap = argparse.ArgumentParser(
        prog="hds.mcp_http",
        description="Менеджер MCP-сервера disk-search (streamable-http): "
                    "ОДИН инстанс на машину для всех агентов")
    sub = ap.add_subparsers(dest="command", required=True)
    for name, help_ in (
            ("check", "живой НАШ инстанс на порту? (exit 0/1)"),
            ("start", "проверить порт и запустить инстанс"),
            ("stop", "остановить инстанс по PID-файлу"),
            ("stop-stdio", "убить stdio-инстансы (mcp_start.py) — чистка сирот"),
            ("status", "JSON-статус (состояние, PID, URL, версия, актуальность кода)"),
            ("restart", "stop + start"),
            ("restart-if-stale", "перезапустить, только если работает старый код "
                                 "(штатное обновление проекта)"),
            ("run", "запустить сервер в foreground (для автозапуска ОС)")):
        p = sub.add_parser(name, help=help_)
        p.add_argument("--host", help="переопределить mcp_http.host")
        p.add_argument("--port", type=int, help="переопределить mcp_http.port")
        p.add_argument("--path", help="переопределить mcp_http.path")
        p.add_argument("--scan-stdio", action="store_true",
                       help="в status: посчитать живые stdio-инстансы")
    args = ap.parse_args(argv)

    from .config import ensure_config, load

    ensure_config()
    cfg = _with_overrides(load(), args.host, args.port, args.path)

    if args.command == "run":
        from .mcp_server import run as mcp_run
        _redirect_to_log()  # pythonw/автозапуск: консоли нет — пишем в лог
        print("[mcp_http] сервер: %s (foreground)" % url(cfg), flush=True)
        mcp_run(transport="streamable-http", host=host(cfg), port=port(cfg),
                path=path(cfg))
        return 0
    if args.command == "check":
        det = probe(cfg)
        print("%s: state=%s pid=%s" % (url(cfg), det["state"], det["pid"]))
        return 0 if det["state"] == STATE_MCP else 1
    if args.command == "stop-stdio":
        pids = stop_stdio()
        print("stdio-инстансы остановлены: %s" % (", ".join(map(str, pids))
                                                 if pids else "нет"))
        return 0
    if args.command == "stop":
        print("остановлен" if stop(cfg) else "не запущен (PID-файла нет)")
        return 0
    if args.command == "status":
        print(json.dumps(status(cfg, scan_stdio=args.scan_stdio),
                         ensure_ascii=False, indent=2))
        return 0
    if args.command == "restart-if-stale":
        info = restart_if_stale(cfg)
        print(json.dumps(info, ensure_ascii=False, indent=2))
        return 1 if "error" in info else 0
    if args.command == "restart":
        stop(cfg)
        if not wait_down(cfg):
            print("[mcp_http] прежний инстанс не освободил порт за 10 с",
                  file=sys.stderr, flush=True)
    info = ensure(cfg)
    print(json.dumps(info, ensure_ascii=False, indent=2))
    return 1 if "error" in info else 0


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())
