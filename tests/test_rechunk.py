"""Фаза 2: путь заголовков md/docx, бюджет токенов эмбеддинга, index --rechunk."""
import os
import struct
import sys
import tempfile
import unittest

from helpers import FakeEmbedder, write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import db as dbmod, indexer  # noqa: E402
from hds.config import db_abs_path, load  # noqa: E402
from hds.extractors import extract, extract_markdown  # noqa: E402


class MarkdownHeadsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-md-")
        write_config(self.tmp)

    def test_markdown_head_path(self):
        """РЕГРЕССИЯ: заголовки md раньше не попадали в текст чанка."""
        p = write_text(self.tmp, "doc.md",
                       "# Руководство\n\nВводный текст руководства.\n\n"
                       "## Настройки\n\nТекст про настройки приложения.\n")
        kind, segs = extract(p, load())
        self.assertEqual(kind, "text")
        self.assertEqual(segs[0]["head"], "# Руководство")
        self.assertEqual(segs[1]["head"], "# Руководство / ## Настройки")
        self.assertIn("настройки", segs[1]["text"].lower())

    def test_markdown_head_hierarchy(self):
        """Заголовок h2 после h3 закрывает вложенную ветку."""
        p = write_text(self.tmp, "doc2.md",
                       "# A\n\n### A1\n\nтекст а1\n\n## B\n\nтекст бэ\n")
        segs = extract_markdown(p, load())  # возвращает список сегментов
        self.assertEqual(segs[0]["head"], "# A / ### A1")
        self.assertEqual(segs[1]["head"], "# A / ## B")

    def test_txt_without_head(self):
        p = write_text(self.tmp, "plain.txt", "просто текст без заголовков")
        _k, segs = extract(p, load())
        self.assertNotIn("head", segs[0])


class TokenBudgetTests(unittest.TestCase):
    def test_short_text_untouched(self):
        t = "короткий текст"
        self.assertEqual(indexer.clip_for_embedding(t), t)

    def test_long_text_clipped(self):
        """РЕГРЕССИЯ: LM Studio молча усекает вход длиннее контекста — режем сами."""
        t = "строка текста про документы\n" * 20000  # ~540k символов
        cut = indexer.clip_for_embedding(t)
        self.assertLess(len(cut), len(t))
        limit = int((indexer.EMB_CONTEXT - 256) * indexer._CHARS_PER_TOKEN)
        self.assertLessEqual(len(cut), limit)

    def test_commit_uses_budget(self):
        """Чанк длиннее бюджета обрезается при эмбеддинге, но в chunks.text
        сохраняется полностью."""
        tmp = tempfile.mkdtemp(prefix="hds-tb-")
        write_config(tmp)
        conn = dbmod.connect(db_abs_path(load()), 8)
        self.addCleanup(conn.close)
        long_text = "предложение про договор. " * 2000  # ~50k символов
        chunks = [{"text": long_text, "page": 1, "t_start": None, "t_end": None}]
        fid = dbmod.upsert_file(conn, r"D:\x\big.txt", ".txt", "text", 10, 1.0, "hb")
        status = indexer._commit_file(conn, FakeEmbedder(8), load(), fid, chunks, "text")
        self.assertTrue(status.startswith("indexed"))
        row = conn.execute("SELECT text FROM chunks WHERE file_id=?", (fid,)).fetchone()
        self.assertEqual(row[0], long_text)


class RechunkTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-rc-")
        write_config(self.tmp)
        self.conn = dbmod.connect(db_abs_path(load()), 8)
        self.addCleanup(self.conn.close)

    def _indexed_file(self, path, text="x"):
        fid = dbmod.upsert_file(self.conn, path, ".txt", "text", 10, 1.0, "h-" + path)
        dbmod.finish_file(self.conn, fid, "indexed", chunks=1)
        return fid

    def test_rechunk_reprocesses_files(self):
        """index --rechunk: перечанковка без OCR/AV, статус indexed сохраняется."""
        p = write_text(self.tmp, "r.txt", "Текст для перечанковки. " * 100)
        fid = self._indexed_file(p)
        status = indexer.process_file(self.conn, FakeEmbedder(8), load(), p)[0]
        self.assertTrue(status.startswith("indexed"))
        old = self.conn.execute("SELECT chunk_count FROM files WHERE id=?", (fid,)).fetchone()
        counters = indexer.run_rechunk(self.conn, FakeEmbedder(8), load(), progress_sec=0)
        self.assertEqual(counters.get("indexed", 0), 1)
        new = self.conn.execute("SELECT status, chunk_count FROM files WHERE id=?",
                                (fid,)).fetchone()
        self.assertEqual(new["status"], "indexed")
        self.assertGreater(new["chunk_count"], 0)
        self.assertEqual(new["chunk_count"], old["chunk_count"])  # тот же чанкер, тот же текст

    def test_rechunk_skips_missing_files(self):
        self._indexed_file(r"D:\nowhere\ghost.txt")
        counters = indexer.run_rechunk(self.conn, FakeEmbedder(8), load(), progress_sec=0)
        self.assertEqual(counters.get("missing"), 1)

    def test_rechunk_disables_ocr_and_transcribe(self):
        captured = {}

        def fake_process(conn, emb, cfg, path, force=False, progress_cb=None):
            captured["ocr"] = (cfg.get("index") or {}).get("ocr")
            captured["transcribe"] = (cfg.get("index") or {}).get("transcribe")
            captured["force"] = force
            return "indexed(1 чанк)", "text"

        orig = indexer.process_file
        indexer.process_file = fake_process
        try:
            p = write_text(self.tmp, "a.txt", "текст")  # файл должен существовать
            self._indexed_file(p)
            indexer.run_rechunk(self.conn, FakeEmbedder(8), load(), progress_sec=0)
        finally:
            indexer.process_file = orig
        self.assertFalse(captured["ocr"])
        self.assertFalse(captured["transcribe"])
        self.assertTrue(captured["force"])


if __name__ == "__main__":
    unittest.main()