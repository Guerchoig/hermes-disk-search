"""Тесты менеджера общего MCP-сервера (hds.mcp_http): конфиг, URL, команда
запуска, probe-состояния, переиспользование живого инстанса — сеть и процессы
замоканы. Живая проверка (поднять инстанс и получить список инструментов
настоящим MCP-клиентом) выполняется вручную при приёмке релиза.
"""
import os
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import mcp_http as mh  # noqa: E402


def _cfg(**over):
    cfg = {"mcp_http": {"host": "127.0.0.1", "port": 8787, "path": "/mcp",
                        "autostart": True, "start_timeout": 2}}
    cfg["mcp_http"].update(over)
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


class ConfigTests(unittest.TestCase):
    def test_defaults(self):
        cfg = {}
        self.assertEqual(mh.host(cfg), "127.0.0.1")
        self.assertEqual(mh.port(cfg), 8787)
        self.assertEqual(mh.path(cfg), "/mcp")
        self.assertTrue(mh.autostart_on(cfg))
        self.assertEqual(mh.url(cfg), "http://127.0.0.1:8787/mcp")

    def test_override(self):
        cfg = _cfg(port=9001, path="mcp", autostart=False)
        self.assertEqual(mh.port(cfg), 9001)
        self.assertEqual(mh.path(cfg), "/mcp")  # ведущий слэш добавляется
        self.assertEqual(mh.url(cfg), "http://127.0.0.1:9001/mcp")
        self.assertFalse(mh.autostart_on(cfg))

    def test_with_overrides_cli(self):
        base = _cfg()
        cfg = mh._with_overrides(base, "0.0.0.0", 9999, "/x")
        self.assertEqual(mh.url(cfg), "http://0.0.0.0:9999/x")
        self.assertEqual(mh.url(base), "http://127.0.0.1:8787/mcp")  # исходный цел


class BuildCommandTests(unittest.TestCase):
    def test_command_shape(self):
        joined = " ".join(mh.build_command(_cfg(port=8899)))
        self.assertIn("-m hds.mcp_http run", joined)
        self.assertIn("--host 127.0.0.1", joined)
        self.assertIn("--port 8899", joined)
        self.assertIn("--path /mcp", joined)

    def test_python_prefers_venv(self):
        with mock.patch.object(mh.os.path, "isfile", lambda p: True):
            self.assertIn(".venv", mh._python())

    def test_python_is_pythonw_on_windows(self):
        """РЕГРЕССИЯ: python.exe давал видимое окно Windows Terminal — нужен pythonw."""
        with mock.patch.object(mh.sys, "platform", "win32"), \
                mock.patch.object(mh.os.path, "isfile", lambda p: True):
            self.assertTrue(mh._python().lower().endswith("pythonw.exe"),
                            "сервер должен стартовать без консоли (pythonw)")


class ProbeTests(unittest.TestCase):
    def test_down(self):
        with mock.patch.object(mh.urllib.request, "urlopen",
                               side_effect=OSError("refused")), \
                mock.patch.object(mh, "_live_pid", lambda: None):
            det = mh.probe(_cfg())
        self.assertEqual(det["state"], mh.STATE_DOWN)

    def test_ours(self):
        import json as _json
        body = _json.dumps({"app": "disk-search", "version": "0.10.0"}).encode()
        with mock.patch.object(mh.urllib.request, "urlopen",
                               lambda *a, **kw: FakeResp(body=body)), \
                mock.patch.object(mh, "_live_pid", lambda: 4242):
            det = mh.probe(_cfg())
        self.assertEqual(det["state"], mh.STATE_MCP)
        self.assertEqual(det["pid"], 4242)
        self.assertEqual(det["info"]["version"], "0.10.0")

    def test_foreign_json(self):
        with mock.patch.object(mh.urllib.request, "urlopen",
                               lambda *a, **kw: FakeResp(body=b'{"hello": 1}')):
            self.assertEqual(mh.probe(_cfg())["state"], mh.STATE_FOREIGN)

    def test_foreign_not_json(self):
        with mock.patch.object(mh.urllib.request, "urlopen",
                               lambda *a, **kw: FakeResp(body=b"<html>")):
            self.assertEqual(mh.probe(_cfg())["state"], mh.STATE_FOREIGN)

    def test_http_error_is_foreign(self):
        import io

        def boom(*a, **kw):
            raise mh.urllib.error.HTTPError("u", 404, "nf", None, io.BytesIO(b""))

        with mock.patch.object(mh.urllib.request, "urlopen", boom):
            self.assertEqual(mh.probe(_cfg())["state"], mh.STATE_FOREIGN)


class StartStopTests(unittest.TestCase):
    def test_reuses_live_instance(self):
        """Ключевое свойство: живой инстанс → второго процесса НЕ появляется."""
        det_ok = {"state": mh.STATE_MCP, "info": {}, "pid": 111}
        with mock.patch.object(mh, "probe", lambda cfg=None, timeout=mh._PROBE_TIMEOUT: det_ok), \
                mock.patch.object(mh.subprocess, "Popen") as popen:
            info = mh.start(_cfg())
        self.assertTrue(info["reused"])
        self.assertFalse(info["started"])
        self.assertEqual(info["pid"], 111)
        popen.assert_not_called()

    def test_foreign_port_raises(self):
        det = {"state": mh.STATE_FOREIGN, "info": {}, "pid": None}
        with mock.patch.object(mh, "probe", lambda cfg=None, timeout=mh._PROBE_TIMEOUT: det):
            with self.assertRaises(RuntimeError):
                mh.start(_cfg())

    def test_start_writes_pid_and_waits(self):
        proc = mock.Mock()
        proc.pid = 777
        tmp = tempfile.mkdtemp(prefix="hds-mcp-")
        pidf = os.path.join(tmp, "mcp.pid")
        down = {"state": mh.STATE_DOWN, "info": {}, "pid": None}
        calls = []

        def fake_popen(*a, **kw):
            calls.append((a, kw))
            return proc

        with mock.patch.object(mh, "probe", lambda cfg=None, timeout=mh._PROBE_TIMEOUT: down), \
                mock.patch.object(mh, "_wait_ready",
                                  lambda cfg, p: {"state": mh.STATE_MCP}), \
                mock.patch.object(mh, "_pid_file", lambda: pidf), \
                mock.patch.object(mh, "_log_file",
                                  lambda: os.path.join(tmp, "mcp.log")), \
                mock.patch.object(mh.subprocess, "Popen", fake_popen):
            info = mh.start(_cfg())
        self.assertTrue(info["started"])
        self.assertFalse(info["reused"])
        self.assertEqual(info["pid"], 777)
        self.assertEqual(len(calls), 1, "должен стартовать ровно один процесс")
        with open(pidf, encoding="utf-8") as f:
            self.assertEqual(f.read(), "777")

    def test_wait_ready_raises_when_process_died(self):
        proc = mock.Mock()
        proc.poll.return_value = 1
        proc.returncode = 1
        with mock.patch.object(mh, "_log_tail", lambda lines=15: "boom"):
            with self.assertRaises(RuntimeError) as cm:
                mh._wait_ready(_cfg(), proc)
        self.assertIn("boom", str(cm.exception))

    def test_stop_uses_pid_file(self):
        tmp = tempfile.mkdtemp(prefix="hds-mcp-")
        pidf = os.path.join(tmp, "mcp.pid")
        with open(pidf, "w", encoding="utf-8") as f:
            f.write("555")
        with mock.patch.object(mh, "_pid_file", lambda: pidf), \
                mock.patch.object(mh, "_kill_pid") as kill:
            self.assertTrue(mh.stop(_cfg()))
            kill.assert_called_once_with(555)
        self.assertFalse(os.path.exists(pidf))

    def test_stop_without_pid_file(self):
        """Без PID-файла и без НАШЕГО /health на порту — останавливать нечего."""
        down = {"state": mh.STATE_DOWN, "info": {}, "pid": None}
        with mock.patch.object(mh, "_pid_file",
                               lambda: os.path.join(tempfile.mkdtemp(), "nope.pid")), \
                mock.patch.object(mh, "probe", lambda cfg=None, timeout=mh._PROBE_TIMEOUT: down):
            self.assertFalse(mh.stop(_cfg()))

    def test_stop_by_port_when_no_pid_file(self):
        """РЕГРЕССИЯ: инстанс автозапуска ОС (`mcp-http run`) PID-файла не пишет —
        раньше restart молча ничего не перезапускал, порт оставался у старого
        процесса. Теперь владельца порта находим, если на нём отвечает наш /health."""
        ours = {"state": mh.STATE_MCP, "info": {}, "pid": None}
        port_mock = mock.Mock(return_value=[4321])
        with mock.patch.object(mh, "_pid_file",
                               lambda: os.path.join(tempfile.mkdtemp(), "nope.pid")), \
                mock.patch.object(mh, "probe", lambda cfg=None, timeout=mh._PROBE_TIMEOUT: ours), \
                mock.patch.object(mh, "port_pids", port_mock), \
                mock.patch.object(mh, "_kill_pid") as kill:
            self.assertTrue(mh.stop(_cfg()))
        port_mock.assert_called_once_with(8787)
        kill.assert_called_once_with(4321)

    def test_stop_by_port_skips_foreign_service(self):
        """Чужой сервис на порту убивать нельзя — даже без PID-файла."""
        foreign = {"state": mh.STATE_FOREIGN, "info": {}, "pid": None}
        with mock.patch.object(mh, "_pid_file",
                               lambda: os.path.join(tempfile.mkdtemp(), "nope.pid")), \
                mock.patch.object(mh, "probe", lambda cfg=None, timeout=mh._PROBE_TIMEOUT: foreign), \
                mock.patch.object(mh, "port_pids") as pids, \
                mock.patch.object(mh, "_kill_pid") as kill:
            self.assertFalse(mh.stop(_cfg()))
        pids.assert_not_called()
        kill.assert_not_called()

    def test_status_shape(self):
        det = {"state": mh.STATE_MCP, "info": {"version": "1.2.3"}, "pid": 9}
        with mock.patch.object(mh, "probe", lambda cfg=None, timeout=mh._PROBE_TIMEOUT: det):
            st = mh.status(_cfg(), scan_stdio=False)
        self.assertTrue(st["running"])
        self.assertEqual(st["version"], "1.2.3")
        self.assertEqual(st["url"], "http://127.0.0.1:8787/mcp")
        self.assertNotIn("stdio_processes", st)


