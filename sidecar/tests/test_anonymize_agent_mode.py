"""Режим `mode: "agent"` в файловых актах (TR-7.4).

Критерий §3.4 карты: в образе нет исходных значений, есть метки проекта с
падежными суффиксами и `[IMAGE_n]`, картинка колонтитула удалена, картинка
тела заменена меткой, маркер `hds-agent/<P>/<artifact_id>` на месте,
оригинальный файл не изменён (sha256 до/после равны).

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import asyncio
import hashlib
import io
import os
import sys
import tempfile
import unittest
import zipfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize.file_parser import read_anon_marker  # noqa: E402
from hds_anonymize.handlers import AnonymizeHandlers  # noqa: E402
from hds_anonymize.vault import Vault  # noqa: E402

def _png_1x1() -> bytes:
    """Валидный PNG 1×1 (белый пиксель) для картинок в тестовом DOCX."""
    import struct
    import zlib

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data)) + tag + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    raw = b"\x00\xff\xff\xff"
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


PERSON_NOMN = "Иванов Иван Иванович"
PERSON_DATV = "Иванову Ивану Ивановичу"


def _sha256(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def _make_docx(path: str) -> None:
    """DOCX с картинкой в колонтитуле и теле, автором и «слипшимся» ФИО."""
    import docx

    document = docx.Document()
    document.core_properties.author = PERSON_NOMN
    document.core_properties.last_modified_by = PERSON_NOMN
    header = document.sections[0].header
    header.paragraphs[0].text = "ООО «Ромашка» — конфиденциально"
    header.paragraphs[0].add_run().add_picture(io.BytesIO(_png_1x1()))
    document.add_paragraph(f"Исполнитель: {PERSON_NOMN}")
    document.add_paragraph("Передать " + PERSON_DATV)
    document.add_paragraph("Подпись: ВолодинВ.В.")
    document.add_paragraph().add_run().add_picture(io.BytesIO(_png_1x1()))
    document.add_paragraph("Система: 1С:ERP, требование REQ-003")
    document.save(path)


DRAWING_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<xdr:wsDr xmlns:xdr="http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing"
          xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <xdr:oneCellAnchor>
    <xdr:from><xdr:col>1</xdr:col><xdr:colOff>0</xdr:colOff>
      <xdr:row>1</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from>
    <xdr:ext cx="9525" cy="9525"/>
    <xdr:pic>
      <xdr:nvPicPr><xdr:cNvPr id="2" name="Picture 1"/><xdr:cNvPicPr/></xdr:nvPicPr>
      <xdr:blipFill><a:blip r:embed="rId1"/>
        <a:stretch><a:fillRect/></a:stretch></xdr:blipFill>
      <xdr:spPr/>
    </xdr:pic>
    <xdr:clientData/>
  </xdr:oneCellAnchor>
</xdr:wsDr>
"""

_RELS_NS = "http://schemas.openxmlformats.org/package/2006/relationships"
_IMAGE_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image"
_DRAWING_REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/drawing"


