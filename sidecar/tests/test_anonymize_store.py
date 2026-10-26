"""mapping_store (P1): per-type счётчики, дедуп по (значение, тип),
сквозная нумерация между актами, unmatched, WAL-флаги БД.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import os
import sqlite3
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize.mapping_store import MappingStore  # noqa: E402


class MappingStoreTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-anon-test-")
        self.db = os.path.join(self.tmp.name, "anonymize.db")
        self.store = MappingStore(self.db)

    def tearDown(self) -> None:
        self.store.close()
        self.tmp.cleanup()

    def test_per_type_counters(self):
        """Счётчики per-type: [PERSON_1], [ORG_1], [PERSON_2]…"""
        self.assertEqual(
            self.store.get_or_create_token("Иванов И.И.", "PERSON"), "[PERSON_1]"
        )
        self.assertEqual(
            self.store.get_or_create_token("ООО Ромашка", "ORG"), "[ORG_1]"
        )
        self.assertEqual(
            self.store.get_or_create_token("Петров П.П.", "PERSON"), "[PERSON_2]"
        )

    def test_dedup_by_value_and_type(self):
        """Дедуп по (значение, тип): одно значение — один токен;
        то же значение другого типа — ДРУГОЙ токен (решение §2.1)."""
        t1 = self.store.get_or_create_token("Иванов И.И.", "PERSON")
        t2 = self.store.get_or_create_token("Иванов И.И.", "PERSON")
        self.assertEqual(t1, t2)
        t3 = self.store.get_or_create_token("Иванов И.И.", "MISC")
        self.assertNotEqual(t1, t3)

    def test_numbering_persists_across_acts(self):
        """Сквозная нумерация: новый store на той же БД продолжает счётчик
        (акты — единичные, БД общая)."""
        first = self.store.get_or_create_token("Сидоров С.С.", "PERSON")
        self.store.close()
        self.store = MappingStore(self.db)
        second = self.store.get_or_create_token("Иванов И.И.", "PERSON")
        self.assertEqual(first, "[PERSON_1]")
        self.assertEqual(second, "[PERSON_2]")

    def test_get_all_and_subset(self):
        token = self.store.get_or_create_token("Иванов И.И.", "PERSON")
        self.store.get_or_create_token("АО Ромашка", "ORG")
        all_map = self.store.get_all_mappings()
        self.assertEqual(set(all_map), {token, "[ORG_1]"})
        self.assertEqual(all_map[token], "Иванов И.И.")

        found, unmatched = self.store.get_mappings_for_tokens([token, "[PERSON_99]"])
        self.assertEqual(found, {token: "Иванов И.И."})
        self.assertEqual(unmatched, ["[PERSON_99]"])

    def test_wal_and_busy_timeout(self):
        """WAL + busy_timeout выставлены (два процесса на одной БД)."""
        mode = self.store._conn.execute("PRAGMA journal_mode").fetchone()[0]
        self.assertEqual(mode.lower(), "wal")
        busy = self.store._conn.execute("PRAGMA busy_timeout").fetchone()[0]
        self.assertEqual(busy, 5000)

    def test_schema_shape(self):
        cols = {
            r[1] for r in self.store._conn.execute("PRAGMA table_info(mappings)")
        }
        self.assertEqual(cols, {"token", "value", "type", "ts"})
        counters = {
            r[1] for r in self.store._conn.execute("PRAGMA table_info(counters)")
        }
        self.assertEqual(counters, {"type", "last"})
        # уник. индекс дедупа существует
        idx = [
            r[1] for r in self.store._conn.execute("PRAGMA index_list(mappings)")
        ]
        self.assertIn("ux_mappings_value_type", idx)

    def test_two_processes_concurrent_numbering(self):
        """Два соединения на одной БД: выдача номеров не дублируется
        (BEGIN IMMEDIATE + busy_timeout)."""
        other = MappingStore(self.db)
        try:
            a = self.store.get_or_create_token("Значение А", "MISC")
            b = other.get_or_create_token("Значение Б", "MISC")
            self.assertNotEqual(a, b)
            self.assertEqual(b, "[MISC_2]")
        finally:
            other.close()


if __name__ == "__main__":
    unittest.main()
