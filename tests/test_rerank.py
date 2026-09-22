"""Фаза 4: клиент реранкера — переупорядочивание на моках, деградация при
недоступности, авто-отключение при превышении лимита латентности, интеграция в rag."""
import os
import sys
import tempfile
import time
import unittest
from unittest import mock

from helpers import FakeEmbedder, write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import db as dbmod, rag, rerank  # noqa: E402
from hds.config import db_abs_path, load  # noqa: E402

DOCS = [{"text": "alpha"}, {"text": "beta"}, {"text": "gamma"}]


def _fake_resp(results):
    m = mock.Mock()
    m.status_code = 200
    m.raise_for_status = lambda: None
    m.json.return_value = {"results": results}
    return m


class RerankTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-rr-")
        write_config(self.tmp)
        rerank._disabled = False

    def tearDown(self):
        rerank._disabled = False

    def test_reorders_by_relevance(self):
        resp = _fake_resp([{"index": 2, "relevance_score": 0.9},
                           {"index": 0, "relevance_score": 0.4},
                           {"index": 1, "relevance_score": 0.1}])
        with mock.patch("hds.rerank.requests.post", return_value=resp) as mp:
            out = rerank.rerank_results(load(), "вопрос", DOCS, top_n=2)
        self.assertEqual([d["text"] for d in out], ["gamma", "alpha"])
        payload = mp.call_args.kwargs["json"]
        self.assertEqual(payload["query"], "вопрос")
        self.assertEqual(payload["documents"], ["alpha", "beta", "gamma"])

    def test_unavailable_degrades(self):
        """РЕГРЕССИЯ: недоступный реранкер не ломает ask — результаты без реранкинга."""
        with mock.patch("hds.rerank.requests.post", side_effect=RuntimeError("refused")):
            out = rerank.rerank_results(load(), "q", DOCS, top_n=2)
        self.assertIsNone(out)
        self.assertFalse(rerank._disabled)  # недоступность != превышение латентности

    def test_latency_auto_disable(self):
        """РЕГРЕССИЯ: латентность выше лимита — реранкер отключается на сессию,
        повторные запросы к нему не отправляются."""
        def slow_post(*a, **k):
            time.sleep(0.03)
            return _fake_resp([{"index": 0, "relevance_score": 1.0}])

        write_config(self.tmp, extra="\nrerank:\n  enabled: true\n  max_latency: 0.001\n")
        with mock.patch("hds.rerank.requests.post", side_effect=slow_post) as mp:
            self.assertIsNone(rerank.rerank_results(load(), "q", DOCS, top_n=2))
            self.assertTrue(rerank._disabled)
            self.assertIsNone(rerank.rerank_results(load(), "q", DOCS, top_n=2))
        self.assertEqual(len(mp.call_args_list), 1)


class RagRerankTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-rrag-")
        write_config(self.tmp)
        rerank._disabled = False
        self.conn = dbmod.connect(db_abs_path(load()), 8)
        self.addCleanup(self.conn.close)

    def _index(self, name, text):
        p = write_text(self.tmp, name, text)
        fid = dbmod.upsert_file(self.conn, p, ".txt", "text", 10, 1.0, "h-" + name)
        dbmod.add_chunk(self.conn, fid, 0, None, None, None, text)
        dbmod.add_vector(self.conn, fid, __import__("struct").pack(
            "<8f", *FakeEmbedder(8).embed_query(text)))
        return p

    def test_ask_uses_reranker(self):
        """РЕГРЕССИЯ: при rerank.enabled ask ищет пул кандидатов и передаёт их
        реранкеру; порядок источников берётся из ответа реранкера (индексы —
        позиции в отправленном пуле)."""
        write_config(self.tmp, extra="\nrerank:\n  enabled: true\n")
        texts = {"a.txt": "Документ про настройки приложения",
                 "b.txt": "Документ про склад и логистику доставки"}
        path_by_text = {}
        for name, t in texts.items():
            path_by_text[t] = self._index(name, t)
        chat = mock.Mock()
        chat.status_code = 200
        chat.raise_for_status = lambda: None
        chat.json.return_value = {"choices": [{"message": {"content": "Ответ [1]"}}]}
        docs_seen = []

        def fake_post(url, json=None, **kw):
            if str(url).endswith("/rerank"):
                docs_seen.extend(json["documents"])
                return _fake_resp([{"index": 1, "relevance_score": 0.9},
                                   {"index": 0, "relevance_score": 0.1}])
            return chat  # chat-completions

        with mock.patch("hds.rerank.requests.post", side_effect=fake_post):
            out = rag.ask(self.conn, FakeEmbedder(8), load(), "склад")
        self.assertEqual(sorted(docs_seen), sorted(path_by_text))
        # реранкер ставит кандидата с индексом 1 первым
        self.assertEqual([s["path"] for s in out["sources"]][:2],
                         [path_by_text[docs_seen[1]], path_by_text[docs_seen[0]]])

    def test_ask_without_reranker_flag(self):
        """По умолчанию rerank выключен — пул запроса не расширяется."""
        p1 = self._index("a.txt", "Документ про настройки приложения")
        chat = mock.Mock()
        chat.status_code = 200
        chat.raise_for_status = lambda: None
        chat.json.return_value = {"choices": [{"message": {"content": "Ответ [1]"}}]}
        with mock.patch("hds.rerank.requests.post") as mp, \
             mock.patch("hds.rag.requests.post", return_value=chat):
            out = rag.ask(self.conn, FakeEmbedder(8), load(), "настройки")
        self.assertFalse(mp.called)
        self.assertTrue(out["sources"][0]["path"].endswith("a.txt"))


def payload_docs(mock_call):
    return mock_call.call_args.kwargs["json"]["documents"]


if __name__ == "__main__":
    unittest.main()