def _make_xlsx_with_image(
    path: str,
    pii_text: str = "Иванов Иван Иванович",
    extra_cells: dict[str, str] | None = None,
) -> None:
    """XLSX с картинкой на листе — без Pillow (инъекция на уровне пакета).

    openpyxl не умеет добавлять картинки без Pillow, а в venv анонимизации
    его нет; поэтому пакет собирается вручную: media + drawing + rels листа.
    """
    import openpyxl

    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "План"
    ws["A1"] = "Исполнитель"
    ws["B1"] = pii_text
    for address, value in (extra_cells or {}).items():
        ws[address] = value
    # Метаданные пакета — как в реальном файле: их обязан вычистить agent-акт
    wb.properties.creator = pii_text
    wb.properties.lastModifiedBy = pii_text
    wb.properties.title = f"План проекта {pii_text}"
    wb.save(path)

    with zipfile.ZipFile(path) as zin:
        data = {info.filename: zin.read(info.filename) for info in zin.infolist()}

    data["xl/media/image1.png"] = _png_1x1()
    data["xl/drawings/drawing1.xml"] = DRAWING_XML.encode("utf-8")
    data["xl/drawings/_rels/drawing1.xml.rels"] = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        f'<Relationships xmlns="{_RELS_NS}">'
        f'<Relationship Id="rId1" Type="{_IMAGE_REL}" Target="../media/image1.png"/>'
        "</Relationships>"
    ).encode("utf-8")

    sheet_rels = "xl/worksheets/_rels/sheet1.xml.rels"
    rel_id = "rId777"
    rels = data.get(sheet_rels)
    rel_xml = (
        rels.decode("utf-8") if rels else
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        f'<Relationships xmlns="{_RELS_NS}"></Relationships>'
    )
    rel_xml = rel_xml.replace(
        "</Relationships>",
        f'<Relationship Id="{rel_id}" Type="{_DRAWING_REL}"'
        ' Target="../drawings/drawing1.xml"/></Relationships>',
    )
    data[sheet_rels] = rel_xml.encode("utf-8")

    sheet = data["xl/worksheets/sheet1.xml"].decode("utf-8")
    if "xmlns:r=" not in sheet.split(">", 1)[0]:
        # openpyxl не всегда объявляет prefix r в корне листа
        sheet = sheet.replace(
            "<worksheet ",
            '<worksheet xmlns:r="http://schemas.openxmlformats.org/'
            'officeDocument/2006/relationships" ',
            1,
        )
    if "<drawing" not in sheet:
        sheet = sheet.replace(
            "</worksheet>", f'<drawing r:id="{rel_id}"/></worksheet>'
        )
    data["xl/worksheets/sheet1.xml"] = sheet.encode("utf-8")

    content_types = data["[Content_Types].xml"].decode("utf-8")
    if 'Extension="png"' not in content_types:
        content_types = content_types.replace(
            "<Default ",
            '<Default Extension="png" ContentType="image/png"/><Default ',
            1,
        )
    if "/xl/drawings/drawing1.xml" not in content_types:
        content_types = content_types.replace(
            "</Types>",
            '<Override PartName="/xl/drawings/drawing1.xml" ContentType='
            '"application/vnd.openxmlformats-officedocument.drawing+xml"/></Types>',
        )
    data["[Content_Types].xml"] = content_types.encode("utf-8")

    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as zout:
        for name, blob in data.items():
            zout.writestr(name, blob)


class AgentModeTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-agent-mode-")
        self.dir = self.tmp.name
        self.vault = Vault(os.path.join(self.dir, "anonymize.db"))
        self.handlers = AnonymizeHandlers(self.vault)
        self.src = os.path.join(self.dir, "ТЗ.docx")
        _make_docx(self.src)
        self.sha_before = _sha256(self.src)

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()

    def _run(self, **kwargs):
        out = os.path.join(self.dir, "image", "ТЗ.docx")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        res = asyncio.run(self.handlers.anonymize(
            self.src, out,
            values=kwargs.pop("values", [PERSON_NOMN, PERSON_DATV]),
            project_id="P0001",
            mode="agent",
            artifact_id="42",
            **kwargs,
        ))
        return out, res

    def test_original_is_not_changed(self):
        """Оригинал не меняется никогда — даже при «слипшихся» ФИО (TR-7.4)."""
        out, res = self._run()
        self.assertEqual(_sha256(self.src), self.sha_before)
        self.assertTrue(os.path.isfile(out))
        self.assertTrue(
            any("только в образе" in w for w in res["warnings"]),
            res["warnings"],
        )

    def test_labels_and_suffixes(self):
        out, _res = self._run()
        import docx

        text = "\n".join(p.text for p in docx.Document(out).paragraphs)
        self.assertNotIn(PERSON_NOMN, text)
        self.assertNotIn(PERSON_DATV, text)
        self.assertIn("[PERSON_1]", text)          # именительный — без суффикса
        self.assertIn("[PERSON_1|дат]", text)      # дательный — с суффиксом
        self.assertIn("[IMAGE_1]", text)           # картинка тела → метка
        # keep_terms (D19): термины 1С и коды REQ остаются как есть
        self.assertIn("1С:ERP", text)
        self.assertIn("REQ-003", text)

    def test_images_stripped_from_package(self):
        out, _res = self._run()
        with zipfile.ZipFile(out) as package:
            names = package.namelist()
        self.assertFalse(
            [name for name in names if name.startswith("word/media/")],
            f"картинки остались в пакете: {names}",
        )

    def test_metadata_scrubbed_and_marker_written(self):
        out, _res = self._run()
        import docx

        document = docx.Document(out)
        props = document.core_properties
        self.assertNotEqual(props.author, PERSON_NOMN)
        self.assertEqual(props.comments, "hds-agent/P0001/42")
        self.assertEqual(read_anon_marker(out), "hds-agent/P0001/42")

    def test_project_label_counters_are_independent(self):
        """Метки проекта не выводятся из глобального номера (TR-1.5)."""
        _out, res = self._run()
        self.assertEqual(res["mode"], "agent")
        self.assertEqual(res["project_id"], "P0001")
        self.assertEqual(self.vault.stats()["projects"], 1)
        # В проекте P0001 Иванов — [PERSON_1]. В другом проекте своя нумерация:
        self.vault.project_label("P0002", "Петров Пётр", "PERSON")
        self.assertEqual(
            self.vault.project_mappings("P0002")["[PERSON_1]"], "Петров Пётр")
        self.assertEqual(
            self.vault.project_mappings("P0001")["[PERSON_1]"], PERSON_NOMN)
        # глобальный счётчик считает все сущности базы: Петров — PERSON_2
        global_map = self.vault.get_all_mappings()
        self.assertEqual(global_map["[PERSON_1]"], PERSON_NOMN)
        self.assertEqual(global_map["[PERSON_2]"], "Петров Пётр")

    def test_deanonymize_restores_forms(self):
        """Обратная подстановка: `[PERSON_1|дат]` → дательная форма (TR-2.4)."""
        out, _res = self._run()
        deanon = os.path.join(self.dir, "restored.docx")
        asyncio.run(self.handlers.deanonymize(
            out, deanon, project_id="P0001"))
        import docx

        text = "\n".join(p.text for p in docx.Document(deanon).paragraphs)
        self.assertIn(PERSON_NOMN, text)
        self.assertIn(PERSON_DATV, text)

    def test_agent_images_keep_leaves_media(self):
        """`agent.images: keep` — картинки тела остаются (не рекомендуется)."""
        out = os.path.join(self.dir, "keep", "ТЗ.docx")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        res = asyncio.run(self.handlers.anonymize(
            self.src, out, values=[PERSON_NOMN], project_id="P0001",
            mode="agent", artifact_id="1", images_mode="keep",
        ))
        self.assertEqual(res["images"], 0)
        with zipfile.ZipFile(out) as package:
            media = [n for n in package.namelist() if n.startswith("word/media/")]
        self.assertTrue(media, "при keep картинки тела должны остаться")

    def test_agent_requires_project(self):
        out = os.path.join(self.dir, "bad.docx")
        with self.assertRaises(ValueError):
            asyncio.run(self.handlers.anonymize(
                self.src, out, values=[PERSON_NOMN], mode="agent"))


