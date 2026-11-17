"""Fix «слипшихся» ФИО перед анонимизацией: «ВолодинВ.В.» → «Володин В. В.».

Unit-тесты фиксера (name_fixer.py) + интеграционный тест handlers:
исправления вносятся в исходный файл ДО анонимизации, значения
«скрой указанного» после исправления находятся, деанон возвращает
исправленную запись.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import asyncio
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize.vault import Vault  # noqa: E402
from hds_anonymize.handlers import AnonymizeHandlers  # noqa: E402
from hds_anonymize.name_fixer import fix_glued_full_names  # noqa: E402


class NameFixerUnitTest(unittest.TestCase):
    """Чистые unit-тесты regex-правила."""

    def test_surname_first_glued(self):
        text, n = fix_glued_full_names("Исполнитель: ВолодинВ.В. подписал")
        self.assertEqual(text, "Исполнитель: Володин В. В. подписал")
        self.assertEqual(n, 1)

    def test_surname_first_space_between_initials(self):
        text, n = fix_glued_full_names("ВолодинВ. В.")
        self.assertEqual(text, "Володин В. В.")
        self.assertEqual(n, 1)

    def test_initials_first_glued(self):
        text, n = fix_glued_full_names("В.В.Володин прибыл")
        self.assertEqual(text, "В. В. Володин прибыл")
        self.assertEqual(n, 1)

    def test_initials_first_spaced_initials(self):
        text, n = fix_glued_full_names("В. В.Володин прибыл")
        self.assertEqual(text, "В. В. Володин прибыл")
        self.assertEqual(n, 1)

    def test_hyphenated_surname(self):
        text, n = fix_glued_full_names("Римский-КорсаковВ.В.")
        self.assertEqual(text, "Римский-Корсаков В. В.")
        self.assertEqual(n, 1)

    def test_yo_letter(self):
        text, n = fix_glued_full_names("СемёноваЕ.Е.")
        self.assertEqual(text, "Семёнова Е. Е.")
        self.assertEqual(n, 1)

    def test_nbsp_between_initials(self):
        text, n = fix_glued_full_names("ВолодинВ.\u00A0В.")
        self.assertEqual(text, "Володин В. В.")
        self.assertEqual(n, 1)

    def test_multiple_fixes_counted(self):
        text, n = fix_glued_full_names(
            "ВолодинВ.В. и ВдовинД.Д., а также Д.Д.Вдовин"
        )
        self.assertEqual(text, "Володин В. В. и Вдовин Д. Д., а также Д. Д. Вдовин")
        self.assertEqual(n, 3)

    def test_correct_records_untouched(self):
        original = "Володин В. В. и Вдовин Д.В., инициалы Д. В. Вдовин"
        text, n = fix_glued_full_names(original)
        self.assertEqual(text, original)
        self.assertEqual(n, 0)

    def test_abbreviations_not_touched(self):
        # «А.Б.В.Гагарин» — не частично разлипаем последовательность
        # инициалов; «Ф.И.О.» и латиница не матчатся по определению
        for sample in ("А.Б.В.Гагарин", "Ф.И.О.", "IvanovV.V.", "ГОСТ 12.В.5"):
            text, n = fix_glued_full_names(sample)
            self.assertEqual(text, sample, sample)
            self.assertEqual(n, 0, sample)

    def test_empty_text(self):
        self.assertEqual(fix_glued_full_names(""), ("", 0))

    def test_no_line_breaks_inserted(self):
        text, _ = fix_glued_full_names("ВолодинВ.В.\nВторая строка")
        self.assertNotIn("\n\n", text)
        self.assertEqual(text.count("\n"), 1)


class NameFixerHandlerTest(unittest.TestCase):
    """Интеграция: исходный файл исправляется до анонимизации."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-anon-nf-")
        self.dir = self.tmp.name
        self.db = os.path.join(self.dir, "anonymize.db")
        self.store = Vault(self.db)
        self.h = AnonymizeHandlers(self.store)

    def tearDown(self) -> None:
        self.store.close()
        self.tmp.cleanup()

    def _run(self, coro):
        return asyncio.run(coro)

    def test_source_file_fixed_before_anonymization(self):
        src = os.path.join(self.dir, "doc.txt")
        with open(src, "w", encoding="utf-8") as f:
            f.write("Исполнитель: ВолодинВ.В.\r\nДолжность: инженер\r\n")
        out = os.path.join(self.dir, "doc.anonymized.txt")

        res = self._run(self.h.anonymize(
            src, out, values=["Володин В. В."]))

        # 1) исходный файл содержит исправленную запись ДО анонимизации
        with open(src, "r", encoding="utf-8") as f:
            fixed = f.read()
        self.assertIn("Володин В. В.", fixed)
        self.assertNotIn("ВолодинВ.В.", fixed)

        # 2) значение найдено именно после исправления
        self.assertEqual(res["matched"], ["Володин В. В."])

        # 3) предупреждение о правках — только счётчик, без значений
        self.assertTrue(
            any("слипшихся" in w and "1" in w for w in res["warnings"]),
            res["warnings"])

        # 4) анонимизованная копия без значения, деанон возвращает
        #    исправленную запись
        with open(out, "r", encoding="utf-8") as f:
            anon_text = f.read()
        self.assertNotIn("Володин", anon_text)
        self.assertIn("[", anon_text)

        deanon_out = os.path.join(self.dir, "doc.deanon.txt")
        self._run(self.h.deanonymize(out, deanon_out))
        with open(deanon_out, "r", encoding="utf-8") as f:
            self.assertIn("Володин В. В.", f.read())

    def test_no_warning_when_nothing_to_fix(self):
        src = os.path.join(self.dir, "clean.txt")
        with open(src, "w", encoding="utf-8") as f:
            f.write("Исполнитель: Володин В. В.\r\n")
        out = os.path.join(self.dir, "clean.anonymized.txt")
        res = self._run(self.h.anonymize(src, out, values=["Володин В. В."]))
        self.assertFalse(
            any("слипшихся" in w for w in res["warnings"]),
            res["warnings"])
        with open(src, "r", encoding="utf-8") as f:
            self.assertIn("Володин В. В.", f.read())


if __name__ == "__main__":
    unittest.main()
