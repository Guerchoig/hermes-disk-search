"""Локальный веб-интерфейс hermes-disk-search (http://127.0.0.1:8765).

Запуск: python -m hds.cli ui  (браузер открывается автоматически)
Только localhost; без сторонних зависимостей (stdlib http.server).
"""
import json
import os
import subprocess
import sys
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from . import db as dbmod, indexer
from .config import dig, db_abs_path, load
from .dbops import move_db
from .embedder import make_embedder

PROJECT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_PAUSE = os.path.join(PROJECT, "index.pause")
_STOP = os.path.join(PROJECT, "index.stop")
_cfg_lock = threading.Lock()
_ui_index_args = {"roots": None, "full": False}  # параметры последнего UI-старта индексации


# --- Устойчивость к отсутствующим/битым настройкам --------------------------
def _safe_cfg():
    """Возвращает (cfg, error): гарантирует наличие config.yaml (создаёт
    дефолтный при отсутствии); при нечитаемом YAML отдаёт пустой словарь
    и текст ошибки. UI обязан открываться при любых настройках."""
    from .config import ensure_config, load

    try:
        ensure_config()
        return load(), ""
    except Exception as e:  # noqa: BLE001
        return {}, "config.yaml не читается: %s" % e


def _venv_python():
    """Интерпретатор venv для фоновых процессов (watcher), кросс-платформенно."""
    if os.name == "nt":
        return os.path.join(PROJECT, ".venv", "Scripts", "pythonw.exe")
    return os.path.join(PROJECT, ".venv", "bin", "python")


# --- Модель эмбеддингов: статус / скачивание / загрузка ---------------------
_MODEL_NAME = "text-embedding-bge-m3"
_MODEL_URL = "https://huggingface.co/lm-kit/bge-m3-gguf/resolve/main/bge-m3-Q8_0.gguf"
_EMB_DL = {"running": False, "progress": 0.0, "msg": "", "error": ""}


def _gguf_path():
    home = os.path.expanduser("~")
    return os.path.join(home, ".lmstudio", "models", "lm-kit",
                        "bge-m3-gguf", "bge-m3-Q8_0.gguf")


def _lm_models():
    """(server_ok, ids) — опрос LM Studio на localhost:1234."""
    import requests as rq

    try:
        r = rq.get("http://localhost:1234/v1/models", timeout=2)
        return True, [m.get("id") for m in r.json().get("data", [])]
    except Exception:  # noqa: BLE001
        return False, []


def _model_status():
    server_ok, models = _lm_models()
    return {
        "gguf_path": _gguf_path(),
        "gguf_ready": os.path.exists(_gguf_path()),
        "downloading": _EMB_DL["running"],
        "progress": _EMB_DL["progress"],
        "msg": _EMB_DL["error"] or _EMB_DL["msg"],
        "server_ok": server_ok,
        "model_loaded": bool(server_ok and _MODEL_NAME in models),
    }


