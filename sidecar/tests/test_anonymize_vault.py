"""База меток v2 (TR-1.1…TR-1.6, D3/D4/D19): схема, два уровня, снимок,
«Объединить метки», удаление проекта, keep_terms, миграция v1 → новый файл.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import os
import sqlite3
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize import vault as vault_mod  # noqa: E402
from hds_anonymize.vault import Vault  # noqa: E402

PERSON = "Иванов Иван Иванович"
PERSON_DATV = "Иванову Ивану Ивановичу"


class VaultBase(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-vault-")
        self.db = os.path.join(self.tmp.name, "anonymize.db")
        self.vault = Vault(self.db)

    def tearDown(self) -> None:
        self.vault.close()
        self.tmp.cleanup()


class SchemaTest(VaultBase):
    def test_schema_version_and_tables(self):
        """Схема — ровно TR-1.2 (одна база, два уровня меток)."""
        tables = {
            row[0]
            for row in self.vault._conn.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table'"
            )
        }
        # sqlite_sequence — служебная таблица AUTOINCREMENT, не часть схемы
        tables.discard("sqlite_sequence")
        self.assertEqual(
            tables,
            {
                "meta", "entities", "forms", "global_counters",
                "projects", "labels", "counters", "aliases", "keep_terms",
            },
        )
        version = self.vault._conn.execute(
            "SELECT value FROM meta WHERE key = 'schema_version'"
        ).fetchone()[0]
        self.assertEqual(int(version), 2)

    def test_columns(self):
        expected = {
            "entities": {"id", "type", "n", "norm_key", "ts"},
            "forms": {"id", "entity_id", "value", "gram", "origin"},
            "global_counters": {"type", "last"},
            "projects": {"id", "ts"},
            "labels": {"project_id", "entity_id", "type", "n", "ts"},
            "counters": {"project_id", "type", "last"},
            "aliases": {"project_id", "type", "n", "entity_id"},
            "keep_terms": {"value"},
            "meta": {"key", "value"},
        }
        for table, columns in expected.items():
            got = {
                row[1]
                for row in self.vault._conn.execute(f"PRAGMA table_info({table})")
            }
            self.assertEqual(got, columns, table)

    def test_wal_and_busy_timeout(self):
        mode = self.vault._conn.execute("PRAGMA journal_mode").fetchone()[0]
        self.assertEqual(mode.lower(), "wal")
        busy = self.vault._conn.execute("PRAGMA busy_timeout").fetchone()[0]
        self.assertEqual(busy, 5000)

    def test_forms_have_origin_check(self):
        """origin — только seen/derived (TR-1.2)."""
        with self.assertRaises(sqlite3.IntegrityError):
            self.vault._conn.execute(
                "INSERT INTO forms(entity_id, value, gram, origin) VALUES(1, 'x', NULL, 'bad')"
            )


class NumberingTest(VaultBase):
    def test_global_numbering_by_type(self):
        """Уровень 0: per-type счётчики, как в прежней базе."""
        self.assertEqual(self.vault.global_label(PERSON, "PERSON"), "[PERSON_1]")
        self.assertEqual(self.vault.global_label("ООО Ромашка", "ORG"), "[ORG_1]")
        self.assertEqual(
            self.vault.global_label("Петров Пётр", "PERSON"), "[PERSON_2]")

    def test_word_forms_share_one_label(self):
        """D7/TR-2.1: падежные формы — одна сущность и одна метка."""
        first = self.vault.global_label(PERSON, "PERSON")
        second = self.vault.global_label(PERSON_DATV, "PERSON")
        self.assertEqual(first, second)
        self.assertEqual(self.vault.get_value(first), PERSON)
        forms = self.vault._conn.execute(
            "SELECT value, origin FROM forms WHERE entity_id = 1 ORDER BY id"
        ).fetchall()
        self.assertIn((PERSON, "seen"), forms)
        self.assertIn((PERSON_DATV, "seen"), forms)
        self.assertTrue(any(origin == "derived" for _value, origin in forms))

    def test_numbering_persists_across_sessions(self):
        self.vault.global_label("Сидоров Сидор", "PERSON")
        self.vault.close()
        self.vault = Vault(self.db)
        self.assertEqual(
            self.vault.global_label("Петров Пётр", "PERSON"), "[PERSON_2]")

    def test_two_connections_do_not_duplicate_numbers(self):
        other = Vault(self.db)
        try:
            a = self.vault.global_label("Значение А", "MISC")
            b = other.global_label("Значение Б", "MISC")
            self.assertNotEqual(a, b)
            self.assertEqual(b, "[MISC_2]")
        finally:
            other.close()

    def test_rev_grows_on_every_write(self):
        rev0 = self.vault.rev()
        self.vault.global_label(PERSON, "PERSON")
        rev1 = self.vault.rev()
        self.assertGreater(rev1, rev0)
        self.vault.global_label(PERSON_DATV, "PERSON")
        self.assertGreater(self.vault.rev(), rev1)


class ProjectTest(VaultBase):
    def test_projects_are_not_linked(self):
        """TR-1.5: номера меток проектов независимы друг от друга."""
        a1 = self.vault.project_label("P0001", PERSON, "PERSON")
        b1 = self.vault.project_label("P0002", "Петров Пётр", "PERSON")
        self.assertEqual(a1, "[PERSON_1]")
        self.assertEqual(b1, "[PERSON_1]")
        self.assertEqual(self.vault.project_value("P0001", a1), PERSON)
        self.assertEqual(self.vault.project_value("P0002", b1), "Петров Пётр")

    def test_project_numbers_not_derived_from_global(self):
        """Глобальные метки и id сущностей не влияют на номера проекта."""
        self.vault.global_label("Сидоров Сидор", "PERSON")
        self.vault.global_label("Петров Пётр", "PERSON")
        label = self.vault.project_label("P0001", PERSON, "PERSON")
        self.assertEqual(label, "[PERSON_1]")

    def test_case_suffix_and_roundtrip(self):
        """TR-2.3/TR-2.4: `[PERSON_1|дат]` → форма в дательном падеже."""
        label = self.vault.project_label("P0001", PERSON_DATV, "PERSON")
        self.assertEqual(label, "[PERSON_1|дат]")
        self.assertEqual(self.vault.project_value("P0001", label), PERSON_DATV)
        # именительный — без суффикса
        self.assertEqual(
            self.vault.project_label("P0001", PERSON, "PERSON"), "[PERSON_1]")

    def test_suffix_absent_in_filenames(self):
        """TR-2.3: в именах файлов и папок суффикса нет (символ | запрещён)."""
        label = self.vault.project_label(
            "P0001", PERSON_DATV, "PERSON", suffix=False)
        self.assertEqual(label, "[PERSON_1]")

    def test_unknown_suffix_falls_back_to_nominative(self):
        """TR-2.4: падеж вне `seen` — порождённая форма + «проверить»."""
        self.vault.project_label("P0001", PERSON, "PERSON")
        warnings: list[str] = []
        value = self.vault.project_value("P0001", "[PERSON_1|твор]", warnings)
        self.assertEqual(value, "Ивановым Иваном Ивановичем")
        self.assertTrue(
            any("порождённых" in w for w in warnings),
            f"должна быть пометка «проверить»: {warnings}",
        )

    def test_unparsable_case_falls_back_to_nominative(self):
        """TR-2.4: неизвестный суффикс — именительная форма + «проверить»."""
        self.vault.project_label("P0001", PERSON, "PERSON")
        warnings: list[str] = []
        value = self.vault.project_value("P0001", "[PERSON_1|зват]", warnings)
        self.assertEqual(value, PERSON)
        self.assertTrue(warnings)

    def test_tolerant_label_syntax(self):
        """TR-2.4: [person_1], [PERSON 1], [PERSON-1] распознаются."""
        self.vault.project_label("P0001", PERSON, "PERSON")
        for token in ("[person_1]", "[PERSON 1]", "[PERSON-1]"):
            self.assertEqual(
                self.vault.project_value("P0001", token), PERSON, token)

    def test_foreign_label_is_unmatched(self):
        self.vault.project_label("P0001", PERSON, "PERSON")
        mappings, unmatched, _warnings = self.vault.mappings_for_text(
            "P0001", "Контакт: [PERSON_7] и [PERSON_1]")
        self.assertEqual(mappings, {"[PERSON_1]": PERSON})
        self.assertEqual(unmatched, ["[PERSON_7]"])

    def test_mappings_for_text_resolves_suffixes(self):
        self.vault.project_label("P0001", PERSON_DATV, "PERSON")
        mappings, unmatched, _warnings = self.vault.mappings_for_text(
            "P0001", "передать [PERSON_1|дат], затем [PERSON_1]")
        self.assertEqual(unmatched, [])
        self.assertEqual(mappings["[PERSON_1|дат]"], PERSON_DATV)

    def test_merge_entities_keeps_alias(self):
        """TR-2.6: младшая метка становится псевдонимом старшей."""
        keep = self.vault.project_label("P0001", PERSON, "PERSON")
        drop = self.vault.project_label("P0001", "Иванов И. И.", "PERSON")
        self.assertNotEqual(keep, drop)
        result = self.vault.merge_entities("P0001", keep, [drop])
        self.assertEqual(result["keep"], keep)
        self.assertIn(drop, result["dropped"])
        # уже выпущенные файлы с младшей меткой деанонимизируются
        self.assertEqual(self.vault.project_value("P0001", drop), PERSON)
        # формы младшей сущности переехали в старшую
        self.assertGreaterEqual(result["forms_moved"], 1)
        labels = self.vault.project_mappings("P0001")
        self.assertNotIn(drop, labels)

    def test_merge_rejects_other_type(self):
        person = self.vault.project_label("P0001", PERSON, "PERSON")
        org = self.vault.project_label("P0001", "ООО Ромашка", "ORG")
        with self.assertRaises(vault_mod.VaultError):
            self.vault.merge_entities("P0001", person, [org])

    def test_drop_project_keeps_global_entities(self):
        label = self.vault.project_label("P0001", PERSON, "PERSON")
        self.vault.project_label("P0002", PERSON, "PERSON")
        result = self.vault.drop_project("P0001")
        self.assertEqual(result["labels"], 1)
        self.assertIsNone(self.vault.project_value("P0001", label))
        self.assertEqual(
            self.vault.project_value("P0002", "[PERSON_1]"), PERSON)
        # глобальная сущность остаётся
        self.assertEqual(
            self.vault.get_all_mappings().get("[PERSON_1]"), PERSON)

    def test_snapshot_shape(self):
        """TR-1.6: {rev, labels[{label,type,forms[{value,gram,origin}]}], others, keep}."""
        label = self.vault.project_label("P0001", PERSON, "PERSON")
        self.vault.global_label("Петров Пётр", "PERSON")  # не в проекте
        snap = self.vault.snapshot("P0001")
        self.assertEqual(set(snap), {"rev", "labels", "others", "keep"})
        self.assertEqual(snap["rev"], self.vault.rev())
        self.assertEqual(snap["labels"][0]["label"], label)
        self.assertEqual(snap["labels"][0]["type"], "PERSON")
        self.assertEqual(
            set(snap["labels"][0]["forms"][0]), {"value", "gram", "origin"})
        self.assertIn("Петров Пётр", snap["others"])
        self.assertIn("1С:ERP", snap["keep"])


class KeepTermsTest(VaultBase):
    def test_default_list_seeded(self):
        """D19/П-14: стартовый список терминов 1С загружен."""
        keep = set(self.vault.keep_list())
        for term in ("1С", "1С:ERP", "1С:ЗУП", "ЗУП", "Документооборот",
                     "ДиректБанк", "ГИС МТ", "ТЗ", "HTTP-сервис"):
            self.assertIn(term, keep)

    def test_product_abbreviations_are_kept(self):
        """Состав продуктов 1С согласован владельцем 17.11.2026."""
        for term in ("1С:БП", "БП", "1С:ЗУП", "ЗУП", "1С:УТ", "УТ", "1С:УНФ",
                     "УНФ", "1С:КА", "1С:ERP", "ЕРП", "1С:УПП", "1С:ДО",
                     "1С:УХ", "1С:ERP УХ", "1С:Розница", "1С:WMS", "1С:TMS",
                     "1С:ТОИР", "1С:ИТС", "1С:ЭДО", "1С:БСП", "1С:Fresh"):
            self.assertIn(term, self.vault.keep_list(), term)
            self.assertTrue(self.vault.is_kept(term), term)
            self.assertIsNone(self.vault.global_label(term, "ORG"), term)

    def test_short_abbrev_case_sensitive(self):
        """`cs:` — короткие аббревиатуры не «слепляются» с русскими словами."""
        self.assertTrue(self.vault.is_kept("ДО"))
        self.assertFalse(self.vault.is_kept("до"))
        self.assertTrue(self.vault.is_kept("КА"))
        self.assertFalse(self.vault.is_kept("ка"))
        # прочие короткие термины — без учёта регистра, как раньше
        self.assertTrue(self.vault.is_kept("тз"))
        self.assertTrue(self.vault.is_kept("зуп"))

    def test_composite_components_are_kept(self):
        """NER режет «1С:ERP» на «ERP» — компонент не должен получать метку."""
        for value in ("ERP", "ЗУП", "WMS", "TMS", "ИТС", "ЭДО", "БСП", "ТОИР"):
            self.assertTrue(self.vault.is_kept(value), value)

    def test_exact_and_regex_terms(self):
        self.assertTrue(self.vault.is_kept("1С:ERP"))
        self.assertTrue(self.vault.is_kept("зуп"))
        self.assertTrue(self.vault.is_kept("REQ-003"))
        self.assertTrue(self.vault.is_kept("REQ-20"))
        self.assertFalse(self.vault.is_kept("REQ-"))
        self.assertFalse(self.vault.is_kept("Иванов Иван Иванович"))

    def test_kept_value_gets_no_label(self):
        """Метка не выдаётся, сущность не создаётся."""
        self.assertIsNone(self.vault.global_label("ЗУП", "ORG"))
        self.assertIsNone(self.vault.project_label("P0001", "1С:ERP", "ORG"))
        self.assertEqual(self.vault.stats()["total"], 0)

    def test_keep_does_not_renumber_existing_labels(self):
        """Уже помеченное значение, попавшее в keep_terms, не пере-нумеруется.

        Критерий §3.3 карты: метка продолжает деанонимизироваться, но новые
        выдачи её не получают.
        """
        label = self.vault.global_label("Вектор Плюс", "ORG")
        self.assertEqual(label, "[ORG_1]")
        self.vault.add_keep_term("Вектор Плюс")
        self.assertIsNone(self.vault.global_label("Вектор Плюс", "ORG"))
        self.assertEqual(self.vault.get_value("[ORG_1]"), "Вектор Плюс")

    def test_add_keep_term(self):
        self.assertFalse(self.vault.is_kept("Вектор Плюс"))
        self.assertTrue(self.vault.add_keep_term("Вектор Плюс"))
        self.assertTrue(self.vault.is_kept("Вектор Плюс"))
        self.assertFalse(self.vault.add_keep_term("Вектор Плюс"))  # уже есть


class MigrationTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-vault-mig-")
        self.db = os.path.join(self.tmp.name, "anonymize.db")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def _make_v1(self) -> None:
        conn = sqlite3.connect(self.db)
        try:
            conn.execute(
                "CREATE TABLE mappings (token TEXT PRIMARY KEY, value TEXT NOT NULL,"
                " type TEXT NOT NULL, ts REAL NOT NULL)")
            conn.execute(
                "CREATE TABLE counters (type TEXT PRIMARY KEY,"
                " last INTEGER NOT NULL DEFAULT 0)")
            conn.execute(
                "INSERT INTO mappings VALUES('[PERSON_1]', 'Иванов Иван', 'PERSON', 1.0)")
            conn.commit()
        finally:
            conn.close()

    def test_v1_database_is_renamed_not_migrated(self):
        """TR-1.3/D4: старая база уходит в `.bak`, новая создаётся пустой."""
        self._make_v1()
        vault = Vault(self.db)
        try:
            self.assertIsNotNone(vault.backup_path)
            backup = vault.backup_path
            self.assertTrue(os.path.isfile(backup), backup)
            self.assertIn(".v1-", os.path.basename(backup))
            self.assertTrue(os.path.basename(backup).endswith(".bak"))
            # новая база пустая, схема v2
            self.assertEqual(vault.stats()["total"], 0)
            version = vault._conn.execute(
                "SELECT value FROM meta WHERE key = 'schema_version'"
            ).fetchone()[0]
            self.assertEqual(int(version), 2)
            # метки не перенесены
            self.assertIsNone(vault.get_value("[PERSON_1]"))
        finally:
            vault.close()

    def test_v2_database_reopened_without_backup(self):
        first = Vault(self.db)
        first.global_label(PERSON, "PERSON")
        first.close()
        second = Vault(self.db)
        try:
            self.assertIsNone(second.backup_path)
            self.assertEqual(second.get_value("[PERSON_1]"), PERSON)
        finally:
            second.close()

    def test_empty_file_is_initialized(self):
        open(self.db, "wb").close()
        vault = Vault(self.db)
        try:
            self.assertIsNone(vault.backup_path)
            self.assertEqual(vault.stats()["schema_version"], 2)
        finally:
            vault.close()


class KeepTermsFileTest(unittest.TestCase):
    """Файл `keep_terms.example.yaml` — тот же состав, что стартовый список."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory(prefix="hds-keep-file-")
        self.db = os.path.join(self.tmp.name, "anonymize.db")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def test_example_file_loads(self):
        from hds_anonymize import vault as vault_mod
        from hds_anonymize.worker import _load_keep_terms

        path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
            "hds_anonymize",
            "keep_terms.example.yaml",
        )
        terms = _load_keep_terms(path)
        self.assertIn("1С:БП", terms)
        self.assertIn("cs:ДО", terms)
        self.assertIn("re:REQ-\\d+", terms)
        # файл покрывает стартовый список (расширение, а не замена)
        for term in vault_mod.DEFAULT_KEEP_TERMS:
            self.assertIn(term, terms, term)
        self.assertIn("1С:Fresh", terms)

    def test_loaded_file_keeps_products(self):
        from hds_anonymize.worker import _load_keep_terms

        path = os.path.join(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
            "hds_anonymize",
            "keep_terms.example.yaml",
        )
        vault = Vault(self.db, keep_terms=_load_keep_terms(path))
        try:
            self.assertIsNone(vault.global_label("1С:УПП", "ORG"))
            self.assertIsNone(vault.global_label("1С:БСП", "ORG"))
            self.assertTrue(vault.is_kept("ЕРП"))
            self.assertFalse(vault.is_kept("Иванов Иван Иванович"))
        finally:
            vault.close()


if __name__ == "__main__":
    unittest.main()
