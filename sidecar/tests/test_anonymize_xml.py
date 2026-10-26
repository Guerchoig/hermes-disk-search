"""P1b: XML (MS Project) — приёмка по §5 плана: текст и структура тегов
идентичны после «анон → деан», PII уходит, в `inspect` — плейсхолдеры
без значений; маркера у XML нет (предупреждение по плейсхолдерам).

Фикстура: sidecar/tests/fixtures/anonymize/msproject.xml.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import asyncio
import os
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize.file_parser import FileParser, read_anon_marker  # noqa: E402
from hds_anonymize.handlers import AnonymizeHandlers  # noqa: E402
from hds_anonymize.mapping_store import MappingStore  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURE = os.path.join(HERE, "fixtures", "anonymize", "msproject.xml")

VALUES = ["Иванов Иван", "Петров Пётр", "ООО Ромашка", "51 903,14"]


def _parse(path: str):
    return asyncio.run(FileParser().parse(open(path, "rb").read(), os.path.basename(path)))


class XmlRoundtripTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-anon-xml-")
        self.dir = self.tmp.name
        self.src = os.path.join(self.dir, "msproject.xml")
        shutil.copyfile(FIXTURE, self.src)
        self.store = MappingStore(os.path.join(self.dir, "anonymize.db"))
        self.h = AnonymizeHandlers(self.store)

    def tearDown(self) -> None:
        self.store.close()
        self.tmp.cleanup()

    def test_parse_detects_msproject(self):
        parsed = _parse(self.src)
        self.assertEqual(parsed.metadata["xml_type"], "msproject")
        for value in VALUES:
            self.assertIn(value, parsed.text)

    def test_anonymize_removes_pii_keeps_structure(self):
        out = os.path.join(self.dir, "msproject.anonymized.xml")
        res = asyncio.run(self.h.anonymize(self.src, out, values=VALUES))
        self.assertEqual(res["unmatched"], [])
        anon = _parse(out)
        for value in VALUES:
            self.assertNotIn(value, anon.text, f"PII утекло: {value!r}")
        self.assertIn("[PERSON_1]", anon.text)
        # структура тегов сохранена: тот же корень и число элементов
        original = _parse(self.src)
        self.assertEqual(
            anon.metadata["root_tag"], original.metadata["root_tag"]
        )
        self.assertEqual(
            len(anon.structure["elements"]), len(original.structure["elements"])
        )
        # маркера у XML нет; inspect находит плейсхолдеры
        self.assertIsNone(read_anon_marker(out))
        info = asyncio.run(self.h.inspect(out))
        self.assertTrue(info["placeholders"])
        self.assertTrue(all(p["in_db"] for p in info["resolved"]))
        self.assertNotIn("Иванов", str(info))

    def test_roundtrip_text_and_tags_identical(self):
        out_anon = os.path.join(self.dir, "msproject.anonymized.xml")
        asyncio.run(self.h.anonymize(self.src, out_anon, values=VALUES))
        out_deanon = os.path.join(self.dir, "msproject.deanon.xml")
        res = asyncio.run(self.h.deanonymize(out_anon, out_deanon))
        self.assertEqual(res["unmatched"], [])
        original = _parse(self.src)
        deanon = _parse(out_deanon)
        self.assertEqual(deanon.text, original.text)
        self.assertEqual(deanon.structure, original.structure)
        self.assertEqual(deanon.metadata["root_tag"], original.metadata["root_tag"])

    def test_partial_deanonymize_values(self):
        out_anon = os.path.join(self.dir, "msproject.anonymized.xml")
        res = asyncio.run(self.h.anonymize(self.src, out_anon, values=VALUES))
        tokens = [t["token"] for t in res["tokens"]]
        subset = tokens[:1]
        out_deanon = os.path.join(self.dir, "msproject.deanon.xml")
        asyncio.run(self.h.deanonymize(out_anon, out_deanon, tokens=subset))
        text = _parse(out_deanon).text
        for t in tokens:
            if t in subset:
                self.assertNotIn(t, text)
            else:
                self.assertIn(t, text)

    def test_reanonymize_warning_on_placeholders(self):
        out1 = os.path.join(self.dir, "a.anonymized.xml")
        asyncio.run(self.h.anonymize(self.src, out1, values=VALUES))
        res = asyncio.run(self.h.anonymize(
            out1, os.path.join(self.dir, "b.anonymized.xml"), values=VALUES))
        self.assertTrue(
            res["warnings"], "XML без маркера — warning по плейсхолдерам"
        )


if __name__ == "__main__":
    unittest.main()