def _model_download():
    """Фоновое скачивание GGUF bge-m3 (~1,2 ГБ) в папку моделей LM Studio."""
    if _EMB_DL["running"]:
        return {"ok": False, "msg": "Скачивание уже идёт"}
    if os.path.exists(_gguf_path()):
        return {"ok": True, "msg": "Файл модели уже на месте"}
    _EMB_DL.update({"running": True, "progress": 0.0,
                    "msg": "начинаю скачивание", "error": ""})

    def job():
        try:
            os.makedirs(os.path.dirname(_gguf_path()), exist_ok=True)
            tmp = _gguf_path() + ".part"
            import requests as rq

            with rq.get(_MODEL_URL, stream=True, timeout=60) as r:
                r.raise_for_status()
                total = int(r.headers.get("Content-Length", 0))
                done = 0
                with open(tmp, "wb") as f:
                    for chunk in r.iter_content(chunk_size=1024 * 1024):
                        f.write(chunk)
                        done += len(chunk)
                        if total:
                            _EMB_DL["progress"] = round(done / total * 100, 1)
                        _EMB_DL["msg"] = "скачано %d МБ из %s" % (
                            done // 1048576,
                            ("%d МБ" % (total // 1048576)) if total else "?")
            os.replace(tmp, _gguf_path())
            _EMB_DL.update({"running": False, "progress": 100.0,
                            "msg": "модель скачана — загрузите её кнопкой «Загрузить» "
                                   "или в LM Studio (Developer → Load)"})
        except Exception as e:  # noqa: BLE001
            _EMB_DL.update({"running": False, "msg": "",
                            "error": "ошибка скачивания: %s" % e})

    threading.Thread(target=job, daemon=True).start()
    return {"ok": True, "msg": "Скачивание начато (~1,2 ГБ, один раз)"}


def _model_load():
    """Загрузка модели в запущенный LM Studio через lms CLI (best effort)."""
    import shutil

    exe = shutil.which("lms")
    if not exe:
        return {"ok": False, "msg": "CLI 'lms' не найден — откройте LM Studio и "
                                    "загрузите модель вручную: Developer → "
                                    "Select a model to load → text-embedding-bge-m3"}
    try:
        r = subprocess.run([exe, "load", _MODEL_NAME, "-y"],
                           capture_output=True, timeout=600)
        if r.returncode == 0:
            return {"ok": True, "msg": "Модель загружена в LM Studio"}
        out = (r.stderr or r.stdout or b"").decode("utf-8", errors="replace")[:300]
        return {"ok": False, "msg": "lms load завершился с ошибкой: %s" % out}
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "msg": "Не удалось выполнить lms load: %s" % e}


def _pid_alive(pid):
    if os.name == "nt":
        import ctypes

        k = ctypes.windll.kernel32
        h = k.OpenProcess(0x100000, False, int(pid))
        if h:
            k.CloseHandle(h)
            return True
        return False
    try:
        os.kill(int(pid), 0)
        return True
    except OSError:
        return False


def _hds_pids(pattern):
    """PID процессов python/pythonw, чья командная строка матчит pattern.
    psutil вместо PowerShell Get-CimInstance: вызов через subprocess ломался
    кавычками WQL ('Invalid query') и тихо возвращал пустой список —
    из-за этого кнопка «Остановить watcher» не убивала процесс."""
    import re as _re

    try:
        import psutil
    except ImportError:
        return []
    rx = _re.compile(pattern)
    out = []
    for p in psutil.process_iter(["pid", "name", "cmdline"]):
        try:
            info = p.info
            if (info.get("name") or "").lower() not in (
                    "python.exe", "pythonw.exe", "python", "python3"):
                continue
            if rx.search(" ".join(info.get("cmdline") or [])):
                out.append(info["pid"])
        except Exception:  # noqa: BLE001
            continue
    return out


def _startup_dir():
    import sys

    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/LaunchAgents")
    return os.path.join(os.environ.get("APPDATA", os.path.expanduser("~")),
                        "Microsoft", "Windows", "Start Menu", "Programs", "Startup")


def _watch_running():
    """Watcher считается запущенным, только если жив реальный процесс с
    'hds.cli watch' в командной строке. watch.lock не доверяем: он мог
    остаться от умершего watcher'а, а PID — быть переиспользован ОС."""
    pids = _hds_pids(r"hds\.cli watch")
    if pids:
        return True, pids[0]
    return False, None


def _watch_autostart_on():
    return os.path.exists(os.path.join(_startup_dir(), "HermesDiskSearchWatch.lnk"))
def _watch_autostart_set(enabled):
    startup = _startup_dir()
    if sys.platform == "darwin":
        plist = os.path.join(startup, "local.hds.watch.plist")
        if enabled:
            py = os.path.join(PROJECT, ".venv", "bin", "python")
            content = (
                '<?xml version="1.0" encoding="UTF-8"?>\n'
                "<plist version=\"1.0\"><dict>"
                "<key>Label</key><string>local.hds.watch</string>"
                "<key>ProgramArguments</key><array>"
                "<string>%s</string><string>-m</string><string>hds.cli</string>"
                "<string>watch</string></array>"
                "<key>RunAtLoad</key><true/><key>KeepAlive</key><true/></dict></plist>\n"
                % py)
            with open(plist, "w") as f:
                f.write(content)
        else:
            if os.path.exists(plist):
                os.remove(plist)
        return True
    if enabled:
        pythonw = os.path.join(PROJECT, ".venv", "Scripts", "pythonw.exe")
        cmd = ("$ws = New-Object -ComObject WScript.Shell; "
               "$l = $ws.CreateShortcut($p + '\\HermesDiskSearchWatch.lnk'); "
               "$l.TargetPath = '%s'; $l.Arguments = '-m hds.cli watch'; "
               "$l.WorkingDirectory = '%s'; "
               "$l.IconLocation = '%s\\assets\\icon.ico,0'; $l.Save()"
               % (pythonw.replace("'", "''"), PROJECT.replace("'", "''"), PROJECT))
    else:
        cmd = ("$p = '%s'; $f = $p + '\\HermesDiskSearchWatch.lnk'; "
               "if (Test-Path $f) { Remove-Item $f }" % _startup_dir().replace("'", "''"))
    r = subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         "$p = '%s'; %s" % (_startup_dir().replace("'", "''"), cmd)],
        capture_output=True)
    return r.returncode == 0


