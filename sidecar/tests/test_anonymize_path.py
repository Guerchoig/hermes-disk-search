"""`anonymize_path` (TR-7.1, A4.2): имена файлов и папок без PII и без `|`.

Что проверяется:
* санитайзинг имени: недопустимые Windows-символы, схлопывание пробелов,
  хвостовые точки/пробелы, пустое имя;
* NER по компонентам пути (`ner=False` — regex-слой, модели не нужны),
  расширение сохраняется, разделитель входа сохраняется;
* `suffix=true` — явный отказ (падежный суффикс содержит `|`, запрещённый
  Windows), а не молчаливая порча имени;
* ошибки: пустой путь, нет `project_id`.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import asyncio
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize.handlers import AnonymizeHandlers, sanitize_component  # noqa: E402
from hds_anonymize.vault import Vault  # noqa: E402


class SanitizeComponentTest(unittest.TestCase):
    def test_invalid_windows_chars_are_removed(self):
        self.assertEqual(sanitize_component('a<b>c:d"e/f\\g|h?i*j'), "abcdefghij")
        self.assertEqual(sanitize_component("имя\u0001с\u001fимвол"), "имясимвол")

    def test_spaces_collapse_and_tail_is_trimmed(self):
        self.assertEqual(sanitize_component("имя   с   пробелами"), "имя с пробелами")
        self.assertEqual(sanitize_component("хвост...  "), "хвост")
        self.assertEqual(sanitize_component("  ведущие  "), "ведущие")

    def test_empty_becomes_placeholder(self):
        self.assertEqual(sanitize_component(""), "_")
        self.assertEqual(sanitize_component("??"), "_")
        self.assertEqual(sanitize_component("..."), "_")


class AnonymizePathTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-anon-path-")
        self.vault = Vault(os.path.join(self.tmp.name, "anonymize.db"))
        self.handlers = AnonymizeHandlers(self.vault)

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()

    def path(self, rel: str, **kwargs) -> dict:
        kwargs.setdefault("ner", False)
        return asyncio.run(
            self.handlers.anonymize_path("P0001", rel, **kwargs)
        )

    def test_directory_and_file_names_get_labels(self):
        res = self.path(
            "Протоколы Иванов И. И.\\Встреча Иванов И. И..docx"
        )
        self.assertTrue(res["path"].startswith("Протоколы [PERSON_"), res["path"])
        self.assertIn("[PERSON_", res["path"])
        self.assertNotIn("Иванов", res["path"])
        self.assertNotIn("|", res["path"])
        self.assertTrue(res["path"].endswith(".docx"), res["path"])
        self.assertEqual(len(res["components"]), 2)
        self.assertEqual(res["path"], "\\".join(res["components"]))
        self.assertTrue(any(l["label"].startswith("[PERSON_") for l in res["labels"]))

    def test_forward_slashes_are_preserved(self):
        res = self.path("a/Иванов И. И..txt")
        self.assertIn("/", res["path"])
        self.assertNotIn("\\", res["path"])
        self.assertTrue(res["path"].endswith(".txt"))

    def test_sanitizing_runs_after_labels(self):
        res = self.path('Отчёт <> Иванов И. И..txt')
        self.assertNotIn("<", res["path"])
        self.assertNotIn(">", res["path"])
        self.assertNotIn("Иванов", res["path"])
        self.assertTrue(res["path"].endswith(".txt"), res["path"])

    def test_suffix_is_rejected(self):
        with self.assertRaises(ValueError):
            self.path("Иванов И. И.docx", suffix=True)

    def test_empty_path_and_project_are_rejected(self):
        with self.assertRaises(ValueError):
            self.path("")
        with self.assertRaises(ValueError):
            asyncio.run(self.handlers.anonymize_path("", "a.txt", ner=False))

    def test_parent_component_is_not_turned_into_name(self):
        # `..` до сюда доходить не должен; если дошёл — не превращаем его в
        # «имя» и не теряем (иначе выход из папки стал бы безвредным).
        res = self.path("..\\Иванов И. И..txt")
        self.assertEqual(res["components"][0], "..")


if __name__ == "__main__":
    unittest.main()
