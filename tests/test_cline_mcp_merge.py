"""Тесты установщика MCP-настроек Cline (installers/cline_mcp_merge.py).

Проверяется форма записи disk-search — ровно то, что валидирует схема Cline
(@cline/core, общая для Cline Desktop, CLI и IDE): плоские поля `type`/`url`
(remote, `streamableHttp`) или `command`/`args` (stdio). Устаревшая обёртка
`"transport": {"type": "http"}` невалидна и приводит к отбрасыванию ВСЕГО файла
настроек: `Invalid MCP settings ... mcpServers.disk-search: Invalid input`.

Реальные настройки Cline не затрагиваются — работаем во временном каталоге.
"""
import argparse
import importlib.util
import json
import os
import sys
import tempfile
import unittest
from unittest import mock

PROJECT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, PROJECT)

MERGE_PY = os.path.join(PROJECT, "installers", "cline_mcp_merge.py")
URL = "http://127.0.0.1:8787/mcp"
PY = r"C:\proj\.venv\Scripts\python.exe"
MCP_START = r"C:\proj\mcp_start.py"


def _load_merge():
    spec = importlib.util.spec_from_file_location("cline_mcp_merge", MERGE_PY)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


mm = _load_merge()


def _args(mode="http", url=URL, python=PY, mcp_start=MCP_START):
    return argparse.Namespace(mode=mode, url=url, python=python,
                              mcp_start=mcp_start)


def _read(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


class EntryShapeTests(unittest.TestCase):
    """Форма записи, которую принимает схема Cline (плоские поля)."""

    def test_http_entry_is_flat_streamable(self):
        e = mm.build_entry(_args())
        self.assertNotIn("transport", e)          # обёртка невалидна для http
        self.assertEqual(e["type"], "streamableHttp")
        self.assertEqual(e["url"], URL)
        self.assertFalse(e["disabled"])
        self.assertEqual(e["timeout"], mm.TIMEOUT)
        self.assertEqual(sorted(e["autoApprove"]), sorted(mm.TOOLS))

    def test_stdio_entry_is_flat(self):
        e = mm.build_entry(_args(mode="stdio"))
        self.assertNotIn("transport", e)
        self.assertNotIn("type", e)               # тип выводится из command
        self.assertEqual(e["command"], PY)
        self.assertEqual(e["args"], [MCP_START])
        self.assertEqual(e["env"], {})

    def test_http_mode_requires_url(self):
        with self.assertRaises(SystemExit):
            mm.build_entry(_args(url=None))


class MergeTests(unittest.TestCase):
    """merge(): чужие серверы не теряются, запись идемпотентна."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = os.path.join(self.tmp.name, "cline_mcp_settings.json")
        self.other = {"command": "npx", "args": ["-y", "some-mcp@latest"]}

    def _seed(self, disk_search=None):
        data = {"mcpServers": {"other-mcp": self.other}}
        if disk_search is not None:
            data["mcpServers"]["disk-search"] = disk_search
        with open(self.path, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)

    def test_other_servers_preserved(self):
        self._seed()
        mm.merge(self.path, mm.build_entry(_args()))
        data = _read(self.path)
        self.assertEqual(data["mcpServers"]["other-mcp"], self.other)
        self.assertNotIn("transport", data["mcpServers"]["disk-search"])
        self.assertEqual(data["mcpServers"]["disk-search"]["type"],
                         "streamableHttp")

    def test_merge_replaces_broken_entry_and_is_idempotent(self):
        # именно такую запись писал установщик до исправления (баг http-режима)
        broken = {"disabled": False, "timeout": 300,
                  "transport": {"type": "http", "url": URL}}
        self._seed(disk_search=broken)
        entry = mm.build_entry(_args())
        mm.merge(self.path, entry)
        first = _read(self.path)
        mm.merge(self.path, entry)
        second = _read(self.path)
        self.assertEqual(first, second)
        self.assertEqual(second["mcpServers"]["disk-search"], entry)

    def test_check_accepts_written_entry(self):
        self._seed()
        args = _args()
        mm.merge(self.path, mm.build_entry(args))
        mm.check(self.path, args)                 # не должно бросать

    def test_check_rejects_legacy_transport_wrapper(self):
        self._seed(disk_search={"timeout": 300,
                                "transport": {"type": "http", "url": URL}})
        with self.assertRaises(AssertionError):
            mm.check(self.path, _args())


class MainCliTests(unittest.TestCase):
    """main(): сквозная запись по ключам командной строки, как из инсталлятора."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.p1 = os.path.join(self.tmp.name, "settings", "cline_mcp_settings.json")
        self.p2 = os.path.join(self.tmp.name, "mcp.json")
        for p in (self.p1, self.p2):
            os.makedirs(os.path.dirname(p), exist_ok=True)
            with open(p, "w", encoding="utf-8") as f:
                json.dump({"mcpServers": {"other-mcp": {"command": "npx"}}}, f)

    def _run(self, *argv):
        with mock.patch.object(sys, "argv", ["cline_mcp_merge.py"] + list(argv)):
            mm.main()

    def test_http_mode_writes_valid_entries_to_all_targets(self):
        self._run("--mode", "http", "--url", URL, "--targets", self.p1, self.p2)
        for p in (self.p1, self.p2):
            data = _read(p)
            entry = data["mcpServers"]["disk-search"]
            self.assertEqual(entry["type"], "streamableHttp", p)
            self.assertEqual(entry["url"], URL, p)
            self.assertNotIn("transport", entry, p)
            self.assertIn("other-mcp", data["mcpServers"], p)

    def test_stdio_mode_writes_command_entry(self):
        self._run("--mode", "stdio", "--python", PY, "--mcp-start", MCP_START,
                  "--targets", self.p1)
        entry = _read(self.p1)["mcpServers"]["disk-search"]
        self.assertEqual(entry["command"], PY)
        self.assertNotIn("transport", entry)


if __name__ == "__main__":
    unittest.main()