def _watch_start():
    running, _pid = _watch_running()
    if running:
        return {"ok": False, "msg": "watcher уже запущен"}
    exe = _venv_python()
    if not os.path.exists(exe):
        return {"ok": False, "msg": "Интерпретатор venv не найден: %s" % exe}
    kwargs = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    subprocess.Popen([exe, "-m", "hds.cli", "watch"], cwd=PROJECT, **kwargs)
    return {"ok": True}


def _watch_stop():
    """Остановка watcher'а через psutil (TerminateProcess). taskkill из
    pythonw срабатывал ненадёжно; здесь же — контроль фактического результата."""
    import psutil

    pids = _hds_pids(r"hds\.cli watch")
    stopped = []
    for pid in pids:
        try:
            p = psutil.Process(pid)
            for ch in p.children(recursive=True):  # shim -> реальный интерпретатор
                try:
                    ch.kill()
                except Exception:  # noqa: BLE001
                    pass
            p.kill()
            stopped.append(pid)
        except psutil.NoSuchProcess:
            stopped.append(pid)  # уже мёртв — считаем остановленным
        except Exception:  # noqa: BLE001
            pass
    deadline = time.time() + 5
    while time.time() < deadline and _hds_pids(r"hds\.cli watch"):
        time.sleep(0.3)
    return {"ok": True, "stopped": stopped}


def _index_state():
    rep = getattr(indexer, "_ACTIVE_REPORTER", None)
    if rep is not None:
        with rep._lock:
            state = {
                "running": True, "paused": rep.paused,
                "seen": rep.seen_count, "processed": rep.processed_count,
                "errors": rep.errors, "chunks": rep.chunks,
                "elapsed": round(time.time() - rep.t0, 1),
                "rate_min": round(rep.seen_count / max(0.001, time.time() - rep.t0) * 60),
                "current": {"path": rep.current[0], "phase": rep.current[1],
                            "progress": rep.progress} if rep.current else None,
                "events": list(rep.events),
                "by_kind": dict(rep.by_kind),
            }
    else:
        last = getattr(indexer, "_LAST_REPORTER", None)
        state = {"running": False, "paused": False}
        if last is not None:
            with last._lock:
                state.update({
                    "seen": last.seen_count, "processed": last.processed_count,
                    "errors": last.errors, "chunks": last.chunks,
                    "elapsed": getattr(last, "final_elapsed", 0.0),
                    "events": list(last.events), "by_kind": dict(last.by_kind),
                })
    # heartbeat: индексация, запущенная в другом процессе (CLI/watcher), тоже видна
    hb_path = os.path.join(PROJECT, "index.heartbeat.json")
    try:
        if os.path.exists(hb_path):
            with open(hb_path, "r", encoding="utf-8") as f:
                hb = json.load(f)
            if hb.get("ts") and time.time() - hb["ts"] < 30:
                state.update({"running": True, "paused": bool(hb.get("paused")),
                              "seen": hb.get("seen", state.get("seen", 0)),
                              "processed": hb.get("processed", state.get("processed", 0)),
                              "errors": hb.get("errors", state.get("errors", 0)),
                              "chunks": hb.get("chunks", 0),
                              "elapsed": hb.get("elapsed", 0),
                              "rate_min": hb.get("rate_min", 0),
                              "current": {"path": hb.get("path", ""),
                                          "phase": hb.get("phase", ""),
                                          "progress": hb.get("progress")}})
    except Exception:  # noqa: BLE001
        pass
    state["stop_requested"] = os.path.exists(_STOP)
    hb_path = os.path.join(PROJECT, "index.heartbeat.json")
    try:
        if os.path.exists(hb_path):
            with open(hb_path, "r", encoding="utf-8") as f:
                hb = json.load(f)
            if hb.get("ts") and time.time() - hb["ts"] < 30:
                state.update({"running": True, "paused": bool(hb.get("paused")),
                              "seen": hb.get("seen", state.get("seen", 0)),
                              "processed": hb.get("processed", state.get("processed", 0)),
                              "errors": hb.get("errors", state.get("errors", 0)),
                              "chunks": hb.get("chunks", 0),
                              "elapsed": hb.get("elapsed", 0),
                              "rate_min": hb.get("rate_min", 0),
                              "total": hb.get("total"),
                              "rate_window": hb.get("rate_window"),
                              "eta_sec": hb.get("eta_sec"),
                              "remaining": hb.get("remaining"),
                              "current": {"path": hb.get("path", ""),
                                          "phase": hb.get("phase", ""),
                                          "progress": hb.get("progress")}})
    except Exception:  # noqa: BLE001
        pass
    if state.get("running") and state.get("eta_sec") is not None:
        state["pending_est"] = state.get("remaining", 0)
    else:
        state["pending_est"] = max(0, state.get("seen", 0) - state.get("processed", 0))
    return state


