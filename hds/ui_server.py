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
    r = subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         "(Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR "
         "Name='pythonw.exe'\") | Where-Object { $_.CommandLine -match '%s' } "
         "| ForEach-Object { Write-Output $_.ProcessId }" % pattern],
        capture_output=True, text=True,
    )
    return [l.strip() for l in (r.stdout or "").splitlines() if l.strip().isdigit()]


def _startup_dir():
    import sys

    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/LaunchAgents")
    return os.path.join(os.environ.get("APPDATA", os.path.expanduser("~")),
                        "Microsoft", "Windows", "Start Menu", "Programs", "Startup")


def _watch_running():
    lock = os.path.join(PROJECT, "watch.lock")
    if not os.path.exists(lock):
        return False, None
    try:
        pid = int(open(lock).read().strip())
        return _pid_alive(pid), pid
    except (ValueError, OSError):
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
    exe = os.path.join(PROJECT, ".venv", "Scripts", "pythonw.exe")
    if not os.path.exists(exe):
        return {"ok": False, "msg": "pythonw.exe не найден"}
    subprocess.Popen([exe, "-m", "hds.cli", "watch"], cwd=PROJECT,
                     creationflags=subprocess.CREATE_NO_WINDOW)
    return {"ok": True}


def _watch_stop():
    pids = _hds_pids("hds\\.cli watch")
    for pid in pids:
        try:
            subprocess.run(["taskkill", "/F", "/PID", pid], capture_output=True)
        except Exception:  # noqa: BLE001
            pass
    return {"ok": True, "stopped": pids}


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
    state["pending_est"] = max(0, state.get("seen", 0) - state.get("processed", 0))
    return state


def _db_stats():
    from .config import db_abs_path

    path = db_abs_path(load())
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
    """Деревья папок от корней индексации со статусом каждой папки:
    done (зелёный) — все файлы папки в индексе; partial (жёлтый) — частично;
    none — не начата. Статус родителя = худший из себя и потомков."""
    import sqlite3

    from .config import db_abs_path
    from .indexer import _kind_of, path_excluded

    cfg = load()
    excl = {str(e).lower() for e in dig(cfg, "index.exclude_dirs", [])}
    disk = {}   # dir -> [count, max_mtime]
    for root in [os.path.abspath(r) for r in dig(cfg, "index.roots", [])]:
        if not os.path.isdir(root):
            continue
        for dirpath, dirnames, filenames in os.walk(root, onerror=lambda e: None):
            dirnames[:] = [d for d in dirnames if d.lower() not in excl]
            for fn in filenames:
                if not _kind_of(os.path.splitext(fn)[1].lower()):
                    continue
                d = dirpath
                e = disk.setdefault(d, [0, 0.0])
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
    NAMES = ("none", "partial", "done")
    status_of = {}
    for d in set(disk) | set(db):
        dn, mtime = disk.get(d, [0, 0.0])
        tot, idx, iat = db.get(d, [0, 0, 0.0])
        if dn == 0 and tot == 0:
            continue
        if tot == 0:
            st = "none"      # индексация не начиналась (нет записей в БД)
        elif dn > 0 and mtime > iat + 2:
            st = "partial"   # есть файлы, изменённые после индексации
        elif idx >= dn:
            st = "done"
        else:
            st = "partial"
        status_of[d] = {"status": st, "files": dn, "indexed": idx}

    # worst-status (агрегация потомков) выполняется в aggregate(); узлы получают
    # собственный статус каталога, родители сворачиваются по детям
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
                    sinfo = sub.get(cur_path) or {"files": 0, "indexed": 0}
                    nxt = {"name": p, "path": cur_path,
                           "status": (sub.get(cur_path) or {"status": "none"})["status"],
                           "files": sinfo["files"],
                           "indexed": sinfo["indexed"],
                           "children": []}
                    by_path[cur_path] = nxt
                    cur["children"].append(nxt)
                cur = nxt
        # сводный узел для длинных списков детей
        def trim(n):
            if len(n["children"]) > child_limit:
                rest = n["children"][child_limit:]
                lvl = min(LEVEL[c["status"]] for c in rest)
                n["children"] = n["children"][:child_limit] + [{
                    "name": "… ещё %d папок" % len(rest), "path": "",
                    "status": NAMES[lvl], "files": 0, "indexed": 0, "children": []}]
            for c in n["children"]:
                trim(c)
        trim(node)
        trees.append(node)

    # статус родителя: все потомки done → done; все none → none;
    # смесь (есть прогресс и есть не начатое) → partial
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
    out = {"trees": trees,
            "dirs": len(status_of),
            "disk_files": sum(v[0] for v in disk.values())}
    if os.environ.get("HDS_DEBUG_TREES"):
        out["status_of"] = status_of
    return out





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
    cfg = load()
    conn = dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))
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
                  venv_pythonw=os.path.join(PROJECT, ".venv", "Scripts", "pythonw.exe"))
    if res.get("ok") and was_ui_index:
        time.sleep(1)
        print("[db-move] перезапускаю индексацию с прежними настройками", flush=True)
        _start_index(full=_ui_index_args["full"], roots=_ui_index_args["roots"])
    return res


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
        if self.path in ("/", "/index.html"):
            html = open(os.path.join(PROJECT, "assets", "ui.html"), "rb").read()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(html)))
            self.end_headers()
            self.wfile.write(html)
        elif self.path == "/api/status":
            cfg = load()
            with open(os.path.join(PROJECT, "config.yaml"),
                      "r", encoding="utf-8-sig") as f:
                yaml_text = f.read()
            w_running, w_pid = _watch_running()
            self._json({
                "index": _index_state(),
                "watch": {"running": w_running, "pid": w_pid,
                          "autostart": _watch_autostart_on()},
                "db": _db_stats(),
                "config_yaml": yaml_text,
                "roots": [os.path.abspath(r) for r in dig(cfg, "index.roots", [])],
            })
        elif self.path == "/api/tree":
            self._json(_build_trees())
        else:
            self._json({"error": "not found"}, 404)

    def do_POST(self):
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
        else:
            self._json({"error": "not found"}, 404)

    def log_message(self, *a):  # тишина в консоли
        pass


def run(port=8765, open_browser=True):
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    url = "http://127.0.0.1:%d" % port
    print("[ui] интерфейс: %s (Ctrl+C — остановка)" % url, flush=True)
    if open_browser:
        threading.Timer(0.5, lambda: webbrowser.open(url)).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass