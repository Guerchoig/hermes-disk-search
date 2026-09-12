"""Тесты индексатора: полный цикл, лимиты, корзина, переезд, пауза/стоп, heartbeat."""
import json
import os
import sys
import tempfile
import threading
import time
import unittest

from helpers import FakeEmbedder, write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import db as dbmod, indexer  # noqa: E402
from hds.config import db_abs_path, load  # noqa: E402

PROJECT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class IndexerTestBase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-idx-")
        self.fixtures = os.path.join(self.tmp, "root")
        self.cfg = write_config(self.tmp)
        self.addCleanup(self._cleanup)

    def _cleanup(self):
        indexer._ACTIVE_REPORTER = None
        for f in ("index.stop", "index.pause", "index.heartbeat.json"):
            p = os.path.join(PROJECT, f)
            try:
                os.remove(p)
            except OSError:
                pass

    def _conn(self):
        return dbmod.connect(db_abs_path(load()), 8)

    def _emb(self):
        return FakeEmbedder(dim=8)


class ProcessFileTests(IndexerTestBase):
    def test_indexed_sets_indexed_at_and_chunks(self):
        p = write_text(self.fixtures, "doc.txt", "Текст про 1С:Документооборот " * 30)
        conn = self._conn()
        status, kind = indexer.process_file(conn, FakeEmbedder(8), load(), p)
        conn.close()
        self.assertTrue(status.startswith("indexed"))
        self.assertEqual(kind, "text")
        c = dbmod.connect(db_abs_path(load()), 8)
        row = c.execute("SELECT status, indexed_at, chunk_count FROM files WHERE path=?",
                        (p,)).fetchone()
        c.close()
        self.assertEqual(row[0], "indexed")
        self.assertIsNotNone(row[1], "РЕГРЕССИЯ: indexed_at должен записываться")
        self.assertGreater(row[2], 0)

    def test_unchanged_skip(self):
        p = write_text(self.fixtures, "a.txt", "контент")
        conn = self._conn()
        indexer.process_file(conn, FakeEmbedder(8), load(), p)
        status, _ = indexer.process_file(conn, FakeEmbedder(8), load(), p)
        conn.close()
        self.assertEqual(status, "unchanged")

    def test_modified_reindexed(self):
        p = write_text(self.fixtures, "b.txt", "старый текст")
        conn = self._conn()
        indexer.process_file(conn, FakeEmbedder(8), load(), p)
        write_text(self.fixtures, "b.txt", "новый текст про документооборот")
        status, _ = indexer.process_file(conn, FakeEmbedder(8), load(), p)
        conn.close()
        self.assertTrue(status.startswith("indexed"), status)

    def test_moved_reuses_chunks(self):
        """Фича: переименование не переиндексирует, а переезжает."""
        p = write_text(self.fixtures, "c.txt", "текст для переезда " * 10)
        conn = self._conn()
        indexer.process_file(conn, FakeEmbedder(8), load(), p)
        n_before = conn.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
        conn.close()
        new_p = os.path.join(self.fixtures, "c2.txt")
        os.replace(p, new_p)
        conn = self._conn()
        status, _ = indexer.process_file(conn, FakeEmbedder(8), load(), new_p)
        conn.close()
        self.assertEqual(status, "moved")
        c = dbmod.connect(db_abs_path(load()), 8)
        row = c.execute("SELECT chunk_count FROM files WHERE path=?", (new_p,)).fetchone()
        c.close()
        self.assertEqual(row[0], n_before, "чанки должны переиспользоваться")

    def test_recycle_bin_skipped(self):
        """РЕГРЕССИЯ: файлы из корзины не индексируются (Windows и macOS)."""
        for p in (r"D:\$RECYCLE.BIN\S-1\f.txt", r"C:\Users\x\.Trash\f.pdf"):
            conn = self._conn()
            status, kind = indexer.process_file(conn, FakeEmbedder(8), load(), p)
            conn.close()
            self.assertEqual(status, "skipped_excluded", p)

    def test_unknown_ext_skipped(self):
        p = write_text(self.fixtures, "file.exe", "MZ...")
        conn = self._conn()
        status, _ = indexer.process_file(conn, FakeEmbedder(8), load(), p)
        conn.close()
        self.assertEqual(status, "skipped_type")

    def test_too_big_skipped(self):
        p = write_text(self.fixtures, "big.txt", "x" * 100)
        cfg = load()
        cfg["index"]["max_file_mb"] = 0  # 0 МБ -> всё пропускается
        conn = self._conn()
        status, _ = indexer.process_file(conn, FakeEmbedder(8), cfg, p)
        conn.close()
        self.assertEqual(status, "skipped_big")