_CONFIG_FIELDS = {
    "index.transcribe": ("bool", r"(?m)^(\s*transcribe:)\s+\w+"),
    "index.max_media_mb": ("int", r"(?m)^(\s*max_media_mb:)\s+\d+"),
    "index.max_chunks": ("int", r"(?m)^(\s*max_chunks:)\s+\d+"),
}


def _db_stats():
    from .config import db_abs_path

    cfg, _err = _safe_cfg()
    path = db_abs_path(cfg)
    info = {"path": path, "size_mb": 0.0}
    if os.path.exists(path):
        info["size_mb"] = round(os.path.getsize(path) / 1048576.0, 1)
        try:
            c = dbmod.connect(path, int(dig(load(), "embedding.dim", 1024)))
            info["files"] = c.execute("SELECT COUNT(*) FROM files").fetchone()[0]
            info["chunks"] = c.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
            info["errors"] = c.execute(
                "SELECT COUNT(*) FROM files WHERE status='error'").fetchone()[0]
            c.close()
        except Exception as e:  # noqa: BLE001
            info["error"] = repr(e)
    return info


def _build_trees():
    """Деревья папок от корней индексации: done/partial/none,
    статус родителя — свёртка по потомкам."""
    from .config import db_abs_path
    from .indexer import _kind_of

    cfg, _err = _safe_cfg()
    excl = {str(e).lower() for e in dig(cfg, "index.exclude_dirs", [])}
    disk = {}
    for root in [os.path.abspath(r) for r in dig(cfg, "index.roots", [])]:
        if not os.path.isdir(root):
            continue
        for dirpath, dirnames, filenames in os.walk(root, onerror=lambda e: None):
            dirnames[:] = [d for d in dirnames if d.lower() not in excl]
            for fn in filenames:
                if not _kind_of(os.path.splitext(fn)[1].lower()):
                    continue
                e = disk.setdefault(dirpath, [0, 0.0])
                e[0] += 1
                try:
                    m = os.stat(os.path.join(dirpath, fn)).st_mtime
                    if m > e[1]:
                        e[1] = m
                except OSError:
                    pass

    db = {}
    path_db = db_abs_path(cfg)
    if os.path.exists(path_db):
        c = dbmod.connect(path_db, int(dig(cfg, "embedding.dim", 1024)))
        for path, status, iat in c.execute("SELECT path, status, indexed_at FROM files"):
            d = os.path.dirname(path)
            e = db.setdefault(d, [0, 0, 0.0])
            e[0] += 1
            if status == "indexed":
                e[1] += 1
            if iat and iat > e[2]:
                e[2] = iat
        c.close()

    LEVEL = {"none": 0, "partial": 1, "done": 2}
    status_of = {}
    for d in set(disk) | set(db):
        dn, mtime = disk.get(d, [0, 0.0])
        tot, idx, iat = db.get(d, [0, 0, 0.0])
        if dn == 0 and tot == 0:
            continue
        if tot == 0:
            st = "none"
        elif dn > 0 and mtime > iat + 2:
            st = "partial"
        elif idx >= dn:
            st = "done"
        else:
            st = "partial"
        status_of[d] = {"status": st, "files": dn, "indexed": idx}

    max_depth = 4
    child_limit = 40
    trees = []
    for root in [os.path.abspath(r) for r in dig(cfg, "index.roots", [])]:
        r = os.path.normpath(root)
        prefix = (r.rstrip(os.sep).lower() + os.sep)
        sub = {d: v for d, v in status_of.items()
               if d.lower().startswith(prefix)}
        if not sub:
            continue
        node = {"name": r, "path": r,
                "status": (status_of.get(r) or {"status": "none"})["status"],
                "files": sum(v["files"] for v in sub.values()),
                "indexed": sum(v["indexed"] for v in sub.values()),
                "children": []}
        by_path = {r: node}
        for d in sorted(sub, key=lambda x: (x.count(os.sep), x.lower())):
            if d == r:
                continue
            parts = [p for p in os.path.relpath(d, r).split(os.sep) if p][:max_depth]
            cur_path, cur = r, node
            for p in parts:
                cur_path = os.path.join(cur_path, p)
                nxt = by_path.get(cur_path)
                if nxt is None:
                    sinfo = sub.get(cur_path) or {"status": "none",
                                                  "files": 0, "indexed": 0}
                    nxt = {"name": p, "path": cur_path,
                           "status": sinfo["status"],
                           "files": sinfo["files"],
                           "indexed": sinfo["indexed"],
                           "children": []}
                    by_path[cur_path] = nxt
                    cur["children"].append(nxt)
                cur = nxt

        def trim(n):
            if len(n["children"]) > child_limit:
                rest = n["children"][child_limit:]
                lvl = min(LEVEL[c["status"]] for c in rest)
                n["children"] = n["children"][:child_limit] + [{
                    "name": "… ещё %d папок" % len(rest), "path": "",
                    "status": ("none", "partial", "done")[lvl],
                    "files": 0, "indexed": 0, "children": []}]
            for c in n["children"]:
                trim(c)
        trim(node)
        trees.append(node)

    def aggregate(n):
        st = n["status"]
        for c in n["children"]:
            aggregate(c)
        ch = n["children"]
        if ch:
            lvls = {c["status"] for c in ch}
            if lvls == {"done"}:
                st = "done"
            elif lvls == {"none"}:
                st = "none"
            else:
                st = "partial"
        n["status"] = st
        return st
    for t in trees:
        aggregate(t)
    return {"trees": trees,
            "dirs": len(status_of),
            "disk_files": sum(v[0] for v in disk.values())}


