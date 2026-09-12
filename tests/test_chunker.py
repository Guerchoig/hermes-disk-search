"""Тесты чанкинга: разрезание длинных текстов, перекрытие, метаданные сегментов."""
import os
import sys
import unittest

from helpers import write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds.chunker import make_chunks  # noqa: E402


class ChunkerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp")
        os.makedirs(self.tmp, exist_ok=True)
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp, ignore_errors=True))
        self.cfg = write_config(self.tmp)

    def test_simple_segment(self):
        chunks = make_chunks([{"text": "привет мир", "page": 3,
                               "t_start": None, "t_end": None}])
        self.assertEqual(len(chunks), 1)
        self.assertEqual(chunks[0]["page"], 3)
        self.assertEqual(chunks[0]["text"], "привет мир")

    def test_long_text_split(self):
        """Длинный текст режется на куски <= size."""
        text = ("Абзац с текстом про документооборот. " * 200).strip()
        chunks = make_chunks([{"text": text, "page": 7, "t_start": None, "t_end": None}],
                             size=300, overlap=50)
        self.assertGreater(len(chunks), 1)
        for c in chunks:
            self.assertLessEqual(len(c["text"]), 300 + 60)  # допуск на границу слов
        # перекрытие: конец предыдущего в начале следующего
        if len(chunks) > 1:
            tail = chunks[0]["text"][-50:].strip()
            self.assertIn(tail[:20], chunks[1]["text"][:80])

    def test_empty_segments_skipped(self):
        chunks = make_chunks([{"text": "", "page": 1}, {"text": "  ", "page": 2},
                              {"text": "контент", "page": 3}])
        self.assertEqual(len(chunks), 1)
        self.assertEqual(chunks[0]["page"], 3)

    def test_media_timecodes_preserved(self):
        segs = [{"text": "первая реплика", "page": None, "t_start": 0.0, "t_end": 5.0},
                {"text": "вторая реплика", "page": None, "t_start": 5.0, "t_end": 9.0}]
        chunks = make_chunks(segs, size=1000, overlap=0)
        self.assertEqual(chunks[0]["t_start"], 0.0)
        self.assertEqual(chunks[0]["t_end"], 5.0)


if __name__ == "__main__":
    unittest.main()