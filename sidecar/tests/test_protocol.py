"""Протокол common-пакета: NDJSON, ошибки, изоляция одного запроса, EOF, idle."""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER = os.path.join(HERE, "echo_worker.py")


def run(payload: str, idle: float = 5.0):
    proc = subprocess.Popen(
        [sys.executable, WORKER, "--idle-timeout", str(idle)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    out, err = proc.communicate(payload.encode("utf-8"), timeout=60)
    answers = [json.loads(line) for line in out.decode("utf-8", "replace").splitlines() if line.strip()]
    return answers, err.decode("utf-8", "replace"), proc.returncode


def request(mid, method, params=None):
    return json.dumps({"jsonrpc": "2.0", "id": mid, "method": method, "params": params or {}})


class ProtocolTest(unittest.TestCase):
    def test_hello_errors_isolation_and_shutdown(self):
        payload = "\n".join([
            request(1, "hello"),
            request(2, "boom"),
            request(3, "echo", {"text": "кириллица"}),
            "не json",
            request(4, "nope"),
            request(5, "shutdown"),
        ]) + "\n"
        answers, err, code = run(payload)
        # hello, boom, echo, bad json, неизвестный метод, shutdown
        self.assertEqual(len(answers), 6)
        self.assertEqual(answers[0]["result"]["protocol"], 1)
        self.assertEqual(answers[1]["error"]["code"], -32603)
        self.assertIn("искусственный сбой", answers[1]["error"]["message"])
        # изоляция: после сбоя следующий запрос обслужен
        self.assertEqual(answers[2]["result"], {"text": "кириллица"})
        self.assertEqual(answers[3]["error"]["code"], -32700)
        self.assertEqual(answers[4]["error"]["code"], -32601)
        self.assertEqual(answers[5]["result"], {"bye": True})
        self.assertEqual(code, 0, err)

    def test_eof_stops_process(self):
        answers, _err, code = run(request(1, "hello") + "\n")
        self.assertEqual(len(answers), 1)
        self.assertEqual(code, 0)

    def test_idle_timeout_exits(self):
        proc = subprocess.Popen(
            [sys.executable, WORKER, "--idle-timeout", "1"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            deadline = time.time() + 15
            while time.time() < deadline and proc.poll() is None:
                time.sleep(0.1)
            self.assertIsNotNone(proc.poll(), "воркер должен выйти по простою")
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()

    def test_cancel_is_processed_while_request_runs(self):
        """`control` в reader-потоке: отмена не ждёт конца долгого запроса."""
        proc = subprocess.Popen(
            [sys.executable, WORKER, "--idle-timeout", "10"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            proc.stdin.write((request(1, "sleep", {"seconds": 10}) + "\n").encode("utf-8"))
            proc.stdin.flush()
            time.sleep(0.2)
            started = time.time()
            proc.stdin.write((request(2, "cancel", {"job_id": "x"}) + "\n").encode("utf-8"))
            proc.stdin.flush()
            first = json.loads(proc.stdout.readline().decode("utf-8"))
            elapsed = time.time() - started
            second = json.loads(proc.stdout.readline().decode("utf-8"))
            self.assertEqual(first["id"], 1)
            self.assertTrue(first["result"]["cancelled"])
            self.assertLess(elapsed, 5.0, "отмена должна сработать до таймаута sleep")
            self.assertEqual(second["id"], 2)
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()
            proc.stdin.close()
            proc.stdout.close()
            proc.stderr.close()

    def test_error_has_kind_for_structured_codes(self):
        """`err(..., kind=...)` попадает в ответ (Rust читает машинный код)."""
        code = (
            "import os,sys;sys.path.insert(0,%r);"
            "from hds_sidecar import stdio;stdio.init();"
            "stdio.err(1, -32011, 'нет модели', 'скачайте', kind='model_missing');"
            "stdio.err(2, -32014, 'oom', kind='cuda_oom')"
        ) % os.path.dirname(HERE)
        proc = subprocess.run([sys.executable, "-c", code], capture_output=True, timeout=60)
        answers = [json.loads(l) for l in proc.stdout.decode().splitlines() if l.strip()]
        self.assertEqual(answers[0]["error"]["kind"], "model_missing")
        self.assertEqual(answers[0]["error"]["hint"], "скачайте")
        self.assertEqual(answers[1]["error"]["kind"], "cuda_oom")


if __name__ == "__main__":
    unittest.main()