def _set_simple_config(key, value):
    """Точечное изменение параметра config.yaml (сохраняет комментарии).
    Белый список ключей; файл заменяется атомарно."""
    import re

    spec = _CONFIG_FIELDS.get(key)
    if spec is None:
        return {"ok": False, "msg": "Недопустимый параметр: %s" % key}
    typ, rx = spec
    if typ == "bool":
        if not isinstance(value, bool):
            return {"ok": False, "msg": "Ожидается true/false"}
        repl = "true" if value else "false"
    else:
        try:
            value = int(value)
        except (TypeError, ValueError):
            return {"ok": False, "msg": "Ожидается целое число"}
        if value < 0:
            return {"ok": False, "msg": "Значение должно быть >= 0"}
        repl = str(value)
    from .config import config_path

    cfg_path = config_path()
    with open(cfg_path, "r", encoding="utf-8-sig") as f:
        text = f.read()
    new_text, n = re.subn(rx, lambda m: m.group(1) + " " + repl, text)
    if n == 0:
        return {"ok": False, "msg": "Параметр не найден в config.yaml"}
    tmp = cfg_path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(new_text)
    os.replace(tmp, cfg_path)
    return {"ok": True,
            "msg": "Сохранено. Применяется к новым запускам — перезапустите "
                   "watcher/индексацию кнопками, чтобы параметр подействовал."}


def _save_config(yaml_text):
    import yaml

    try:
        data = yaml.safe_load(yaml_text)
        if not isinstance(data, dict):
            raise ValueError("YAML должен быть словарём")
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "msg": "Ошибка YAML: %s" % e}
    from .config import config_path

    cfg_path = config_path()
    tmp = cfg_path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(yaml_text if yaml_text.endswith("\n") else yaml_text + "\n")
    os.replace(tmp, cfg_path)
    return {"ok": True,
            "msg": "Сохранено. Изменения применятся к новым запускам — "
                   "watcher/индексацию перезапустите кнопками."}





def _looks_abs(p):
    """Абсолютен ли путь на любой ОС: os.path.isabs плюс Windows-пути с буквой
    диска ('D:\\...' / 'D:/...'), которые на POSIX os.path.abspath ошибочно
    считает относительными и приклеивает к ним cwd."""
    if os.path.isabs(p):
        return True
    return len(p) >= 3 and p[1] == ":" and p[2] in "\\/"


