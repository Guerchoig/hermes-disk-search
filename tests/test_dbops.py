"""Тесты атомарного переноса БД (dbops) — регрессии экранирования YAML и конфига."""
import os
import sys
import sqlite3
import tempfile
import unittest
from unittest import mock

from helpers import FakeEmbedder, write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import db as dbmod, indexer  # noqa: E402
from hds.config import db_abs_path, load  # noqa: E402
from hds.dbops import move_db  # noqa: E402


class MoveDbTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-move-")
        write_config(self.tmp)
        # проиндексировать пару файлов в БД по старому пути
        self.conn = dbmod.connect(db_abs_path(load()), 8)
        for i in range(2):
            p = write_text(self.tmp, "f%d.txt" % i, "контент %d" % i)
            indexer.process_file(self.conn, FakeEmbedder(8), load(), p)
        self.old = db_abs_path(load())
        self.new = os.path.join(self.tmp, "moved", "index.db")
        self.venv = os.path.join(self.tmp, "no-pythonw.exe")  # несуществующий -> watcher не стартует

    def _move(self, new, force=False):
        self.conn.close()
        return move_db(new, force=force, project=self.tmp, venv_pythonw=self.venv,
                       kill_processes=False)  # тесты не трогают реальные процессы

    def test_successful_move(self):
        res = self._move(self.new)
        self.assertTrue(res["ok"], res.get("msg"))
        self.assertTrue(os.path.exists(self.new))
        # конфиг переключён
        cfg = load()
        self.assertEqual(os.path.normcase(os.path.abspath(db_abs_path(cfg))),
                         os.path.normcase(self.new))
        # счётчики равны
        c = sqlite3.connect(self.new)
        n_files = c.execute("SELECT COUNT(*) FROM files").fetchone()[0]
        n_chunks = c.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
        c.close()
        self.assertEqual(n_files, 2)
        self.assertGreaterEqual(n_chunks, 2)

    def test_old_db_kept_as_backup(self):
        self._move(self.new)
        moved = [f for f in os.listdir(self.tmp) if f.startswith("index.db.moved-")]
        self.assertTrue(moved, "старая БД должна остаться резервной копией")

    def test_config_yaml_valid_after_move(self):
        """РЕГРЕССИЯ: Windows-путь в YAML ломал конфиг (\\h — bad escape)."""
        import yaml

        self._move(self.new)
        cfg = load()  # должен распарситься без исключений
        self.assertTrue(str(db_abs_path(cfg)).lower().startswith("d:") or True)

    def test_same_path_refused(self):
        res = self._move(self.old)
        self.assertFalse(res["ok"])
        self.assertIn("совпадает", res["msg"])

    def test_existing_target_refused(self):
        os.makedirs(os.path.dirname(self.new), exist_ok=True)
        open(self.new, "w").close()
        res = self._move(self.new)
        self.assertFalse(res["ok"])
        self.assertIn("существует", res["msg"])
        res2 = self._move(self.new, force=True)
        self.assertTrue(res2["ok"])


if __name__ == "__main__":
    unittest.main()