class RunIndexTests(IndexerTestBase):
    def test_single_file_root(self):
        """РЕГРЕССИЯ: одиночный файл как корень."""
        p = write_text(self.fixtures, "one.txt", "контент")
        indexer.run_index(self._conn(), FakeEmbedder(8), load(),
                          roots=[p], progress_sec=0)
        c = dbmod.connect(db_abs_path(load()), 8)
        n = c.execute("SELECT COUNT(*) FROM files WHERE path=?", (p,)).fetchone()[0]
        c.close()
        self.assertEqual(n, 1)

    def test_max_chunks_truncates(self):
        """РЕГРЕССИЯ: гигантский текст обрезается по index.max_chunks."""
        long_text = "строка данных " * 400
        p = write_text(self.fixtures, "huge.txt", long_text)
        cfg = load()
        cfg["index"]["max_chunks"] = 2
        indexer.run_index(self._conn(), FakeEmbedder(8), cfg, roots=[p],
                          full=True, progress_sec=0, prune=False)
        c = dbmod.connect(db_abs_path(load()), 8)
        row = c.execute("SELECT chunk_count FROM files WHERE path=?", (p,)).fetchone()
        c.close()
        self.assertEqual(row[0], 2)

    def test_stop_file_stops_gracefully(self):
        """Фича: index.stop аккуратно останавливает; стоп-файл удаляется."""
        for i in range(6):
            write_text(self.fixtures, "f%d.txt" % i, "текст номер %d " % i * 40)
        open(os.path.join(PROJECT, "index.stop"), "w").close()
        res = indexer.run_index(self._conn(), FakeEmbedder(8), load(),
                                roots=[self.fixtures], full=True,
                                progress_sec=0, prune=False)
        self.assertTrue(res.get("stopped"))
        self.assertFalse(os.path.exists(os.path.join(PROJECT, "index.stop")),
                         "стоп-файл должен удаляться автоматически")

    def test_pause_file_resumes(self):
        """Фича: index.pause приостанавливает и снимается."""
        write_text(self.fixtures, "g.txt", "контент для паузы")
        pause = os.path.join(PROJECT, "index.pause")
        open(pause, "w").close()
        t = threading.Thread(target=lambda: indexer.run_index(
            self._conn(), FakeEmbedder(8), load(), roots=[self.fixtures],
            progress_sec=0, prune=False), daemon=True)
        t.start()
        time.sleep(2)
        rep = getattr(indexer, "_ACTIVE_REPORTER", None)
        self.assertIsNotNone(rep)
        self.assertTrue(rep.paused, "индексация должна быть в состоянии паузы")
        os.remove(pause)
        t.join(timeout=30)
        self.assertFalse(t.is_alive())

    def test_heartbeat_written_during_and_removed_after(self):
        """Фича: кросс-процессный статус (heartbeat): пишется при паузе, удаляется после."""
        write_text(self.fixtures, "h.txt", "контент heartbeat")
        hb = os.path.join(PROJECT, "index.heartbeat.json")
        pause = os.path.join(PROJECT, "index.pause")
        open(pause, "w").close()
        res = {}
        t = threading.Thread(target=lambda: res.update(
            indexer.run_index(self._conn(), FakeEmbedder(8), load(), roots=[self.fixtures],
                              full=True, progress_sec=0, prune=False)), daemon=True)
        t.start()
        seen_paused = False
        for _ in range(100):
            if os.path.exists(hb):
                with open(hb, encoding="utf-8") as f:
                    d = json.load(f)
                if d.get("paused"):
                    seen_paused = True
                    break
            time.sleep(0.1)
        self.assertTrue(seen_paused, "heartbeat в паузе должен содержать paused=True")
        os.remove(pause)
        t.join(timeout=60)
        self.assertFalse(t.is_alive())
        self.assertFalse(os.path.exists(hb), "heartbeat удаляется после прогона")
        self.assertTrue(res)

    def test_last_reporter_snapshot_kept(self):
        """Фича: после завершения UI видит счётчики последнего прогона."""
        p = write_text(self.fixtures, "i.txt", "контент снапшота")
        indexer.run_index(self._conn(), FakeEmbedder(8), load(), roots=[p],
                          progress_sec=0, prune=False)
        last = indexer._LAST_REPORTER
        self.assertIsNotNone(last)
        self.assertEqual(last.seen_count, 1)