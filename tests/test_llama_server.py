"""Тесты менеджера llama-server (hds.llama_server): конфиг ролей, команды
запуска, probe-состояния — сеть и процессы замоканы."""
import os
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import llama_server as ls  # noqa: E402


def _cfg(**over):
    cfg = {
        "llm_server": {
            "bin": "fake-llama-server",
            "host": "127.0.0.1",
            "start_timeout": 2,
            "parallel": 1,
        },
        "chat": {"model": "qwen3.5-9b"},
        "embedding": {"model": "text-embedding-bge-m3"},
        "rerank": {"model": "bge-reranker-v2-m3"},
    }
    cfg["llm_server"].update(over.pop("llm", {}))
    return cfg


class FakeResp:
    def __init__(self, status=200, body=b"{}"):
        self.status = status
        self._body = body

    def read(self):
        return self._body

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class RoleConfigTests(unittest.TestCase):
    def test_defaults(self):
        cfg = {}
        chat = ls._role_cfg(cfg, "chat")
        self.assertEqual(chat["port"], 8010)
        self.assertEqual(chat["ctx_per_slot"], 16384)
        emb = ls._role_cfg(cfg, "embedding")
        self.assertEqual(emb["port"], 8011)
        self.assertEqual(emb["ctx_per_slot"], 8192)
        rr = ls._role_cfg(cfg, "rerank")
        self.assertEqual(rr["port"], 8012)

    def test_override_from_config(self):
        cfg = {"llm_server": {"chat": {"port": 9000, "ctx_per_slot": 8192}}}
        self.assertEqual(ls._role_cfg(cfg, "chat")["port"], 9000)
        self.assertEqual(ls._role_cfg(cfg, "chat")["ctx_per_slot"], 8192)
        self.assertEqual(ls._role_cfg(cfg, "embedding")["port"], 8011)

    def test_base_urls(self):
        self.assertEqual(ls.base_url(_cfg(), "chat"), "http://127.0.0.1:8010")
        self.assertEqual(ls.api_url(_cfg(), "embedding"),
                         "http://127.0.0.1:8011/v1")


class BuildCommandTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-llama-")
        self.gguf = os.path.join(self.tmp, "fake.gguf")
        open(self.gguf, "wb").close()

    def _cfg(self):
        cfg = _cfg()
        cfg["llm_server"]["chat"] = {
            "port": 8010, "model": self.gguf, "ctx_per_slot": 16384,
            "extra_args": "--cache-type-k q8_0"}
        cfg["llm_server"]["embedding"] = {
            "port": 8011, "model": self.gguf, "ctx_per_slot": 8192,
            "extra_args": ""}
        cfg["llm_server"]["rerank"] = {
            "port": 8012, "model": self.gguf, "ctx_per_slot": 8192,
            "extra_args": "-ngl 0"}
        return cfg

    def test_chat_flags(self):
        cmd = ls.build_command(self._cfg(), "chat")
        joined = " ".join(cmd)
        self.assertIn("-m", cmd)
        self.assertIn("--port", cmd)
        self.assertIn("8010", cmd)
        self.assertIn("--parallel", cmd)
        self.assertIn("1", cmd)
        self.assertIn("--ctx-size", cmd)
        self.assertIn("16384", cmd)
        self.assertIn("--no-webui", joined)
        self.assertIn("--alias qwen3.5-9b", joined)
        self.assertNotIn("--jinja", joined)  # tool-calling остаётся у агента
        self.assertNotIn("--embedding", joined)
        self.assertIn("--cache-type-k", joined)

    def test_embedding_mode_flags(self):
        cmd = ls.build_command(self._cfg(), "embedding")
        joined = " ".join(cmd)
        self.assertIn("--embedding", joined)
        self.assertIn("--pooling cls", joined)
        self.assertIn("8011", cmd)
        self.assertNotIn("--reranking", joined)
        self.assertIn("--alias text-embedding-bge-m3", joined)

    def test_rerank_mode_flags(self):
        cmd = ls.build_command(self._cfg(), "rerank")
        joined = " ".join(cmd)
        self.assertIn("--reranking", joined)
        self.assertIn("--pooling rank", joined)
        self.assertIn("-ngl 0", joined)

    def test_missing_binary_raises(self):
        cfg = self._cfg()
        cfg["llm_server"]["bin"] = ""
        with mock.patch.object(ls.shutil, "which", lambda _n: None), \
                mock.patch.object(ls.os.path, "isfile", lambda _p: False):
            with self.assertRaises(RuntimeError):
                ls.build_command(cfg, "chat")

    def test_missing_model_raises(self):
        cfg = self._cfg()
        cfg["llm_server"]["embedding"] = dict(
            cfg["llm_server"]["embedding"], model="models/none/missing.gguf")
        with self.assertRaises(RuntimeError):
            ls.build_command(cfg, "embedding")

    def test_ctx_is_parallel_times_slot(self):
        cfg = self._cfg()
        cfg["llm_server"]["parallel"] = 2
        cmd = ls.build_command(cfg, "embedding")
        i = cmd.index("--ctx-size")
        self.assertEqual(cmd[i + 1], str(2 * 8192))


class ProbeTests(unittest.TestCase):
    def test_down(self):
        with mock.patch.object(ls.urllib.request, "urlopen",
                               side_effect=OSError("refused")):
            det = ls.probe(_cfg(), "chat")
        self.assertEqual(det["state"], ls.STATE_DOWN)

    def test_props_not_llama_is_foreign(self):
        def urlopen(url, **kw):
            if getattr(url, "full_url", url).endswith("/health"):
                return FakeResp()
            return FakeResp(body=b"{}")
        with mock.patch.object(ls.urllib.request, "urlopen", urlopen):
            self.assertEqual(ls.probe(_cfg(), "chat")["state"],
                             ls.STATE_FOREIGN)

    def test_llama_with_expected_model(self):
        props = {"total_slots": 1,
                 "model_path": os.path.normcase(os.path.normpath(
                     os.path.join(os.getcwd(), "x", "fake.gguf")))}
        def urlopen(url, **kw):
            if getattr(url, "full_url", url).endswith("/health"):
                return FakeResp()
            import json as _json
            return FakeResp(body=_json.dumps(props).encode("utf-8"))
        with mock.patch.object(ls, "_abs_model",
                               lambda c, r: props["model_path"]), \
                mock.patch.object(ls.urllib.request, "urlopen", urlopen):
            det = ls.probe(_cfg(), "chat")
        self.assertEqual(det["state"], ls.STATE_LLAMA)
        self.assertEqual(det["total_slots"], 1)

    def test_foreign_llama_with_other_model(self):
        props = {"total_slots": 1, "model_path": "D:/other/model.gguf"}
        def urlopen(url, **kw):
            if getattr(url, "full_url", url).endswith("/health"):
                return FakeResp()
            import json as _json
            return FakeResp(body=_json.dumps(props).encode("utf-8"))
        with mock.patch.object(ls, "_abs_model", lambda c, r: "C:\\ours\\m.gguf"), \
                mock.patch.object(ls.urllib.request, "urlopen", urlopen):
            self.assertEqual(ls.probe(_cfg(), "chat")["state"],
                             ls.STATE_FOREIGN)

    def test_props_context(self):
        self.assertEqual(ls.props_context({"default_generation_settings":
                                           {"n_ctx": 8192}}), 8192)
        self.assertIsNone(ls.props_context({}))


if __name__ == "__main__":
    unittest.main()