def _set_exclude_paths(paths):
    """Сохранение index.exclude_paths в config.yaml с сохранением комментариев.
    Пути нормализуются в абсолютные; YAML-значения в одинарных кавычках
    (Windows-пути в двойных кавычках ломают YAML)."""
    import re

    if not isinstance(paths, list):
        return {"ok": False, "msg": "Ожидается список путей"}
    norm, seen = [], set()
    for p in paths:
        if not isinstance(p, str):
            continue
        p = str(p).strip().strip('"').strip("'")
        if not p:
            continue
        ap = p if _looks_abs(p) else os.path.abspath(p)
        k = os.path.normcase(ap).lower()
        if k not in seen:
            seen.add(k)
            norm.append(ap)
    block = "  exclude_paths:\n" + "".join(
        "    - '%s'\n" % p.replace("'", "''") for p in norm) if norm else "  exclude_paths: []\n"
    from .config import config_path

    cfg_path = config_path()
    with open(cfg_path, "r", encoding="utf-8-sig") as f:
        text = f.read()
    # блок-форма: заголовок + только строки-элементы списка ('- ...'),
    # чтобы не съесть соседние ключи секции index с тем же отступом
    rx_block = re.compile(r"(?m)^  exclude_paths:\n(?:[ \t]*-[ \t][^\n]*\n?)*")
    rx_inline = re.compile(r"(?m)^  exclude_paths:[ \t]*\[.*\][ \t]*\r?\n?")
    if rx_block.search(text):
        new_text = rx_block.sub(lambda m: block, text, count=1)
    elif rx_inline.search(text):
        new_text = rx_inline.sub(lambda m: block, text, count=1)
    else:
        m = re.search(r"(?m)^  exclude_dirs:.*\n", text)
        if not m:
            return {"ok": False, "msg": "Не найдена секция index.exclude_dirs в config.yaml"}
        new_text = text[:m.end()] + block + text[m.end():]
    try:
        parsed = _yaml_safe_load(new_text)
        eps = (parsed.get("index") or {}).get("exclude_paths")
        if not isinstance(eps, list):
            raise ValueError("index.exclude_paths не список")
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "msg": "Итоговый config.yaml некорректен: %s" % e}
    tmp = cfg_path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(new_text)
    os.replace(tmp, cfg_path)
    roots_hit = [r for r in (dig(load(), "index.roots", []) or [])
                 if indexer._prefix_excluded(
                     r, [indexer._norm_path(p) for p in norm])]
    warn = " ВНИМАНИЕ: исключение покрывает корень индексации (%s)!" % ", ".join(roots_hit) if roots_hit else ""
    return {"ok": True, "paths": norm,
            "msg": "Сохранено путей: %d. Применяется к новым запускам — перезапустите "
                   "watcher/индексацию кнопками.%s" % (len(norm), warn)}


def _set_roots(paths):
    """Сохранение index.roots в config.yaml (комментарии сохраняются).
    Пути нормализуются в абсолютные; YAML-значения в одинарных кавычках
    (Windows-пути в двойных кавычках ломают YAML)."""
    import re

    if not isinstance(paths, list):
        return {"ok": False, "msg": "Ожидается список путей"}
    norm, seen = [], set()
    for p in paths:
        if not isinstance(p, str):
            continue
        p = str(p).strip().strip('"').strip("'")
        if not p:
            continue
        p = os.path.expanduser(p)
        ap = p if _looks_abs(p) else os.path.abspath(p)
        k = os.path.normcase(ap).lower()
        if k not in seen:
            seen.add(k)
            norm.append(ap)
    block = ("  roots:\n" + "".join("    - '%s'\n" % p.replace("'", "''") for p in norm)
             if norm else "  roots: []\n")
    from .config import config_path

    cfg_path = config_path()
    with open(cfg_path, "r", encoding="utf-8-sig") as f:
        text = f.read()
    # блок-форма: заголовок + только строки-элементы списка ('- ...'),
    # чтобы не съесть соседние ключи секции index с тем же отступом
    rx_block = re.compile(r"(?m)^  roots:\n(?:[ \t]*-[ \t][^\n]*\n?)*")
    rx_inline = re.compile(r"(?m)^  roots:[ \t]*\[.*\][ \t]*\r?\n?")
    if rx_block.search(text):
        new_text = rx_block.sub(lambda m: block, text, count=1)
    elif rx_inline.search(text):
        new_text = rx_inline.sub(lambda m: block, text, count=1)
    else:
        m = re.search(r"(?m)^index:\r?\n", text)
        if not m:
            return {"ok": False, "msg": "Не найдена секция index в config.yaml"}
        new_text = text[:m.end()] + block + text[m.end():]
    try:
        parsed = _yaml_safe_load(new_text)
        rr = (parsed.get("index") or {}).get("roots")
        if not isinstance(rr, list):
            raise ValueError("index.roots не список")
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "msg": "Итоговый config.yaml некорректен: %s" % e}
    tmp = cfg_path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(new_text)
    os.replace(tmp, cfg_path)
    warn = (" ВНИМАНИЕ: список корней пуст — индексация не найдёт файлов."
            if not norm else "")
    return {"ok": True, "roots": norm,
            "msg": "Корни сохранены (%d). Применяются к новым запускам — "
                   "запустите/перезапустите watcher и индексацию кнопками выше.%s"
                   % (len(norm), warn)}


