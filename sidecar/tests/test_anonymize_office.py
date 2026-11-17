"""Office-операции агента (TR-3.5): чтение, правка, создание.

Критерий §3.6 карты: правка ячейки xlsx сохраняет формулу и формат, правка
абзаца docx не теряет стили, `office_create` создаёт читаемый файл.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize.office_ops import (  # noqa: E402
    OfficeError,
    office_create,
    office_edit,
    office_read,
)

MARKDOWN = """# Требования к системе

Вводный абзац документа.

## Реестр требований

| Код | Требование | Приоритет |
| --- | --- | --- |
| REQ-001 | Учёт договоров | Высокий |
| REQ-002 | Отчёты | Средний |

- Первый тезис
- Второй тезис
"""


class DocxOfficeTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-office-docx-")
        self.path = os.path.join(self.tmp.name, "ТЗ.docx")
        office_create(self.path, "docx", {"markdown": MARKDOWN})

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_read_has_paragraphs_and_tables(self):
        result = office_read(self.path)
        self.assertEqual(result["kind"], "docx")
        self.assertTrue(result["paragraphs"])
        self.assertEqual(len(result["tables"]), 1)
        self.assertIn("REQ-001", result["markdown"])

    def test_replace_text_keeps_style(self):
        office_edit(self.path, [{
            "op": "replace_text",
            "find": "Требования к системе",
            "replace": "Требования к учётной системе",
        }])
        result = office_read(self.path)
        headline = next(
            p for p in result["paragraphs"] if "учётной системе" in p["text"]
        )
        self.assertEqual(headline["style"], "Heading 1")

    def test_cell_and_row_operations(self):
        office_edit(self.path, [
            {"op": "set_cell", "table": 0, "row": 1, "col": 2, "text": "Критичный"},
            {"op": "add_row", "table": 0, "values": ["REQ-003", "Кадры", "Низкий"]},
            {"op": "add_column", "table": 0, "header": "Оценка"},
        ])
        table = office_read(self.path)["tables"][0]
        self.assertEqual(table[1][2], "Критичный")
        self.assertEqual(table[-1][0], "REQ-003")
        self.assertEqual(table[0][-1], "Оценка")

    def test_insert_and_delete_paragraph(self):
        office_edit(self.path, [
            {"op": "insert_paragraph", "index": 2, "text": "Вставленный абзац"},
        ])
        texts = [p["text"] for p in office_read(self.path)["paragraphs"]]
        self.assertIn("Вставленный абзац", texts)
        index = texts.index("Вставленный абзац") + 1
        office_edit(self.path, [{"op": "delete_paragraph", "index": index}])
        texts = [p["text"] for p in office_read(self.path)["paragraphs"]]
        self.assertNotIn("Вставленный абзац", texts)

    def test_unknown_operation(self):
        with self.assertRaises(OfficeError):
            office_edit(self.path, [{"op": "make_coffee"}])


class XlsxOfficeTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-office-xlsx-")
        self.path = os.path.join(self.tmp.name, "План.xlsx")
        office_create(self.path, "xlsx", {"sheets": [{
            "name": "План",
            "rows": [
                ["Этап", "Сумма"],
                ["Обследование", 100],
                ["Внедрение", 200],
            ],
        }]})
        import openpyxl

        wb = openpyxl.load_workbook(self.path)
        ws = wb["План"]
        ws["C1"] = "Итого"
        ws["C2"] = "=B2+B3"
        ws["C2"].number_format = "#,##0.00"
        wb.save(self.path)
        wb.close()

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_read_cells_with_addresses(self):
        result = office_read(self.path)
        self.assertEqual(result["kind"], "xlsx")
        addresses = {cell["address"] for cell in result["sheets"][0]["cells"]}
        self.assertIn("A1", addresses)
        self.assertIn("C2", addresses)

    def test_edit_keeps_formula_and_format(self):
        office_edit(self.path, [
            {"op": "set_cell", "sheet": "План", "cell": "B2", "value": 150},
            {"op": "set_cell", "sheet": "План", "cell": "C3", "value": 300},
        ])
        import openpyxl

        wb = openpyxl.load_workbook(self.path, data_only=False)
        ws = wb["План"]
        self.assertEqual(ws["B2"].value, 150)
        self.assertEqual(ws["C2"].value, "=B2+B3")
        self.assertEqual(ws["C2"].number_format, "#,##0.00")
        wb.close()

    def test_add_row_and_column(self):
        office_edit(self.path, [
            {"op": "add_row", "sheet": "План", "values": ["Сопровождение", 50]},
            {"op": "add_column", "sheet": "План", "values": ["Комментарий"]},
        ])
        sheet = office_read(self.path)["sheets"][0]
        self.assertIn("Сопровождение", sheet["markdown"])
        self.assertIn(
            "Комментарий", {cell["value"] for cell in sheet["cells"]})

    def test_replace_text_in_cells(self):
        office_edit(self.path, [{
            "op": "replace_text", "find": "Обследование",
            "replace": "Обследование процессов",
        }])
        self.assertIn("Обследование процессов",
                      office_read(self.path)["markdown"])


class PptxOfficeTest(unittest.TestCase):
    def setUp(self) -> None:
        try:
            import pptx  # noqa: F401
        except ImportError:
            self.skipTest("python-pptx не установлен в venv анонимизации")
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-office-pptx-")
        self.path = os.path.join(self.tmp.name, "Презентация.pptx")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_create_read_edit(self):
        office_create(self.path, "pptx", {"slides": [
            {"title": "План проекта", "bullets": ["Этап 1", "Этап 2"]},
            {"title": "Риски", "bullets": ["Сроки"]},
        ]})
        result = office_read(self.path)
        self.assertEqual(result["kind"], "pptx")
        self.assertEqual(len(result["slides"]), 2)
        office_edit(self.path, [{
            "op": "replace_text",
            "find": "Этап 1",
            "replace": "Этап 1 (обследование)",
        }])
        self.assertIn("обследование", office_read(self.path)["markdown"])


if __name__ == "__main__":
    unittest.main()
