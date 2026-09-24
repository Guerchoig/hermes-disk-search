"""Тесты диагностики: контекст embedding-инстанса llama-server (/props)."""
import os
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import diag  # noqa: E402
from hds import llama_server as ls  # noqa: E402


class EmbeddingContextDiagTests(unittest.TestCase):
    """При контексте меньше EMB_CONTEXT длинные чанки теряют хвост, поэтому
    `check` (и кнопка «Проверить» в UI) обязан показать заниженный контекст.
    Обоснование — замер: при ctx=512 около 45 % чанков проекта потеряли бы
    хвост (медиана 484 токена, p90 829, максимум 2114)."""

    def _props(self, ctx):
        return {"total_slots": 1,
                "default_generation_settings": {"n_ctx": ctx}}

    def _patch(self, ctx):
        """probe роли embedding: живой llama с контекстом ctx."""
        det = {"state": ls.STATE_LLAMA, "total_slots": 1,
               "props": self._props(ctx)}
        return mock.patch.object(ls, "probe", lambda cfg, role, **kw: det)

    def test_context_reported(self):
        self.assertEqual(ls.props_context(self._props(8192)), 8192)

    def test_small_context_warns(self):
        import tempfile
        from helpers import write_config  # noqa: I100

        tmp = tempfile.mkdtemp(prefix="hds-diag-ctx-")
        cfg_path = write_config(tmp)
        self.addCleanup(lambda: os.path.exists(cfg_path)
                        and os.remove(cfg_path))
        from hds.config import EMB_CONTEXT, load
        self.assertLess(512, EMB_CONTEXT)
        with self._patch(512):
            checks = diag.run_checks(load())
        embctx = [c for c in checks if c["id"] == "embctx"]
        self.assertTrue(embctx, "заниженный контекст обязан попадать в check")
        self.assertEqual(embctx[0]["status"], "warn")
        self.assertIn("512", embctx[0]["title"])

    def test_correct_context_no_warn(self):
        import tempfile
        from helpers import write_config  # noqa: I100

        tmp = tempfile.mkdtemp(prefix="hds-diag-ctx2-")
        cfg_path = write_config(tmp)
        self.addCleanup(lambda: os.path.exists(cfg_path)
                        and os.remove(cfg_path))
        from hds.config import load
        with self._patch(8192):
            checks = diag.run_checks(load())
        self.assertFalse([c for c in checks if c["id"] == "embctx"])


if __name__ == "__main__":
    unittest.main()