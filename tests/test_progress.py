"""Тесты репортёра прогресса: счётчики, события, пауза, heartbeat, финал."""
import os
import sys
import unittest

from helpers import write_config  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds.progress import ProgressReporter  # noqa: E402


class ProgressReporterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp")
        os.makedirs(self.tmp, exist_ok=True)
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))
        write_config(self.tmp)
        self.rep = ProgressReporter(sec=0)  # поток печати отключён

    def test_counters_and_processed(self):
        self.rep.seen()
        self.rep.seen()
        self.rep.processed("indexed(2 чанков)", "text", 1.5, chunks=2)
        self.assertEqual(self.rep.seen_count, 2)
        self.assertEqual(self.rep.processed_count, 1)
        self.assertEqual(self.rep.chunks, 2)
        self.assertEqual(self.rep.by_kind.get("text"), 1)

    def test_events_recorded(self):
        self.rep.set_last_path(r"D:\x\файл.txt")
        self.rep.processed("indexed(1 чанков)", "pdf", 2.0, chunks=1)
        self.assertEqual(len(self.rep.events), 1)
        ev = self.rep.events[0]
        self.assertEqual(ev["path"], r"D:\x\файл.txt")
        self.assertEqual(ev["status"], "indexed")
        self.assertEqual(ev["chunks"], 1)

    def test_error_counted(self):
        self.rep.processed("error: boom", "pdf", 0.5)
        self.assertEqual(self.rep.errors, 1)

    def test_pause_flag(self):
        self.rep.set_paused(True)
        self.assertTrue(self.rep.paused)
        self.assertIn("ПАУЗА", self.rep._status_line())
        self.rep.set_paused(False)
        self.assertNotIn("ПАУЗА", self.rep._status_line())

    def test_progress_percent(self):
        self.rep.set_current(r"D:\x\v.mp4", "Whisper")
        self.rep.set_progress(43)
        line = self.rep._status_line()
        self.assertIn("43%", line)

    def _status_line(self):
        return self.rep._status_line()

    def test_heartbeat_data(self):
        self.rep.set_current(r"D:\x\v.mp4", "Whisper")
        self.rep.set_progress(50)
        d = self.rep.heartbeat_data()
        self.assertEqual(d["path"], r"D:\x\v.mp4")
        self.assertEqual(d["progress"], 50)
        self.assertIn("seen", d)

    def test_finish_final_elapsed(self):
        self.rep.finish()
        self.assertGreaterEqual(getattr(self.rep, "final_elapsed", -1), 0)

    def test_status_line_has_pause_prefix(self):
        self.rep.set_paused(True)
        self.assertIn("ПАУЗА", self.rep._status_line())

    def test_eta_and_rate_survive_empty_window(self):
        """РЕГРЕССИЯ: при долгой обработке одного файла скользящее окно пустело,
        rate_window давал 0, eta_sec исчезал — строка скорость/ETA пропадала из UI."""
        import time as t
        from collections import deque

        self.rep.set_total(100)
        for _ in range(20):
            self.rep.seen()
        self.rep.t0 = t.time() - 120  # стартовали 2 минуты назад
        self.rep._seen_ts = deque([t.time() - 400, t.time() - 350])  # всё старше окна
        rw = self.rep.rate_window()
        eta = self.rep.eta_sec()
        self.assertGreater(rw, 0, "скорость не должна обнуляться при пустом окне")
        self.assertIsNotNone(eta, "ETA не должен исчезать при пустом окне")
        self.assertGreater(eta, 0)
        self.assertLess(eta, 24 * 3600, "ETA не должен улетать в недели")


if __name__ == "__main__":
    unittest.main()