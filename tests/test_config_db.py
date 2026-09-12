"""Тесты конфигурации и БД (включая регрессии indexed_at и vec-миграции)."""
import os
import sqlite3
import struct
import sys
import unittest

from helpers import FakeEmbedder, write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds.config import dig, load  # noqa: E402
from hds import db as dbmod  # noqa: E402


class ConfigTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-cfg-")
        self.cfg = write_config(self.tmp)
        self.addCleanup(lambda: os.path.exists(self.cfg) and os.remove(self.cfg))

    def test_load_and_dig(self):
        cfg = load()
        self.assertEqual(dig(cfg, "embedding.dim"), 8)
        self.assertEqual(dig(cfg, "index.max_chunks"), 2000)
        self.assertIsNone(dig(cfg, "no.such.key", None))
        self.assertEqual(dig(cfg, "no.such.key", "def"), "def")

    def test_db_path_isolated(self):
        cfg = load()
        self.assertTrue(db_abs_path(cfg).startswith(self.tmp))


from tempfile import mkdtemp  # noqa: E402
from hds.config import db_abs_path  # noqa: E402
import tempfile  # noqa: E402


class DbTests(unittest.TestCase):
    def setUp(self):
        self.tmp = mkdtemp(prefix="hds-db-")
        self.cfg = write_config(self.tmp)
        self.db = db_abs_path(load())
        self.conn = dbmod.connect(self.db, 8)
        self.addCleanup(self.conn.close)

    def test_schema_and_vec_load(self):
        c = self.conn.execute("SELECT COUNT(*) FROM chunks_vec").fetchone()
        self.assertIsNotNone(c)  # sqlite-vec загружен, vec0-таблица создана

    def test_finish_file_sets_indexed_at(self):
        """РЕГРЕССИЯ: indexed_at не записывался -> папки вечно жёлтые."""
        fid = dbmod.upsert_file(self.conn, r"D:\x\a.txt", ".txt", "text", 10, 1.0, "h1")
        dbmod.finish_file(self.conn, fid, "indexed")
        row = self.conn.execute("SELECT status, indexed_at FROM files WHERE id=?",
                                (fid,)).fetchone()
        self.assertEqual(row["status"], "indexed")
        self.assertIsNotNone(row["indexed_at"], "indexed_at должен записываться")

    def test_finish_file_error_keeps_indexed_at(self):
        fid = dbmod.upsert_file(self.conn, r"D:\x\b.txt", ".txt", "text", 5, 6, "h2")
        dbmod.finish_file(self.conn, fid, "indexed")
        dbmod.finish_file(conn=self.conn, file_id=fid, status="error", error="боом")
        row = self.conn.execute("SELECT indexed_at, status FROM files WHERE id=?",
                                (fid,)).fetchone()
        self.assertEqual(row["status"], "error")
        self.assertIsNotNone(row["indexed_at"])

    def test_backfill_indexed_at_from_mtime(self):
        """РЕГРЕССИЯ: старые записи с NULL indexed_at заполняются по mtime."""
        conn2 = dbmod.connect(self.db, 8)  # новое подключение -> бэкфилл
        rows = conn2.execute(
            "SELECT COUNT(*) FROM files WHERE status='indexed' AND indexed_at IS NULL"
        ).fetchone()[0]
        conn2.close()
        self.assertEqual(rows, 0)

    def test_vec_dim_migration_rebuilds_table(self):
        """РЕГРЕССИЯ: смена embedding.dim пересоздаёт векторную таблицу."""
        v = [0.1] * 8
        blob = struct.pack("<8f", *v)
        fid = dbmod.upsert_file(self.conn, r"D:\x\c.txt", ".txt", "text", 3, 4, "hh")
        cid = dbmod.add_chunk(self.conn, fid, 0, None, None, None, "x")
        dbmod.add_vector(self.conn, cid, blob)
        self.conn.close()  # иначе database is locked
        dbmod.connect(self.db, 16).close()  # другая размерность
        c = sqlite3.connect(self.db)
        c.enable_load_extension(True)
        __import__("sqlite_vec").load(c)
        c.enable_load_extension(False)
        n = c.execute("SELECT COUNT(*) FROM chunks_vec").fetchone()[0]
        c.close()
        self.assertEqual(n, 0, "векторная таблица должна быть пересоздана")

    def test_rename_and_remove_path(self):
        fid = dbmod.upsert_file(self.conn, r"D:\old\a.txt", ".txt", "text", 5, 6, "h3")
        cid = dbmod.add_chunk(self.conn, fid, 0, None, None, None, "txt")
        dbmod.add_vector(self.conn, cid, struct.pack("<8f", *([0.2] * 8)))
        dbmod.rename_path(self.conn, r"D:\old\a.txt", r"D:\new\a.txt")
        self.assertIsNone(dbmod.get_file_by_path(self.conn, r"D:\old\a.txt"))
        self.assertIsNotNone(dbmod.get_file_by_path(self.conn, r"D:\new\a.txt"))
        self.assertTrue(dbmod.remove_path(self.conn, r"D:\new\a.txt"))
        self.assertFalse(dbmod.remove_path(self.conn, r"D:\new\a.txt"))
        self.assertEqual(self.conn.execute("SELECT COUNT(*) FROM chunks").fetchone()[0], 0)

    def test_stats(self):
        st = dbmod.stats(self.conn)
        self.assertIn("by_kind", st) and self.assertIn("chunks", st)


if __name__ == "__main__":
    unittest.main()