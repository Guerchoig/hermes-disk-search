"""Тесты веб-интерфейса: heartbeat-статус, сохранение настроек, деревья папок."""
import json
import os
import sys
import tempfile
import unittest

from helpers import FakeEmbedder, write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import indexer  # noqa: E402
from hds import db as dbmod  # noqa: E402
from hds.config import db_abs_path, load  # noqa: E402
from hds import ui_server  # noqa: E402


class HeartbeatStatusTests(unittest.TestCase):
    """РЕГРЕССИЯ: индексация из другого процесса не была видна в UI."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-ui-")
        write_config(self.tmp, extra="")
        self.hb = os.path.join(os.path.dirname(os.path.dirname(
            os.path.abspath(__file__))), "index.heartbeat.json")
        self.addCleanup(lambda: os.path.exists(self.hb) and os.remove(self.hb))
        indexer._ACTIVE_REPORTER = None
        indexer._LAST_REPORTER = None
        ui_server._LAST_HB_EVENTS.clear()

    def _write_hb(self, path="D:\\x\\v.mp4"):
        with open(self.hb, "w", encoding="utf-8") as f:
            json.dump({"ts": __import__("time").time(), "path": path,
                       "phase": "Whisper", "progress": 55,
                       "seen": 5, "processed": 3, "errors": 0, "paused": False,
                       "events": [{"path": r"D:\x\a.txt", "status": "indexed",
                                   "kind": "text", "dur": 0.4, "chunks": 2,
                                   "ts": 1000.0}]}, f)

    def test_heartbeat_makes_running_true(self):
        self._write_hb()
        st = ui_server._index_state()
        self.assertTrue(st["running"], "heartbeat из другого процесса должен быть виден")
        self.assertEqual(st["current"]["path"], "D:\\x\\v.mp4")
        self.assertEqual(st["current"]["progress"], 55)

    def test_heartbeat_carries_events(self):
        """РЕГРЕССИЯ: «Последние обработанные» не обновлялись, когда индексация
        идёт в другом процессе (watcher/CLI): heartbeat не содержал events."""
        self._write_hb()
        st = ui_server._index_state()
        self.assertEqual(len(st["events"]), 1)
        self.assertEqual(st["events"][0]["path"], r"D:\x\a.txt")

    def test_events_survive_after_run_finish(self):
        """Прогон в другом процессе завершился: heartbeat удалён,
        но последние события должны остаться в UI."""
        self._write_hb()
        ui_server._index_state()  # кеширует events из heartbeat
        os.remove(self.hb)
        st = ui_server._index_state()
        self.assertEqual(len(st["events"]), 1)

    def test_stale_heartbeat_not_running(self):
        import time

        self._write_hb()
        with open(self.hb, "w", encoding="utf-8") as f:
            json.dump({"ts": 1.0, "path": "x"}, f)  # очень старый
        st = ui_server._index_state()
        self.assertFalse(st["running"])

    def test_no_heartbeat_not_running(self):
        if os.path.exists(self.hb):
            os.remove(self.hb)
        st = ui_server._index_state()
        self.assertFalse(st["running"])


class SaveConfigTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-uicfg-")
        self.cfg = write_config(self.tmp)

    def test_invalid_yaml_refused(self):
        res = ui_server._save_config("index: [unclosed")
        self.assertFalse(res["ok"])
        self.assertIn("Ошибка YAML", res["msg"])

    def test_valid_yaml_saved(self):
        good = load() and open(self.cfg, encoding="utf-8-sig").read()
        res = ui_server._save_config(good)
        self.assertTrue(res["ok"])

    def test_changed_db_path_roundtrip(self):
        import yaml

        res = ui_server._save_config("db_path: 'D:\\test\\index.db'\nindex:\n  roots: []\n")
        self.assertTrue(res["ok"])
        cfg = load()
        self.assertTrue(str(cfg["db_path"]).lower().startswith("d:"))
        yaml.safe_load(open(self.cfg, encoding="utf-8-sig"))  # конфиг остаётся валидным


class ExcludePathsTests(unittest.TestCase):
    """Фича: редактирование index.exclude_paths (исключаемые пути-префиксы)."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-uiexcl-")
        self.cfg = write_config(self.tmp)

    def _yaml(self):
        import yaml
        with open(self.cfg, encoding="utf-8-sig") as f:
            return yaml.safe_load(f)

    def test_save_inserts_and_dedupes(self):
        res = ui_server._set_exclude_paths([r"D:\Backup\Downloads\opencv",
                                            "  " + r"d:\backup\downloads\OPENCV  ",
                                            "", r"D:\Backup\Downloads\cmake-4.3.1"])
        self.assertTrue(res["ok"], res.get("msg"))
        data = self._yaml()
        eps = data["index"]["exclude_paths"]
        self.assertEqual(len(eps), 2, "дубликаты и пустые отбрасываются")
        self.assertEqual(data["index"]["exclude_dirs"][0], "$RECYCLE.BIN",
                         "соседние параметры не затронуты")

    def test_second_save_replaces_block(self):
        ui_server._set_exclude_paths([r"D:\a", r"D:\b"])
        res = ui_server._set_exclude_paths([r"D:\c"])
        self.assertTrue(res["ok"], res.get("msg"))
        with open(self.cfg, encoding="utf-8-sig") as f:
            text = f.read()
        self.assertEqual(text.count("exclude_paths:"), 1, "дублей блока быть не должно")
        self.assertEqual(self._yaml()["index"]["exclude_paths"], [r"D:\c"])

    def test_save_over_empty_inline_list(self):
        """В config.yaml вида 'exclude_paths: []' замена должна работать."""
        with open(self.cfg, "w", encoding="utf-8") as f:
            f.write("index:\n  roots: []\n  exclude_dirs: ['X']\n"
                    "  exclude_paths: []\ndb_path: 't.db'\n")
        res = ui_server._set_exclude_paths([r"D:\x\y"])
        self.assertTrue(res["ok"], res.get("msg"))
        with open(self.cfg, encoding="utf-8-sig") as f:
            text = f.read()
        self.assertEqual(text.count("exclude_paths:"), 1)
        self.assertEqual(self._yaml()["index"]["exclude_paths"], [r"D:\x\y"])

    def test_save_empty_list(self):
        res = ui_server._set_exclude_paths([])
        self.assertTrue(res["ok"], res.get("msg"))
        self.assertEqual(self._yaml()["index"]["exclude_paths"], [])

    def test_invalid_input_refused(self):
        res = ui_server._set_exclude_paths("не список")
        self.assertFalse(res["ok"])

    def test_block_form_does_not_eat_following_keys(self):
        """РЕГРЕССИЯ: замена блока не должна съедать соседние ключи секции index."""
        with open(self.cfg, "w", encoding="utf-8") as f:
            f.write("index:\n  roots: []\n  exclude_dirs: ['X']\n"
                    "  exclude_paths:\n    - 'D:\\old'\n"
                    "  max_file_mb: 200\ndb_path: 't.db'\n")
        res = ui_server._set_exclude_paths([r"D:\new"])
        self.assertTrue(res["ok"], res.get("msg"))
        data = self._yaml()
        self.assertEqual(data["index"]["exclude_paths"], [r"D:\new"])
        self.assertEqual(data["index"]["max_file_mb"], 200,
                         "соседний ключ не должен исчезать при замене блока")


