"""РЕГРЕССИЯ: start_indexing (MCP) запускал фоновую индексацию дважды —
строка threading.Thread(target=job, daemon=True).start() была продублирована."""
import os
import sys
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import mcp_server  # noqa: E402


class StartIndexingSingleThreadTests(unittest.TestCase):
    def setUp(self):
        mcp_server._idx_state["running"] = False
        self.addCleanup(mcp_server._idx_state.update, running=False)

    def test_single_thread_started(self):
        started = []

        class FakeThread:
            def __init__(self, target=None, daemon=None):
                self.target = target

            def start(self):
                started.append(self.target)

        with mock.patch.object(mcp_server.threading, "Thread", FakeThread):
            res = mcp_server.start_indexing(full=False)

        self.assertEqual(len(started), 1,
                         "start_indexing должен запускать ровно один поток")
        self.assertIn("запущена", res)

    def test_running_flag_blocks_second_start(self):
        mcp_server._idx_state["running"] = True
        self.addCleanup(mcp_server._idx_state.update, running=False)
        res = mcp_server.start_indexing()
        self.assertIn("уже идёт", res)


class HealthBuildStampTests(unittest.TestCase):
    """РЕГРЕССИЯ: метка сборки считалась прямо в обработчике /health, то есть на
    каждый запрос — и всегда совпадала с текущими исходниками. Из-за этого
    hds.mcp_http не мог увидеть, что на порту работает СТАРЫЙ код, и
    `mcp-http restart-if-stale` не перезапускал сервер после обновления."""

    def _body(self):
        import asyncio
        import json

        resp = asyncio.run(mcp_server._health(None))
        return json.loads(resp.body.decode("utf-8"))

    def test_build_is_captured_once_at_import(self):
        with mock.patch.object(mcp_server, "build_stamp", lambda: 999.0):
            body = self._body()
        self.assertEqual(body["build"], mcp_server._BUILD_STAMP)
        self.assertNotEqual(body["build"], 999.0,
                            "метка сборки фиксируется при старте процесса, а не в обработчике")

    def test_health_marks_own_instance(self):
        body = self._body()
        self.assertEqual(body["app"], mcp_server.APP_NAME)
        self.assertTrue(body["version"])


if __name__ == "__main__":
    unittest.main()