class PortPidsTests(unittest.TestCase):
    """Владелец порта — запасной путь для инстанса без PID-файла."""

    def test_windows_parses_output(self):
        res = mock.Mock(stdout="987\n654\n", returncode=0)
        with mock.patch.object(mh.sys, "platform", "win32"), \
                mock.patch.object(mh.subprocess, "run", lambda *a, **kw: res), \
                mock.patch.object(mh.os, "getpid", lambda: 654):
            self.assertEqual(mh.port_pids(8787), [987])

    def test_posix_uses_lsof(self):
        calls = []
        res = mock.Mock(stdout="1234\n", returncode=0)
        with mock.patch.object(mh.sys, "platform", "darwin"), \
                mock.patch.object(mh.subprocess, "run",
                                  lambda *a, **kw: calls.append(a[0]) or res):
            self.assertEqual(mh.port_pids(8787), [1234])
        self.assertEqual(calls[0][0], "lsof")

    def test_survives_errors(self):
        with mock.patch.object(mh.subprocess, "run", side_effect=OSError("no tools")):
            self.assertEqual(mh.port_pids(8787), [])


class StalenessTests(unittest.TestCase):
    """`mcp-http restart-if-stale` — обновление проекта (клиенты ходят по URL)."""

    def _det(self, version, build=None, pid=5):
        info = {"app": "disk-search", "version": version}
        if build is not None:
            info["build"] = build
        return {"state": mh.STATE_MCP, "info": info, "pid": pid}

    def _probe(self, det):
        return mock.patch.object(mh, "probe",
                                 lambda cfg=None, timeout=mh._PROBE_TIMEOUT: det)

    def test_fresh_when_version_and_build_match(self):
        det = self._det(mh.__version__, build=mh.build_stamp())
        with self._probe(det):
            st = mh.staleness(_cfg())
        self.assertTrue(st["running"])
        self.assertFalse(st["stale"])
        self.assertEqual(st["reason"], "")

    def test_stale_on_version_mismatch(self):
        with self._probe(self._det("0.0.1")):
            st = mh.staleness(_cfg())
        self.assertTrue(st["stale"])
        self.assertIn("0.0.1", st["reason"])

    def test_stale_on_fresh_sources(self):
        """Правки без поднятия версии тоже видны — по метке сборки (mtime)."""
        det = self._det(mh.__version__, build=mh.build_stamp() - 3600)
        with self._probe(det):
            st = mh.staleness(_cfg())
        self.assertTrue(st["stale"])
        self.assertIn("исходники", st["reason"])

    def test_stale_when_build_stamp_missing(self):
        """Инстанс старше этого механизма (нет метки сборки) — тоже старый код."""
        with self._probe(self._det(mh.__version__)):
            st = mh.staleness(_cfg())
        self.assertTrue(st["stale"])
        self.assertIn("метку сборки", st["reason"])

    def test_not_stale_when_down(self):
        down = {"state": mh.STATE_DOWN, "info": {}, "pid": None}
        with self._probe(down):
            st = mh.staleness(_cfg())
        self.assertFalse(st["stale"])
        self.assertFalse(st["running"])

    def test_status_reports_staleness(self):
        with self._probe(self._det("0.0.1")):
            st = mh.status(_cfg(), scan_stdio=False)
        self.assertEqual(st["code_version"], mh.__version__)
        self.assertTrue(st["stale"])
        self.assertTrue(st["stale_reason"])

    def test_restart_if_stale_reuses_fresh(self):
        det = self._det(mh.__version__, build=mh.build_stamp())
        with self._probe(det), \
                mock.patch.object(mh, "start") as start_, \
                mock.patch.object(mh, "stop") as stop_:
            info = mh.restart_if_stale(_cfg())
        self.assertEqual(info["action"], "reused")
        start_.assert_not_called()
        stop_.assert_not_called()

    def test_restart_if_stale_starts_when_down(self):
        down = {"state": mh.STATE_DOWN, "info": {}, "pid": None}
        with self._probe(down), \
                mock.patch.object(mh, "start",
                                  lambda cfg, wait=True: {"started": True, "pid": 1}):
            info = mh.restart_if_stale(_cfg())
        self.assertEqual(info["action"], "started")

    def test_restart_if_stale_restarts_when_stale(self):
        with self._probe(self._det("0.0.1")), \
                mock.patch.object(mh, "stop") as stop_, \
                mock.patch.object(mh, "wait_down", lambda cfg, timeout=10.0: True), \
                mock.patch.object(mh, "start",
                                  lambda cfg, wait=True: {"started": True, "pid": 2}):
            info = mh.restart_if_stale(_cfg())
        self.assertEqual(info["action"], "restarted")
        stop_.assert_called_once()

    def test_restart_if_stale_reports_unfreed_port(self):
        """Порт не освободился — не притворяемся, что обновление применено."""
        with self._probe(self._det("0.0.1")), \
                mock.patch.object(mh, "stop"), \
                mock.patch.object(mh, "wait_down", lambda cfg, timeout=10.0: False), \
                mock.patch.object(mh, "start") as start_:
            info = mh.restart_if_stale(_cfg())
        self.assertIn("error", info)
        start_.assert_not_called()