def _yaml_safe_load(text):
    import yaml
    return yaml.safe_load(text)


def _start_index(full=False, roots=None):
    if getattr(indexer, "_ACTIVE_REPORTER", None) is not None:
        return {"ok": False, "msg": "Индексация уже идёт"}
    if os.path.exists(_STOP):
        os.remove(_STOP)
    if os.path.exists(_PAUSE):
        os.remove(_PAUSE)
    if isinstance(roots, str):
        roots = [r.strip() for r in roots.split(";") if r.strip()]
    roots_list = roots or None
    _ui_index_args.update({"roots": roots_list, "full": full})
    cfg, cfg_err = _safe_cfg()
    if cfg_err:
        return {"ok": False, "msg": "Настройки не читаются: " + cfg_err}
    try:
        conn = dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))
    except Exception as e:  # noqa: BLE001
        return {"ok": False, "msg": "Не удалось открыть БД: %s — проверьте db_path "
                                    "в настройках (файл config.yaml ниже)" % e}
    emb = make_embedder(cfg)

    def job():
        try:
            indexer.run_index(conn, emb, cfg, roots=roots_list, full=full,
                              progress_sec=3, prune=True)
        finally:
            conn.close()

    threading.Thread(target=job, daemon=True).start()
    return {"ok": True}


def _db_move(new_path, force=False):
    """Перенос БД из UI: остановка индексации -> атомарный перенос ->
    восстановление состояния (индексация перезапускается, watcher — внутри move_db)."""
    was_ui_index = getattr(indexer, "_ACTIVE_REPORTER", None) is not None
    if was_ui_index:
        print("[db-move] останавливаю индексацию перед переносом...", flush=True)
        open(_STOP, "w").close()
        deadline = time.time() + 120
        while getattr(indexer, "_ACTIVE_REPORTER", None) is not None and time.time() < deadline:
            time.sleep(1)
    res = move_db(new_path, force=force, project=PROJECT,
                  venv_pythonw=_venv_python())
    if res.get("ok") and was_ui_index:
        time.sleep(1)
        print("[db-move] перезапускаю индексацию с прежними настройками", flush=True)
        _start_index(full=_ui_index_args["full"], roots=_ui_index_args["roots"])
    return res


