"""Обратный путь (TR-7.5/7.7, A4.3): маркер, деанонимизация имён и файла.

Что проверяется:
* `read_marker`: DOCX (свойства пакета) и текстовый образ (`<!-- hds-agent/… -->`);
* `deanonymize_path`: метки проекта → значения, неизвестные — в `unresolved`;
* `deanonymize(restore_from=…, images=True)`: docProps из основания, картинки
  тела возвращаются на место `[IMAGE_n]`, маркер `hds-agent/…` не остаётся;
* XLSX: docProps из основания (картинки — только при наличии Pillow).

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import asyncio
import base64
import io
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from docx import Document  # noqa: E402
from openpyxl import Workbook  # noqa: E402
from openpyxl import load_workbook  # noqa: E402

from hds_anonymize import file_parser  # noqa: E402
from hds_anonymize.handlers import AnonymizeHandlers  # noqa: E402
from hds_anonymize.vault import Vault  # noqa: E402

# 1×1 PNG — достаточно, чтобы python-docx/openpyxl приняли картинку
PNG_1PX = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
)

PERSON_VALUE = "Иванов И. И."


class ReverseHelpersTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-anon-rev-")
        self.dir = self.tmp.name
        self.vault = Vault(os.path.join(self.dir, "anonymize.db"))
        self.handlers = AnonymizeHandlers(self.vault)

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()

    # ── маркер ──────────────────────────────────────────────────────────

    def test_read_marker_from_text_image(self):
        path = os.path.join(self.dir, "ТЗ.pdf.md")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("<!-- hds-agent/P0001/42 -->\n# ТЗ\n")
        res = self.handlers.read_marker(path)
        self.assertEqual(res["marker"], "hds-agent/P0001/42")
        self.assertEqual(res["project_id"], "P0001")
        self.assertEqual(res["artifact_id"], "42")

    def test_read_marker_from_docx_props(self):
        path = os.path.join(self.dir, "образ.docx")
        doc = Document()
        doc.add_paragraph("текст")
        doc.core_properties.comments = "hds-agent/P0001/7"
        doc.save(path)
        res = self.handlers.read_marker(path)
        self.assertEqual(res["project_id"], "P0001")
        self.assertEqual(res["artifact_id"], "7")

    def test_read_marker_absent(self):
        path = os.path.join(self.dir, "письмо.md")
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("# без маркера\n")
        res = self.handlers.read_marker(path)
        self.assertIsNone(res["marker"])
        self.assertIsNone(res["project_id"])
        self.assertIsNone(res["artifact_id"])

    # ── имена ───────────────────────────────────────────────────────────

    def test_deanonymize_path_uses_project_values(self):
        label = self.handlers.label_for_value(
            "P0001", PERSON_VALUE, "PERSON")["label"]
        res = self.handlers.deanonymize_path(
            "P0001", f"Протоколы\\Встреча {label}.docx")
        self.assertEqual(res["path"], f"Протоколы\\Встреча {PERSON_VALUE}.docx")
        self.assertEqual(res["components"][1], f"Встреча {PERSON_VALUE}.docx")
        self.assertEqual(res["unresolved"], [])

    def test_deanonymize_path_keeps_unknown_label(self):
        res = self.handlers.deanonymize_path(
            "P0001", "Архив\\Отчёт [PERSON_999].md")
        self.assertIn("[PERSON_999]", res["path"])
        self.assertEqual(res["unresolved"], ["[PERSON_999]"])

    def test_deanonymize_path_requires_project(self):
        with self.assertRaises(ValueError):
            self.handlers.deanonymize_path("", "a [PERSON_1].md")
        with self.assertRaises(ValueError):
            self.handlers.deanonymize_path("P0001", "")


class RestoreTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-anon-restore-")
        self.dir = self.tmp.name
        self.zone = os.path.join(self.dir, "zone")
        os.makedirs(self.zone, exist_ok=True)
        self.vault = Vault(os.path.join(self.dir, "anonymize.db"))
        self.handlers = AnonymizeHandlers(self.vault)
        self.label = self.handlers.label_for_value(
            "P0001", PERSON_VALUE, "PERSON")["label"]

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()

    def test_docx_restore_props_and_images(self):
        base = os.path.join(self.dir, "Договор.docx")
        doc = Document()
        doc.add_paragraph(f"Контакт: {PERSON_VALUE}")
        doc.add_paragraph().add_run().add_picture(io.BytesIO(PNG_1PX))
        doc.core_properties.author = "Автор Оригинала"
        doc.core_properties.title = "Договор"
        doc.save(base)

        image = os.path.join(self.zone, "Договор.docx")
        asyncio.run(self.handlers.anonymize(
            base, image, values=[PERSON_VALUE], project_id="P0001",
            mode="agent", artifact_id="7"))
        anon = Document(image)
        anon_text = "\n".join(p.text for p in anon.paragraphs)
        self.assertIn(self.label, anon_text)
        self.assertIn("[IMAGE_1]", anon_text)
        self.assertNotIn(PERSON_VALUE, anon_text)
        self.assertEqual(anon.core_properties.comments, "hds-agent/P0001/7")

        entries = file_parser.image_map_for_path(base)
        self.assertEqual(len(entries), 1)
        self.assertTrue(entries[0]["part"].startswith("word/media/"),
                        entries)

        pending = os.path.join(self.dir, "pending", "Договор.docx")
        res = asyncio.run(self.handlers.deanonymize(
            image, pending, project_id="P0001",
            restore_from=base, images=True))
        self.assertEqual(res["images_restored"], 1, res["warnings"])

        restored = Document(pending)
        text = "\n".join(p.text for p in restored.paragraphs)
        self.assertIn(PERSON_VALUE, text)
        self.assertNotIn(self.label, text)
        self.assertNotIn("[IMAGE_", text)
        self.assertEqual(len(restored.inline_shapes), 1)
        self.assertEqual(restored.core_properties.author, "Автор Оригинала")
        self.assertEqual(restored.core_properties.title, "Договор")
        self.assertNotIn("hds-agent", restored.core_properties.comments or "")

    def test_xlsx_restore_props(self):
        base = os.path.join(self.dir, "План.xlsx")
        wb = Workbook()
        wb.active["A1"] = f"Контакт: {PERSON_VALUE}"
        wb.properties.creator = "Автор Оригинала"
        wb.properties.title = "План"
        wb.save(base)

        image = os.path.join(self.zone, "План.xlsx")
        asyncio.run(self.handlers.anonymize(
            base, image, values=[PERSON_VALUE], project_id="P0001",
            mode="agent", artifact_id="9"))
        anon = load_workbook(image)
        try:
            self.assertEqual(anon.properties.keywords, "hds-agent/P0001/9")
        finally:
            anon.close()

        pending = os.path.join(self.dir, "pending", "План.xlsx")
        asyncio.run(self.handlers.deanonymize(
            image, pending, project_id="P0001",
            restore_from=base, images=False))
        restored = load_workbook(pending)
        try:
            self.assertEqual(restored.properties.creator, "Автор Оригинала")
            self.assertEqual(restored.properties.title, "План")
            self.assertIsNone(restored.properties.keywords)
            self.assertIn(PERSON_VALUE, restored.active["A1"].value)
        finally:
            restored.close()

    def test_without_restore_from_props_untouched(self):
        """Обычная деанонимизация (CLI/события) не переписывает docProps."""
        source = os.path.join(self.zone, "a.md")
        with open(source, "w", encoding="utf-8") as handle:
            handle.write(f"Текст: {self.label}\n")
        out = os.path.join(self.dir, "a.deanon.md")
        asyncio.run(self.handlers.deanonymize(
            source, out, project_id="P0001"))
        with open(out, encoding="utf-8") as handle:
            self.assertIn(PERSON_VALUE, handle.read())

    def test_md_marker_is_stripped_on_deanonymize(self):
        """Маркер текстового образа не уезжает в деанонимизированный файл."""
        source = os.path.join(self.zone, "ТЗ [PERSON_1].md")
        with open(source, "w", encoding="utf-8") as handle:
            handle.write("<!-- hds-agent/P0001/5 -->\n[PERSON_1] текст\n")
        out = os.path.join(self.dir, "out.md")
        asyncio.run(self.handlers.deanonymize(source, out, project_id="P0001"))
        with open(out, encoding="utf-8") as handle:
            body = handle.read()
        self.assertNotIn("hds-agent", body)
        self.assertIn(PERSON_VALUE, body)

    def test_anonymize_md_writes_marker(self):
        """Образ текста в режиме agent получает маркер первой строкой."""
        source = os.path.join(self.dir, "Письмо Иванов Иван.md")
        with open(source, "w", encoding="utf-8") as handle:
            handle.write(f"Контакт: {PERSON_VALUE}\n")
        image = os.path.join(self.zone, "Письмо [PERSON_1].md")
        asyncio.run(self.handlers.anonymize(
            source, image, values=[PERSON_VALUE], project_id="P0001",
            mode="agent", artifact_id="3"))
        with open(image, encoding="utf-8") as handle:
            body = handle.read()
        self.assertTrue(body.startswith("<!-- hds-agent/P0001/3 -->"), body)
        self.assertIn(self.label, body)
        marker = self.handlers.read_marker(image)
        self.assertEqual(marker["project_id"], "P0001")
        self.assertEqual(marker["artifact_id"], "3")


if __name__ == "__main__":
    unittest.main()
