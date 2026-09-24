"""Наблюдатель файловой системы: события ОС (ReadDirectoryChangesW / FSEvents /
inotify через watchdog) -> мгновенная индексация изменений."""
import os
import queue
import sys
import threading
import time

from .config import dig, db_abs_path, PROJECT_ROOT
from . import db as dbmod, indexer
from .embedder import make_embedder
from .indexer import path_excluded

_state = {"processed": 0, "errors": 0, "moved": 0, "last_event": None}


def wait_stable(path, debounce, max_wait):
    last_size, stable, t0 = -1, 0, time.time()
    while time.time() - t0 < max_wait:
        try:
            size = os.path.getsize(path)
        except OSError:
            return False  # файл исчез
        if size == last_size:
            stable += 1
            if stable * 2 >= debounce:
                return True
        else:
            stable = 0
            last_size = size
        time.sleep(2)
    return True


def _remove_lock(path):
    try:
        os.unlink(path)
    except OSError:
        pass


def _pid_alive(pid):
    """Проверка живого процесса, безопасная для Windows (os.kill(0) убивает!)."""
    if os.name == "nt":
        import ctypes

        k = ctypes.windll.kernel32
        h = k.OpenProcess(0x100000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if h:
            k.CloseHandle(h)
            return True
        return False
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def _lock_pid_is_watcher(pid):
    """Живой процесс с этим PID — действительно watcher? PID мог быть
    переиспользован ОС под другую программу (классическая ловушка lock-файлов)."""
    try:
        import psutil

        p = psutil.Process(int(pid))
        if "python" not in (p.name() or "").lower():
            return False
        return "hds.cli watch" in " ".join(p.cmdline() or [])
    except Exception:  # noqa: BLE001
        return False


def _acquire_lock(cfg):
    lock = os.path.join(PROJECT_ROOT, "watch.lock")
    if os.path.exists(lock):
        try:
            pid = int(open(lock).read().strip())
            if _pid_alive(pid) and _lock_pid_is_watcher(pid):
                return None  # уже работает
        except (ValueError, OSError):
            pass
        _remove_lock(lock)
    with open(lock, "w") as f:
        f.write(str(os.getpid()))
    return lock


class _Handler:
    def __init__(self, q):
        self.q = q

    def dispatch(self, event):
        if getattr(event, "is_directory", False):
            return
        et = getattr(event, "event_type", "")
        if et in ("created", "modified"):
            self.q.put(("modified", event.src_path))
        elif et == "deleted":
            self.q.put(("deleted", event.src_path))
        elif et == "moved":
            self.q.put(("moved", (event.src_path, event.dest_path)))


def _worker(q, conn, emb, cfg, stop):
    debounce = int(dig(cfg, "watch.debounce_seconds", 8))
    max_wait = int(dig(cfg, "watch.max_stable_wait", 120))
    while not stop.is_set():
        try:
            kind, data = q.get(timeout=1)
        except queue.Empty:
            continue
        _state["last_event"] = time.time()
        try:
            if kind == "modified":
                path = os.path.abspath(data)
                if os.path.splitext(path)[1].lower() == ".tmp":
                    continue
                if path_excluded(path, cfg):
                    continue  # корзина ($RECYCLE.BIN / .Trash) и системные каталоги
                if wait_stable(path, debounce, max_wait) and os.path.exists(path):
                    status, _k = indexer.process_file(conn, emb, cfg, path)
                    print("[watch] %s -> %s" % (path, status), flush=True)
                    _bump(status)
            elif kind == "deleted":
                if dbmod.remove_path(conn, os.path.abspath(data)):
                    _bump("removed_from_index")
            elif kind == "moved":
                src, dst = os.path.abspath(data[0]), os.path.abspath(data[1])
                dst_excluded = path_excluded(dst, cfg)
                if dbmod.get_file_by_path(conn, src):
                    if dst_excluded:
                        dbmod.remove_path(conn, src)  # файл ушёл в корзину — из индекса
                    else:
                        # перезапись: запись для dst уже есть в индексе (со старым
                        # содержимым) — rename упал бы на UNIQUE(path). Старую
                        # запись убираем; актуальное содержимое доиндексируется
                        # ближайшим событием modified для dst
                        if dbmod.get_file_by_path(conn, dst):
                            dbmod.remove_path(conn, dst)
                        dbmod.rename_path(conn, src, dst)
                    _state["processed"] += 1
                elif not dst_excluded and os.path.exists(dst):
                    status, _k = indexer.process_file(conn, emb, cfg, dst)
                    _bump(status)
        except Exception as e:  # noqa: BLE001
            _state["errors"] += 1
            print("[watch] ошибка: %s" % e, file=sys.stderr, flush=True)
        finally:
            q.task_done()


def _bump(status):
    _state["processed"] += 1
    if status.startswith("error"):
        _state["errors"] += 1


def run_watch(cfg, roots=None):
    from watchdog.observers import Observer

    roots = roots or dig(cfg, "index.roots", [])
    if not roots:
        print("Не заданы index.roots в config.yaml")
        return 1
    lock = _acquire_lock(cfg)
    if lock is None:
        print("[watch] Наблюдатель уже запущен (watch.lock). Выход.")
        return 0
    conn = dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))
    emb = make_embedder(cfg)

    # Наблюдатель включается ДО reconcile-сверки: события, возникшие во время
    # долгого обхода, копятся в очереди и обрабатываются параллельно (worker
    # использует собственную БД-связь, WAL + busy_timeout разрешают параллель).
    q = queue.Queue()
    stop = threading.Event()
    worker_conn = dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))
    worker_emb = make_embedder(cfg)
    threading.Thread(target=_worker, args=(q, worker_conn, worker_emb, cfg, stop), daemon=True).start()

    obs = Observer(timeout=10)
    handler = _Handler(q)
    for root in roots:
        root = os.path.abspath(root)
        if os.path.isdir(root):
            obs.schedule(handler, root, recursive=True)
            print("[watch] наблюдаю: %s" % root)
    obs.start()

    if dig(cfg, "watch.reconcile_on_start", True):
        print("[watch] Сверка индекса с дисками (быстрый stat-обход)...")
        try:
            indexer.run_index(conn, emb, cfg, roots=roots, prune=True)
        except Exception as e:  # noqa: BLE001
            # сверка не должна убивать наблюдателя: события ФС важнее
            print("[watch] ошибка сверки (наблюдение продолжается): %s" % e,
                  file=sys.stderr, flush=True)
    print("[watch] Готово. События обрабатываются автоматически. Ctrl+C — остановка.", flush=True)
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        pass
    finally:
        stop.set()
        obs.stop()
        obs.join(timeout=5)
        _remove_lock(lock)
        print("[watch] Остановлен. Обработано событий: %d, ошибок: %d"
              % (_state["processed"], _state["errors"]))
    return 0


def status():
    return dict(_state)