class XlsxAgentModeTest(unittest.TestCase):
    """Картинки листов XLSX: `[IMAGE_n]` вместо картинки, media вымыты."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-agent-xlsx-")
        self.dir = self.tmp.name
        self.vault = Vault(os.path.join(self.dir, "anonymize.db"))
        self.handlers = AnonymizeHandlers(self.vault)
        self.src = os.path.join(self.dir, "План.xlsx")
        _make_xlsx_with_image(self.src)
        self.sha_before = _sha256(self.src)

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()

    def _run(self, **kwargs):
        out = os.path.join(self.dir, "image", "План.xlsx")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        res = asyncio.run(self.handlers.anonymize(
            self.src, out,
            values=kwargs.pop("values", ["Иванов Иван Иванович"]),
            project_id="P0001",
            mode="agent",
            artifact_id="9",
            **kwargs,
        ))
        return out, res

    def test_image_replaced_with_label_and_media_dropped(self):
        out, res = self._run()
        self.assertEqual(res["images"], 1)
        self.assertEqual(_sha256(self.src), self.sha_before, "оригинал не меняется")
        with zipfile.ZipFile(out) as package:
            names = package.namelist()
        self.assertFalse(
            [name for name in names if name.startswith("xl/media/")],
            f"media остались в пакете: {names}",
        )
        import openpyxl

        wb = openpyxl.load_workbook(out)
        try:
            ws = wb["План"]
            values = [str(cell.value) for row in ws.iter_rows() for cell in row
                      if cell.value is not None]
            self.assertIn("[IMAGE_1]", values)
            self.assertIn("[PERSON_1]", values)
            self.assertNotIn("Иванов Иван Иванович", values)
            self.assertEqual(wb.properties.keywords, "hds-agent/P0001/9")
            self.assertNotEqual(wb.properties.creator, "Иванов Иван Иванович")
        finally:
            wb.close()

    def test_label_does_not_overwrite_data(self):
        """Метка не затирает значение в ячейке под якорем картинки."""
        import openpyxl

        # фикстуру собираем сразу с занятой ячейкой B2 (пересохранение
        # openpyxl без Pillow потеряло бы картинку)
        _make_xlsx_with_image(
            self.src, extra_cells={"B2": "важное значение"})
        self.sha_before = _sha256(self.src)
        out, res = self._run()
        self.assertEqual(res["images"], 1)
        wb = openpyxl.load_workbook(out)
        try:
            ws = wb["План"]
            values = [str(cell.value) for row in ws.iter_rows() for cell in row
                      if cell.value is not None]
            self.assertIn("важное значение", values)
            self.assertIn("[IMAGE_1]", values)
        finally:
            wb.close()

    def test_keep_mode_adds_no_labels(self):
        _out, res = self._run(images_mode="keep")
        self.assertEqual(res["images"], 0)


class KnownFormsPassTest(unittest.TestCase):
    """AG-21: словарный проход дозакрывает известные формы, которые NER не увидел.

    NER на короткой строке слабее — в образе оставались открытые имена
    (I1: 22–28 попаданий на стенде). Проход берёт значения из базы проекта и
    заменяет их метками, не задевая уже стоящие метки.
    """

    class _NoNer:
        """NER «ничего не нашёл» — воспроизводит пропуск на короткой строке."""

        async def extract_entities_detailed(self, text, use_llm=True):  # noqa: ANN001
            return [], 0, False

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-known-forms-")
        self.dir = self.tmp.name
        self.vault = Vault(os.path.join(self.dir, "anonymize.db"))
        self.handlers = AnonymizeHandlers(self.vault, ner=self._NoNer())

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()

    def test_known_form_is_closed_without_ner(self):
        label = self.vault.project_label("P0001", PERSON_NOMN, "PERSON")
        self.assertIsNotNone(label)
        res = asyncio.run(
            self.handlers.anonymize_text("P0001", f"Передать {PERSON_NOMN} сегодня\n")
        )
        self.assertIn(label, res["text"])
        self.assertNotIn(PERSON_NOMN, res["text"], "значение осталось открытым")
        self.assertTrue(
            any(item["label"].startswith("[PERSON_") for item in res["labels"]),
            res["labels"],
        )

    def test_existing_labels_are_not_corrupted(self):
        # Короткое значение из базы не должно попасть внутрь готовой метки:
        # `[PERSON_1]` обязан остаться разбираемым.
        self.vault.project_label("P0001", "Иван", "PERSON")
        res = asyncio.run(
            self.handlers.anonymize_text("P0001", "Метка [PERSON_1] и Иван\n")
        )
        self.assertIn("[PERSON_1]", res["text"])
        self.assertNotIn("Иван", res["text"])


if __name__ == "__main__":
    unittest.main()
