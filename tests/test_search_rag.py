"""Тесты поиска: FTS-запросы, сниппеты, гибрид, деградация без эмбеддингов, RAG-фолбэк."""
import os
import sys
import tempfile
import unittest
from unittest import mock

from helpers import FakeEmbedder, write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import db as dbmod, rag  # noqa: E402
from hds.config import db_abs_path, load  # noqa: E402
from hds.search import fts_query, make_snippet, search  # noqa: E402


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-srch-")
        write_config(self.tmp)
        self.conn = dbmod.connect(db_abs_path(load()), 8)
        self.addCleanup(self.conn.close)

    def _index_two(self):
        f1 = write_text(self.tmp, "a.txt", "В проекте ИРИС использовался 1С:Документооборот для согласования договоров")
        f2 = write_text(self.tmp, "b.txt", "Отчёт по складу: логистика и маршруты доставки")
        emb = FakeEmbedder(8)
        for p, txt in ((f1, "a"), (f2, "b")):
            fid = dbmod.upsert_file(self.conn, p, ".txt", "text", 100, 1.0, "hash-" + txt)
            cid = dbmod.add_chunk(self.conn, fid, 0, None, None, None,
                                  open(p, encoding="utf-8").read())
            vec = emb.embed_query(open(p, encoding="utf-8").read())
            dbmod.add_vector(self.conn, cid, __import__("struct").pack("<8f", *vec))
        return f1, f2


class SearchTests(Base):
    def test_fts_query_escaping(self):
        self.assertEqual(fts_query("тест"), '"тест"')
        self.assertEqual(fts_query("тест два слова"), '"тест" OR "два" OR "слова"')
        self.assertIsNone(fts_query(",,,"))

    def test_snippet_around_token(self):
        snip = make_snippet("длинный текст. Здесь 1С:Документооборот упоминается. Ещё текст.", "документооборот")
        self.assertIn("Документооборот", snip)

    def test_fts_search_finds(self):
        self._index_two()
        res = search(self.conn, None, load(), "ИРИС документооборот")
        self.assertTrue(res)
        self.assertTrue(res[0]["path"].endswith("a.txt"))

    def test_degraded_search_without_embeddings(self):
        """РЕГРЕССИЯ: при недоступных эмбеддингах поиск работает по ключевым словам."""
        self._index_two()
        res = search(self.conn, None, load(), "документооборот")
        self.assertEqual(len(res), 1)

    def test_hybrid_with_embeddings(self):
        self._index_two()
        res = search(self.conn, FakeEmbedder(8), load(), "1С:Документооборот", limit=5)
        self.assertTrue(res)
        self.assertTrue(all("score" in r for r in res))


class RagTests(Base):
    def test_ask_no_results(self):
        out = rag.ask(self.conn, None, load(), "несуществующий запрос xyzzy")
        self.assertIn("не найдено", out["answer"].lower())

    def test_ask_with_fake_model(self):
        self._index_two()
        fake_resp = mock.Mock()
        fake_resp.status_code = 200
        fake_resp.json.return_value = {"choices": [{"message": {
            "content": "Ответ: проект ИРИС [1]"}}]}
        fake_resp.raise_for_status = lambda: None
        with mock.patch("hds.rag.requests.post", return_value=fake_resp):
            out = rag.ask(self.conn, FakeEmbedder(8), load(), "1С:Документооборот")
        self.assertIn("ИРИС", out["answer"])
        self.assertTrue(out["sources"])


if __name__ == "__main__":
    unittest.main()