class Handler(BaseHTTPRequestHandler):
    def _json(self, obj, code=200):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _body(self):
        n = int(self.headers.get("Content-Length", 0))
        return json.loads(self.rfile.read(n) or b"{}")

    def do_GET(self):
        try:
            if self.path in ("/", "/index.html"):
                html = open(os.path.join(PROJECT, "assets", "ui.html"), "rb").read()
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(html)))
                self.end_headers()
                self.wfile.write(html)
            elif self.path == "/api/status":
                cfg, cfg_err = _safe_cfg()
                yaml_text = ""
                try:
                    from .config import config_path
                    with open(config_path(), "r", encoding="utf-8-sig") as f:
                        yaml_text = f.read()
                except Exception:  # noqa: BLE001
                    pass
                w_running, w_pid = _watch_running()
                self._json({
                    "config_error": cfg_err,
                    "index": _index_state(),
                    "watch": {"running": w_running, "pid": w_pid,
                              "autostart": _watch_autostart_on()},
                    "db": _db_stats(),
                    "config_yaml": yaml_text,
                    "roots": [os.path.abspath(r) for r in (dig(cfg, "index.roots", []) or [])
                              if isinstance(r, str)],
                    "exclude_paths": [os.path.abspath(p)
                                      for p in (dig(cfg, "index.exclude_paths", []) or [])
                                      if isinstance(p, str)],
                    "options": {"transcribe": bool(dig(cfg, "index.transcribe", True)),
                                "max_media_mb": dig(cfg, "index.max_media_mb", 1500),
                                "max_chunks": dig(cfg, "index.max_chunks", 2000)},
                })
            elif self.path == "/api/tree":
                self._json(_build_trees())
            elif self.path == "/api/model/status":
                self._json(_model_status())
            elif self.path == "/api/diagnostics":
                from .diag import run_checks
                cfg, cfg_err = _safe_cfg()
                if cfg_err:
                    checks = [{"id": "config", "status": "fail",
                               "title": "Настройки не читаются", "msg": cfg_err,
                               "fix": "Исправьте config.yaml в группе «Настройки» и нажмите «Сохранить настройки»."}]
                else:
                    checks = run_checks(cfg)
                self._json({"checks": checks,
                            "ok": all(c["status"] != "fail" for c in checks)})
            else:
                self._json({"error": "not found"}, 404)
        except Exception as e:  # noqa: BLE001
            # UI не должен «падать» ни при каких настройках — отдаём ошибку JSON-ом
            try:
                self._json({"error": str(e)}, 500)
            except Exception:  # noqa: BLE001
                pass

    def do_POST(self):
        try:
            body = self._body()
            if self.path == "/api/index/start":
                self._json(_start_index(full=bool(body.get("full")),
                                        roots=body.get("roots") or None))
            elif self.path == "/api/index/stop":
                open(_STOP, "w").close()
                self._json({"ok": True})
            elif self.path == "/api/index/pause":
                open(_PAUSE, "w").close()
                self._json({"ok": True})
            elif self.path == "/api/index/resume":
                if os.path.exists(_PAUSE):
                    os.remove(_PAUSE)
                self._json({"ok": True})
            elif self.path == "/api/watch/start":
                self._json(_watch_start())
            elif self.path == "/api/watch/stop":
                self._json(_watch_stop())
            elif self.path == "/api/watch/autostart":
                self._json({"ok": _watch_autostart_set(bool(body.get("enabled")))})
            elif self.path == "/api/config/save":
                with _cfg_lock:
                    self._json(_save_config(body.get("yaml", "")))
            elif self.path == "/api/db/move":
                path = body.get("path", "").strip()
                if not path:
                    self._json({"ok": False, "msg": "Укажите путь"})
                else:
                    self._json(_db_move(path, force=bool(body.get("force"))))
            elif self.path == "/api/index/reindex":
                path = (body.get("path") or "").strip().strip('"')
                if not path or not os.path.exists(path):
                    self._json({"ok": False, "msg": "Путь не существует: %s" % path})
                else:
                    self._json(_start_index(full=True, roots=path))
            elif self.path == "/api/config/set":
                self._json(_set_simple_config(body.get("key", ""), body.get("value")))
            elif self.path == "/api/config/excludes":
                self._json(_set_exclude_paths(body.get("paths") or []))
            elif self.path == "/api/roots/save":
                with _cfg_lock:
                    self._json(_set_roots(body.get("paths") or []))
            elif self.path == "/api/model/download":
                self._json(_model_download())
            elif self.path == "/api/model/load":
                self._json(_model_load())
            else:
                self._json({"error": "not found"}, 404)
        except Exception as e:  # noqa: BLE001
            try:
                self._json({"error": str(e)}, 500)
            except Exception:  # noqa: BLE001
                pass

    def log_message(self, *a):  # тишина в консоли
        pass


def run(port=8765, open_browser=True):
    # защита от двойного запуска: на Windows SO_REUSEADDR позволяет двум
    # серверам молча делить один порт — сначала пробуем «постучаться»
    import socket as _socket
    probe = _socket.socket()
    probe.settimeout(1.0)
    try:
        probe.connect(("127.0.0.1", port))
        print("[ui] порт %d уже занят — интерфейс, вероятно, уже запущен. Выход."
              % port, flush=True)
        return
    except OSError:
        pass
    finally:
        probe.close()
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    url = "http://127.0.0.1:%d" % port
    print("[ui] интерфейс: %s (Ctrl+C — остановка)" % url, flush=True)
    if open_browser:
        threading.Timer(0.5, lambda: webbrowser.open(url)).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
