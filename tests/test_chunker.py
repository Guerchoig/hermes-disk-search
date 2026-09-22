"""Тесты структурного чанкера (Фаза 2): границы по предложениям, overlap целыми
предложениями, точные страницы/таймкоды, путь заголовков."""
import os
import sys
import unittest

from helpers import write_config  # noqa: I100

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

    def test_long_text_split_by_sentences(self):
        """РЕГРЕССИЯ: раньше чанкер резал по ближайшему \\n в середине окна —
        чанки начинались/заканчивались посреди предложения."""
        text = ("Абзац с текстом про документооборот. " * 200).strip()
        chunks = make_chunks([{"text": text, "page": 7, "t_start": None, "t_end": None}],
                             size=300, overlap=50)
        self.assertGreater(len(chunks), 1)
        for c in chunks:
            # допуск: overlap-хвост целыми предложениями + граница слова
            self.assertLessEqual(len(c["text"]), 300 + 120)
        # перекрытие: начало следующего чанка содержит последнее предложение
        # предыдущего ЦЕЛИКОМ (а не хвост N символов)
        last_sent = "Абзац с текстом про документооборот."
        self.assertTrue(chunks[1]["text"].startswith(last_sent),
                        "overlap должен начинаться с целого предложения: %r"
                        % chunks[1]["text"][:60])

    def test_empty_segments_skipped(self):
        chunks = make_chunks([{"text": "", "page": 1}, {"text": "  ", "page": 2},
                              {"text": "контент", "page": 3}])
        self.assertEqual(len(chunks), 1)
        self.assertEqual(chunks[0]["page"], 3)

    def test_media_timecodes_preserved(self):
        """РЕГРЕССИЯ: чанк не смешивает сегменты с разными таймкодами."""
        segs = [{"text": "первая реплика", "page": None, "t_start": 0.0, "t_end": 5.0},
                {"text": "вторая реплика", "page": None, "t_start": 5.0, "t_end": 9.0}]
        chunks = make_chunks(segs, size=1000, overlap=0)
        self.assertEqual(len(chunks), 2)
        self.assertEqual(chunks[0]["t_start"], 0.0)
        self.assertEqual(chunks[0]["t_end"], 5.0)
        self.assertEqual(chunks[1]["t_start"], 5.0)
        self.assertEqual(chunks[1]["t_end"], 9.0)

    def test_pdf_pages_not_merged(self):
        """РЕГРЕССИЯ: раньше PDF-чанк склеивался из нескольких страниц, а page
        брался от первого сегмента — ссылка на страницу была неверной."""
        segs = [{"text": "Текст первой страницы, довольно длинный. " * 20, "page": 1,
                 "t_start": None, "t_end": None},
                {"text": "Текст второй страницы про договоры. " * 20, "page": 2,
                 "t_start": None, "t_end": None}]
        chunks = make_chunks(segs, size=400, overlap=0)
        self.assertTrue(chunks)
        for c in chunks:
            self.assertIn(c["page"], (1, 2))
            if "договор" in c["text"]:
                self.assertEqual(c["page"], 2)
            else:
                self.assertEqual(c["page"], 1)

    def test_head_path_in_chunks(self):
        """Фаза 2: путь заголовков md попадает в начало каждого чанка секции."""
        segs = [{"text": "Первый абзац раздела. " * 30, "page": None,
                 "t_start": None, "t_end": None, "head": "# Руководство / ## Настройки"}]
        chunks = make_chunks(segs, size=300, overlap=50)
        self.assertGreater(len(chunks), 1)
        for c in chunks:
            self.assertTrue(c["text"].startswith("# Руководство / ## Настройки\n"),
                            "каждый чанк секции начинается с пути заголовков")

    def test_no_content_loss(self):
        """Склейка кусков без потерь: суммарный текст сохраняется (кроме пробелов
        на границах)."""
        text = "\n\n".join("Абзац номер %d с содержанием." % i for i in range(50))
        chunks = make_chunks([{"text": text, "page": None, "t_start": None,
                               "t_end": None}], size=200, overlap=0)
        joined = " ".join(c["text"] for c in chunks)
        for i in range(50):
            self.assertIn("Абзац номер %d с содержанием." % i, joined)


if __name__ == "__main__":
    unittest.main()