"""End-to-end тест воркера hds_anonymize (P1): протокол по stdio,
kind-ошибки, progress-нотификации без id, чистота stdout.

NER-модели не установлены (offline-first) — полный `anonymize` обязан
отдать `kind: model_missing`; детерминированные сценарии (`anonymize_values`,
`deanonymize_values`, `inspect`) работают без весов.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
WORKER = os.path.join(os.path.dirname(HERE), "hds_anonymize", "worker.py")


def run(payload_lines: list[str], root: str, db: str, models: str, timeout=120):
    """Прогнать строки запросов через воркер; вернуть (ответы, stderr)."""
    argv = [
        sys.executable, WORKER,
        "--root", root,
        "--db", db,
        "--models-dir", models,
        "--natasha", "0",
        "--idle-timeout", "10",
    ]
    proc = subprocess.Popen(
        argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    out, err = proc.communicate("\n".join(payload_lines).encode("utf-8"), timeout)
    answers = [
        json.loads(line)
        for line in out.decode("utf-8", "replace").splitlines()
        if line.strip()
    ]
    return answers, err.decode("utf-8", "replace"), proc.returncode


def req(mid, method, params=None):
    return json.dumps(
        {"jsonrpc": "2.0", "id": mid, "method": method, "params": params or {}},
        ensure_ascii=False,
    )


class AnonymizeWorkerTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.tmp = tempfile.TemporaryDirectory(prefix="hds-anon-worker-")
        cls.root = os.path.dirname(HERE)  # корень проекта
        cls.db = os.path.join(cls.tmp.name, "anonymize.db")
        cls.models = os.path.join(cls.tmp.name, "models")  # пусто → model_missing
        # TXT-фикстура
        cls.txt = os.path.join(cls.tmp.name, "doc.txt")
        with open(cls.txt, "w", encoding="utf-8", newline="") as f:
            f.write("Контакт: Иванов Иван, +7 912 345-67-89\nСумма: 51 903,14\n")

    @classmethod
    def tearDownClass(cls) -> None:
        cls.tmp.cleanup()

    def test_hello_and_full_flow_without_models(self):
        payload = [
            req(1, "hello"),
            req(2, "warmup"),
            req(3, "anonymize", {"in_path": self.txt, "out_dir": self.tmp.name}),
            req(4, "anonymize_values", {
                "in_path": self.txt, "out_dir": self.tmp.name,
                "values": ["Иванов Иван", "+7 912 345-67-89", "51 903,14"],
            }),
            req(5, "deanonymize", {
                "in_path": os.path.join(self.tmp.name, "doc.anonymized.txt"),
                "out_dir": self.tmp.name,
            }),
            req(6, "shutdown"),
        ]
        answers, err, code = run(payload, self.root, self.db, self.models)

        # прогресс-нотификации приходят без id; ответы — по id
        replies = {a["id"]: a for a in answers if "id" in a}
        self.assertEqual(
            sorted(replies), [1, 2, 3, 4, 5, 6],
            f"ожидаем ответы на все 6 запросов; stderr: {err}",
        )

        # hello: протокол, форматы, capabilities — stdout содержит ТОЛЬКО протокол
        hello = replies[1]["result"]
        self.assertEqual(hello["protocol"], 1)
        self.assertEqual(hello["formats"], ["docx", "xlsx", "txt", "md", "xml"])
        self.assertIn("anonymize_values", hello["capabilities"])
        self.assertEqual(hello["model"], "knowledgator/gliner-pii-large-v1.0")

        # warmup без весов → kind: model_missing + hint про установщик
        werr = replies[2].get("error") or {}
        self.assertEqual(werr.get("kind"), "model_missing", err)
        self.assertIn("fetch_anonymize_models", werr.get("hint", ""))

        # полный anonymize без весов → model_missing (воркер жив)
        aerr = replies[3].get("error") or {}
        self.assertEqual(aerr.get("kind"), "model_missing")

        # детерминированный акт работает БЕЗ весов
        res = replies[4]["result"]
        self.assertTrue(res["out_path"].endswith("doc.anonymized.txt"))
        types = {t["type"] for t in res["tokens"]}
        # is_name_like → PERSON; телефон и сумма → MISC (порт из command_args)
        self.assertEqual(types, {"PERSON", "MISC"})
        self.assertTrue(os.path.isfile(res["out_path"]))
        anon_text = open(res["out_path"], encoding="utf-8").read()
        self.assertNotIn("Иванов Иван", anon_text)
        self.assertIn("[PERSON_1]", anon_text)

        # деанонимизация восстановила значения
        res = replies[5]["result"]
        self.assertTrue(res["out_path"].endswith("doc.deanon.txt"))
        de_text = open(res["out_path"], encoding="utf-8").read()
        self.assertIn("Иванов Иван", de_text)
        self.assertIn("+7 912 345-67-89", de_text)
        self.assertEqual(res["unmatched"], [])
        self.assertEqual(code, 0, err)

    def test_progress_notifications_have_no_id(self):
        """Прогресс — NDJSON-нотификации без `id` (клиент их пропускает)."""
        payload = [
            req(1, "anonymize_values", {
                "in_path": self.txt, "out_dir": self.tmp.name,
                "values": ["Иванов Иван"],
            }),
            req(2, "shutdown"),
        ]
        answers, err, code = run(payload, self.root, self.db, self.models)
        self.assertEqual(code, 0, err)
        notifications = [a for a in answers if "id" not in a]
        self.assertTrue(notifications, "должны быть progress-нотификации")
        for n in notifications:
            self.assertEqual(n["method"], "progress")
            self.assertIn("file", n["params"])
            self.assertIn("phase", n["params"])
        replies = [a for a in answers if "id" in a]
        self.assertEqual(len(replies), 2)

    def test_stderr_has_no_pii(self):
        """PII-значения не попадают в stderr воркера."""
        payload = [
            req(1, "anonymize_values", {
                "in_path": self.txt, "out_dir": self.tmp.name,
                "values": ["Иванов Иван", "+7 912 345-67-89"],
            }),
            req(2, "shutdown"),
        ]
        _answers, err, _code = run(payload, self.root, self.db, self.models)
        self.assertNotIn("Иванов Иван", err)
        self.assertNotIn("+7 912", err)


if __name__ == "__main__":
    unittest.main()
