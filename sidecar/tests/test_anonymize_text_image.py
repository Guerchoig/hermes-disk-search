"""Текстовые образы pdf/pptx/mpp (D18): маркер, разметка, извлечение.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import asyncio
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize import text_image as images  # noqa: E402
from hds_anonymize.handlers import AnonymizeHandlers  # noqa: E402
from hds_anonymize.vault import Vault  # noqa: E402

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
STAND_PDF = os.path.join(
    REPO, "tools", "parity", "out", "agent_a0", "s10", "project", "03 ТЗ",
    "ТЗ 1С ERP сводное (подписанное).pdf",
)


class TextImageHelpersTest(unittest.TestCase):
    def test_image_path_and_detection(self):
        self.assertEqual(
            images.image_path("D:/ws", "D:/project/ТЗ.pdf").name, "ТЗ.pdf.md")
        self.assertTrue(images.is_text_image("ТЗ.pdf.md"))
        self.assertTrue(images.is_text_image("План.MPP.md"))
        self.assertFalse(images.is_text_image("ТЗ.docx"))

    def test_marker_line_and_parse(self):
        self.assertEqual(
            images.marker_line("P0001", "42"), "<!-- hds-agent/P0001/42 -->")
        self.assertEqual(
            images.marker_line("P0001"), "<!-- hds-agent/P0001 -->")
        body = images.build_text_image("текст", "ТЗ.pdf", "P0001", "42")
        self.assertEqual(images.read_marker(body), ("P0001", "42"))
        self.assertEqual(body.splitlines()[0], "<!-- hds-agent/P0001/42 -->")

    def test_write_is_atomic(self):
        with tempfile.TemporaryDirectory(prefix="hds-image-") as tmp:
            target = os.path.join(tmp, "out", "ТЗ.pdf.md")
            images.write_text_image(target, "тело")
            with open(target, encoding="utf-8") as handle:
                self.assertEqual(handle.read(), "тело")
            self.assertFalse(os.path.exists(target + ".tmp"))

    def test_mpp_requires_extract(self):
        with tempfile.NamedTemporaryFile(suffix=".mpp", delete=False) as handle:
            path = handle.name
        try:
            with self.assertRaises(images.ExtractRequired):
                images.extract_text(path)
        finally:
            os.unlink(path)


class TextImageHandlerTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-image-act-")
        self.dir = self.tmp.name
        self.vault = Vault(os.path.join(self.dir, "anonymize.db"))
        self.handlers = AnonymizeHandlers(self.vault)

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()

    def test_text_image_from_extracted_text(self):
        """Текст из `hds_extract` → метки проекта → `<имя>.pdf.md` (D18)."""
        source = os.path.join(self.dir, "ТЗ.pdf")
        with open(source, "wb") as handle:
            handle.write(b"%PDF-1.4 stub")  # содержимое не читается: текст передан
        text = (
            "<!-- страница 1 -->\n\nИсполнитель: Иванов И. И.\n"
            "Система 1С:ERP, требование REQ-003.\n"
        )
        res = asyncio.run(self.handlers.text_image(
            source, out_dir=self.dir, project_id="P0001",
            artifact_id="7", text=text, ner=False,
        ))
        target = res["out_path"]
        self.assertTrue(os.path.isfile(target))
        self.assertTrue(target.endswith("ТЗ.pdf.md"))
        body = open(target, encoding="utf-8").read()
        self.assertIn("<!-- страница 1 -->", body)
        self.assertIn("[PERSON_1]", body)
        self.assertNotIn("Иванов И. И.", body)
        self.assertIn("1С:ERP", body)        # keep_terms
        self.assertIn("REQ-003", body)       # keep_terms (regex)
        self.assertEqual(images.read_marker(body), ("P0001", "7"))
        self.assertTrue(any("только для чтения" in w for w in res["warnings"]))


@unittest.skipUnless(os.path.isfile(STAND_PDF), "нет PDF стенда S10")
class StandPdfTest(unittest.TestCase):
    def test_extract_stand_pdf_has_pages(self):
        """PDF стенда извлекается постранично (PyMuPDF/портативный CPython)."""
        text = images.extract_text(STAND_PDF)
        self.assertGreater(len(text), 500)
        self.assertIn("<!-- страница 1 -->", text)


if __name__ == "__main__":
    unittest.main()
