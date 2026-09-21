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


if __name__ == "__main__":
    unittest.main()
