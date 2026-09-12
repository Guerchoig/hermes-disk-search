"""Атомарный перенос индексной БД (используется CLI db-move и веб-интерфейс)."""
import os
import re
import sqlite3
import subprocess
import time


def _hds_processes():
    """[(pid, cmdline)] процессов hds.cli watch/index (не включая UI-сервер)."""
    import base64

    script = ("(Get-CimInstance Win32_Process -Filter \"Name='python.exe' OR "
              "Name='pythonw.exe'\") | Where-Object { $_.CommandLine -match "
              "'-m hds\\.cli (watch|index)' } | ForEach-Object { Write-Output "
              "($_.ProcessId.ToString() + '|' + $_.CommandLine) }")
    enc = base64.b64encode(script.encode("utf-16-le")).decode()
    r = subprocess.run(["powershell", "-NoProfile", "-EncodedCommand", enc],
                       capture_output=True, text=True)
    if r.returncode != 0 and not (r.stdout or "").strip():
        print("[db-move] powershell перечисление процессов: %s" % (r.stderr or "")[:200],
              flush=True)
    out = []
    for line in (r.stdout or "").splitlines():
        if "|" in line.strip():
            pid, cmd = line.strip().split("|", 1)
            out.append((pid, cmd))
    return out


def move_db(new_path, force=False, project=None, venv_pythonw=None):
    """Атомарный перенос индексной БД.

    Возвращает {"ok": bool, "msg": str, "watch_was_running": bool}.
    Этапы: остановка watch/index -> консистентная копия (backup API) ->
    проверка счётчиков -> атомарное переключение db_path в config.yaml ->
    переименование старой БД (.moved-<дата>) -> перезапуск watcher.
    """
    from .config import config_path, db_abs_path, load

    cfg = load()
    old = db_abs_path(cfg)
    new = os.path.abspath(new_path)
    if new == old:
        return {"ok": False, "msg": "Новый путь совпадает с текущим: %s" % new}
    if os.path.exists(new) and not force:
        return {"ok": False, "msg": "Целевой файл уже существует: %s (проверьте путь "
                                    "или включите перезапись)" % new}

    procs = _hds_processes()
    watch_was = any("watch" in cmd for _pid, cmd in procs)
    if procs:
        print("[db-move] останавливаю процессы hds: %d шт." % len(procs), flush=True)
        for pid, _cmd in procs:
            try:
                subprocess.run(["taskkill", "/F", "/PID", pid], capture_output=True)
            except Exception:  # noqa: BLE001
                pass
        time.sleep(2)

    import sqlite3

    os.makedirs(os.path.dirname(new) or ".", exist_ok=True)
    print("[db-move] копирую %s -> %s ..." % (old, new), flush=True)
    src = sqlite3.connect(old)
    dst = sqlite3.connect(new)
    src.backup(dst)
    dst.close()
    src.close()

    def counts(path):
        c = sqlite3.connect(path)
        files = c.execute("SELECT COUNT(*) FROM files").fetchone()[0]
        chunks = c.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
        c.close()
        return files, chunks

    c_old, c_new = counts(old), counts(new)
    if c_old != c_new:
        os.remove(new)
        return {"ok": False, "msg": "Проверка не сошлась (%s vs %s) — откат" % (c_old, c_new)}
    print("[db-move] ok: файлов %d, чанков %d" % c_new, flush=True)

    # атомарная замена config.yaml (одинарные кавычки YAML: бэкслэши не экранируются)
    cfg_path = config_path()
    with open(cfg_path, "r", encoding="utf-8-sig") as f:
        text = f.read()
    safe = new.replace("'", "''")
    if re.search(r"(?m)^db_path:", text):
        text = re.sub(r"(?m)^db_path:.*$", lambda m: "db_path: '%s'" % safe, text)
    else:
        text += "\ndb_path: '%s'\n" % safe
    tmp = cfg_path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, cfg_path)

    stamp = time.strftime("%Y%m%d-%H%M%S")
    for suffix in ("", "-wal", "-shm"):
        if os.path.exists(old + suffix):
            try:
                os.replace(old + suffix, old + suffix + ".moved-" + stamp)
            except OSError:
                pass

    if watch_was and venv_pythonw and os.path.exists(venv_pythonw):
        subprocess.Popen([venv_pythonw, "-m", "hds.cli", "watch"], cwd=project,
                         creationflags=subprocess.CREATE_NO_WINDOW)
        print("[db-move] watcher перезапущен", flush=True)

    return {"ok": True, "msg": "БД перенесена в %s (файлов %d, чанков %d); "
                               "старая копия: index.db.moved-%s" % (new, c_new[0], c_new[1], stamp),
            "watch_was_running": watch_was}