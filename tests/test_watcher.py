"""Тесты watcher'а: маршрутизация событий, пауза-файлы, PID, lock."""
import os
import sys
import tempfile
import unittest
from unittest import mock

from helpers import write_config  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds.indexer import path_excluded  # noqa: E402
from hds import watcher  # noqa: E402


class _Evt:
    def __init__(self, event_type, src, dest=None, is_dir=False):
        self.event_type = event_type
        self.src_path = src
        self.dest_path = dest
        self.is_directory = is_dir


class HandlerRoutingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp")
        os.makedirs(self.tmp, exist_ok=True)
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))
        write_config(self.tmp)
        import queue

        self.q = queue.Queue()
        self.h = watcher._Handler(self.q)

    def _pop(self):
        return self.q.get_nowait()

    def test_created_event(self):
        self.h.dispatch(_Evt("created", r"D:\x\new.txt"))
        kind, data = self.q.get_nowait()
        self.assertEqual(kind, "modified")
        self.assertEqual(data, r"D:\x\new.txt")

    def _get_kind(self):
        return self.q.get_nowait()

    def test_modified_event(self):
        self.h.dispatch(_Evt("modified", r"D:\x\f.txt"))
        kind, data = self.q.get_nowait()
        self.assertEqual(kind, "modified")
        self.assertEqual(data, r"D:\x\f.txt")

    def test_deleted_event(self):
        self.h.dispatch(_Evt("deleted", r"D:\x\f.txt"))
        kind, data = self.q.get_nowait()
        self.assertEqual(kind, "deleted")
        self.assertEqual(data, r"D:\x\f.txt")

    def test_moved_event_pair(self):
        self.h.dispatch(_Evt("moved", r"D:\a.txt", r"D:\b.txt"))
        kind, data = self.q.get_nowait()
        self.assertEqual(kind, "moved")
        self.assertEqual(data, (r"D:\a.txt", r"D:\b.txt"))

    def test_directory_events_ignored(self):
        self.h.dispatch(_Evt("created", r"D:\folder", is_dir=True))
        with self.assertRaises(Exception):
            self.q.get_nowait()


class PidAliveTests(unittest.TestCase):
    def test_current_process_alive(self):
        self.assertTrue(watcher._pid_alive(os.getpid()))

    def test_dead_pid_not_alive(self):
        # PID заведомо не занят (выбираем крупный номер)
        if os.name == "nt":
            self.assertFalse(watcher._pid_alive(999999))


class LockTests(unittest.TestCase):
    """РЕГРЕССИЯ: проверка watch.lock и его запись были НЕатомарны — два
    одновременных старта (автозапуск + кнопка в UI / двойной клик) давали
    ДВА живых watcher'а, которые потом индексировали одни и те же файлы."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-lock-")
        self.lock = os.path.join(self.tmp, "watch.lock")
        patcher = mock.patch.object(watcher, "PROJECT_ROOT", self.tmp)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))

    def _write_lock(self, pid):
        with open(self.lock, "w") as f:
            f.write(str(pid))

    def test_acquire_creates_lock_with_own_pid(self):
        self.assertEqual(watcher._acquire_lock({}), self.lock)
        self.assertEqual(open(self.lock).read().strip(), str(os.getpid()))

    def test_second_watcher_refuses_when_lock_alive(self):
        """Главное свойство: живой watcher в lock → второй НЕ стартует."""
        self._write_lock(12345)
        with mock.patch.object(watcher, "_pid_alive", lambda p: True), \
                mock.patch.object(watcher, "_lock_pid_is_watcher", lambda p: True):
            self.assertIsNone(watcher._acquire_lock({}))
        self.assertEqual(open(self.lock).read().strip(), "12345")  # lock не переписан

    def test_stale_lock_is_replaced(self):
        """После падения watcher'а lock от мёртвого PID снимается и занимается заново."""
        self._write_lock(999999)
        with mock.patch.object(watcher, "_pid_alive", lambda p: False):
            self.assertEqual(watcher._acquire_lock({}), self.lock)
        self.assertEqual(open(self.lock).read().strip(), str(os.getpid()))

    def test_empty_lock_is_stale(self):
        open(self.lock, "w").close()
        self.assertTrue(watcher._lock_is_stale(self.lock))
        self.assertEqual(watcher._acquire_lock({}), self.lock)

    def test_foreign_pid_in_lock_is_replaced(self):
        """PID переиспользован другой программой (не watcher) → lock устаревший."""
        self._write_lock(4242)
        with mock.patch.object(watcher, "_pid_alive", lambda p: True), \
                mock.patch.object(watcher, "_lock_pid_is_watcher", lambda p: False):
            self.assertTrue(watcher._lock_is_stale(self.lock))
            self.assertEqual(watcher._acquire_lock({}), self.lock)

    def test_without_psutil_live_pid_counts_as_watcher(self):
        """Без psutil доказать «чужой PID» нельзя — считаем живым (дубли хуже)."""
        with mock.patch.dict(sys.modules, {"psutil": None}):
            self.assertTrue(watcher._lock_pid_is_watcher(os.getpid()))

    def test_unwritable_lock_does_not_start_second_watcher(self):
        """Ошибка ФС на lock — не рискуем вторым watcher'ом (None = не стартуем)."""
        with mock.patch.object(watcher.os, "open", side_effect=OSError("denied")):
            self.assertIsNone(watcher._acquire_lock({}))


class PathExcludedTests(unittest.TestCase):
    def setUp(self):
        self.tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp")
        os.makedirs(self.tmp, exist_ok=True)
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))
        self.cfg_path = write_config(self.tmp)

    def test_recycle_bin_excluded(self):
        """РЕГРЕССИЯ: корзина исключается на всех уровнях (передаём СЛОВАРЬ cfg)."""
        from hds.config import load

        cfg = load()
        self.assertTrue(path_excluded(r"D:\$RECYCLE.BIN\S-1\f.txt", cfg=cfg))
        self.assertTrue(path_excluded(r"C:\Users\u\.Trash\doc.pdf", cfg=cfg))


def path_excluded(path, cfg):
    from hds.indexer import path_excluded as _pe

    return _pe(path, cfg)


if __name__ == "__main__":
    unittest.main()