class HdsPidsTests(unittest.TestCase):
    """РЕГРЕССИЯ: _hds_pids через PowerShell тихо возвращал пустой список
    (ломались кавычки WQL) — кнопка «Остановить watcher» не работала."""

    def test_finds_process_by_cmdline(self):
        import subprocess
        import sys
        import time

        p = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        try:
            time.sleep(1.5)
            pids = ui_server._hds_pids(r"time\.sleep")
            self.assertIn(p.pid, pids,
                          "процесс с 'time.sleep' в cmdline должен находиться")
        finally:
            p.kill()
            p.wait()

    def test_no_match_returns_empty(self):
        self.assertEqual(ui_server._hds_pids(r"hds\.cli definitely-not-running-xyz"), [])


class TreeBuildTests(unittest.TestCase):
    """РЕГРЕССИИ: не начатые папки не жёлтые; проиндексированные — зелёные;
    смесь — жёлтая. Статусы родителя — свёртка по потомкам."""

    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-tree-")
        self.fixtures = os.path.join(self.tmp, "root")
        write_text(self.fixtures, "a.txt", "данные корня")
        write_text(os.path.join(self.fixtures, "sub"), "b.txt", "вложенный")
        write_config(self.tmp, roots=[self.fixtures])
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))

    def _find(self, nodes, name):
        for n in nodes:
            if os.path.basename(n["path"]).lower() == name.lower():
                return n
            r = self._find(n.get("children", []), name)
            if r:
                return r
        return None

    def test_empty_db_all_none(self):
        res = ui_server._build_trees()
        self.assertEqual(res["trees"][0]["status"], "none")
        sub = self._find([res["trees"][0]], "sub")
        self.assertEqual(sub["status"], "none")
        self.assertEqual(sub["files"], 1)

    def test_done_when_all_indexed(self):
        conn = __import__("hds.db", fromlist=["db"]).connect(
            __import__("hds.config", fromlist=["db_abs_path"]).db_abs_path(load()), 8)
        emb = FakeEmbedder(8)
        for root, dirs, files in os.walk(self.fixtures):
            for fn in files:
                p = os.path.join(root, fn)
                indexer.process_file(conn, emb, load(), p)
        conn.close()
        res = ui_server._build_trees()
        self.assertEqual(res["trees"][0]["status"], "done")
        sub = self._find([res["trees"][0]], "sub")
        self.assertEqual(sub["status"], "done")
        self.assertEqual(sub["status"], "done")

    def test_mixed_children_make_parent_partial(self):
        # a.txt в корне (done), sub не начата (none), sub2 проиндексирована (done)
        write_text(os.path.join(self.fixtures, "sub2"), "c.txt", "готовый контент")
        conn = dbmod.connect(db_abs_path(load()), 8)
        for root, dirs, files in os.walk(os.path.join(self.fixtures, "sub2")):
            for fn in files:
                indexer.process_file(conn, FakeEmbedder(8), load(),
                                     os.path.join(root, fn))
        conn.close()
        res = ui_server._build_trees()
        self.assertEqual(res["trees"][0]["status"], "partial")

    def _find(self, nodes, name):
        for n in nodes:
            if os.path.basename(n["path"]).lower() == name.lower():
                return n
            r = self._find(n.get("children", []), name)
            if r:
                return r
        return None


if __name__ == "__main__":
    unittest.main()