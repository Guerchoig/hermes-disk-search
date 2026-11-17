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
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


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

    def test_agent_requires_project(self):
        out = os.path.join(self.dir, "bad.docx")
        with self.assertRaises(ValueError):
            asyncio.run(self.handlers.anonymize(
                self.src, out, values=[PERSON_NOMN], mode="agent"))


if __name__ == "__main__":
    unittest.main()
