"""Тесты диагностики: контекст embedding-модели (тихое усечение входа)."""
import json
import os
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import diag  # noqa: E402


class EmbeddingContextDiagTests(unittest.TestCase):
    """LM Studio молча усекает вход длиннее загруженного контекста, поэтому
    `check` (и кнопка «Проверить» в UI) обязан показать заниженный контекст.
    Обоснование — замер: при ctx=512 около 45 % чанков проекта потеряли бы
    хвост (медиана 484 токена, p90 829, максимум 2114)."""

    def _run_with(self, ctx):
        import json as _json

        payload = _json.dumps([{"identifier": "text-embedding-bge-m3",
                                "contextLength": ctx}]).encode("utf-8")
        return mock.patch.object(diag.subprocess, "run",
                                 lambda *a, **k: mock.Mock(stdout=payload,
                                                           stderr=b"",
                                                           returncode=0))

    def test_context_reported(self):
        with self._run_with(8192):
            self.assertEqual(diag._loaded_embedding_context("lms"), 8192)

    def test_small_context_detected(self):
        from hds.config import EMB_CONTEXT

        with self._run_with(512):
            ctx = diag._loaded_embedding_context("lms")
        self.assertEqual(ctx, 512)
        self.assertLess(ctx, EMB_CONTEXT)

    def test_unknown_context_is_none(self):
        """lms без --json или ошибка запуска: контекст неизвестен — молчим."""
        with mock.patch.object(diag.subprocess, "run",
                               lambda *a, **k: mock.Mock(stdout=b"not json",
                                                         stderr=b"",
                                                         returncode=0)):
            self.assertIsNone(diag._loaded_embedding_context("lms"))
        with mock.patch.object(diag.subprocess, "run",
                               side_effect=OSError("lms not found")):
            self.assertIsNone(diag._loaded_embedding_context("lms"))

    def test_other_models_ignored(self):
        import json as _json

        payload = _json.dumps([{"identifier": "qwen3.5-9b@q6_k",
                                "contextLength": 60160}]).encode("utf-8")
        with mock.patch.object(diag.subprocess, "run",
                               lambda *a, **k: mock.Mock(stdout=payload,
                                                         stderr=b"",
                                                         returncode=0)):
            self.assertIsNone(diag._loaded_embedding_context("lms"))


if __name__ == "__main__":
    unittest.main()