class StdioCleanupTests(unittest.TestCase):
    def test_stdio_pids_parses_output(self):
        res = mock.Mock(stdout="111\n222\n333\n", returncode=0)
        with mock.patch.object(mh.subprocess, "run", lambda *a, **kw: res), \
                mock.patch.object(mh.os, "getpid", lambda: 222):
            self.assertEqual(mh.stdio_pids(), [111, 333])

    def test_stdio_pids_survives_errors(self):
        with mock.patch.object(mh.subprocess, "run", side_effect=OSError("no ps")), \
                mock.patch.object(mh.sys, "platform", "linux"):
            self.assertEqual(mh.stdio_pids(), [])

    def test_stop_stdio_kills_found(self):
        with mock.patch.object(mh, "stdio_pids", lambda exclude=(): [1, 2]), \
                mock.patch.object(mh, "_kill_pid") as kill:
            self.assertEqual(mh.stop_stdio(), [1, 2])
        self.assertEqual(kill.call_count, 2)


class LogRedirectTests(unittest.TestCase):
    """Автозапуск ОС идёт через pythonw: stdout=None → вывод должен идти в лог."""

    def test_redirect_when_no_console(self):
        tmp = tempfile.mkdtemp(prefix="hds-mcp-log-")
        logfile = os.path.join(tmp, "mcp.log")
        with mock.patch.object(mh, "_log_file", lambda: logfile), \
                mock.patch.object(mh.sys, "stdout", None), \
                mock.patch.object(mh.sys, "stderr", None):
            mh._redirect_to_log()
            opened = mh.sys.stdout
            self.assertIsNotNone(opened)
            print("строка в лог")
            print("ошибка в лог", file=mh.sys.stderr)
            opened.close()
        text = open(logfile, encoding="utf-8").read()
        self.assertIn("строка в лог", text)
        self.assertIn("ошибка в лог", text)

    def test_no_redirect_when_console_present(self):
        before = mh.sys.stdout
        with mock.patch.object(mh, "_log_file", lambda: os.devnull):
            mh._redirect_to_log()
        self.assertIs(before, mh.sys.stdout)


class McpServerRunTests(unittest.TestCase):
    """run() прокидывает транспорт в mcp.run: stdio по умолчанию, http — по URL."""

    def _patched(self):
        from hds import mcp_server
        return mcp_server, mock.patch.object(mcp_server, "load",
                                             lambda: {"llm_server": {"autostart": False}})

    def test_stdio_default(self):
        mcp_server, load_patch = self._patched()
        with load_patch, mock.patch.object(mcp_server.mcp, "run") as run:
            mcp_server.run()
        run.assert_called_once_with(transport="stdio")

    def test_streamable_http_kwargs(self):
        mcp_server, load_patch = self._patched()
        with load_patch, mock.patch.object(mcp_server.mcp, "run") as run:
            mcp_server.run(transport="streamable-http", host="127.0.0.1",
                           port=8787, path="/mcp")
        run.assert_called_once_with(transport="streamable-http",
                                    host="127.0.0.1", port=8787,
                                    streamable_http_path="/mcp")

    def test_health_route_registered(self):
        """Кастомный маршрут /health — по нему probe опознаёт СВОЙ инстанс."""
        from hds import mcp_server
        paths = []
        for r in mcp_server.mcp._custom_starlette_routes:
            paths.append(getattr(r, "path", ""))
        self.assertIn("/health", paths)


if __name__ == "__main__":
    unittest.main()
