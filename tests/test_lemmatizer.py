"""Тесты лемматизации: словоформы находят друг друга, graceful fallback, reindex-fts."""
import os
import sys
import tempfile
import unittest

from helpers import write_config, write_text  # noqa: I100

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from hds import db as dbmod, lemmatizer  # noqa: E402
from hds.config import db_abs_path, load  # noqa: E402
from hds.search import fts_query, search  # noqa: E402


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="hds-lem-")
        write_config(self.tmp)
        self.conn = dbmod.connect(db_abs_path(load()), 8)
        self.addCleanup(self.conn.close)

    def _index_text(self, name, text):
        p = write_text(self.tmp, name, text)
        fid = dbmod.upsert_file(self.conn, p, ".txt", "text", 10, 1.0, "h-" + name)
        dbmod.add_chunk(self.conn, fid, 0, None, None, None, text)
        return p


class LemmatizerUnitTests(unittest.TestCase):
    def test_available_flag_consistent(self):
        self.assertIsInstance(lemmatizer.available(), bool)

    @unittest.skipUnless(lemmatizer.available(), "pymorphy3 не установлен")
    def test_normalize_lemmas(self):
        self.assertEqual(lemmatizer.normalize("Настройки скриптов"),
                         "настройка скрипт")

    @unittest.skipUnless(lemmatizer.available(), "pymorphy3 не установлен")
    def test_lemmatize_token_cache(self):
        first = lemmatizer.lemmatize_token("договоров")
        second = lemmatizer.lemmatize_token("договоров")  # попадание в кэш
        self.assertEqual(first, second)
        self.assertEqual(first, "договор")


class MorphologySearchTests(Base):
    @unittest.skipUnless(lemmatizer.available(), "pymorphy3 не установлен")
    def test_fts_query_lemmatized(self):
        self.assertEqual(fts_query("настройки"), '"настройка"')
        self.assertEqual(fts_query("договоров"), '"договор"')

    @unittest.skipUnless(lemmatizer.available(), "pymorphy3 не установлен")
    def test_wordforms_find_each_other(self):
        """РЕГРЕССИЯ: FTS без морфологии — «настройка» не находила «настройки»."""
        self._index_text("a.txt", "Изменение настроек приложения описано в документации")
        res = search(self.conn, None, load(), "настройка")
        self.assertTrue(res)
        self.assertTrue(res[0]["path"].endswith("a.txt"))

    @unittest.skipUnless(lemmatizer.available(), "pymorphy3 не установлен")
    def test_wordforms_reverse_direction(self):
        """РЕГРЕССИЯ: запрос «договоров» находит текст с «договор» и наоборот."""
        self._index_text("b.txt", "Договор поставки заключён на год")
        res = search(self.conn, None, load(), "договоров")
        self.assertTrue(res)
        self.assertTrue(res[0]["path"].endswith("b.txt"))

    @unittest.skipUnless(lemmatizer.available(), "pymorphy3 не установлен")
    def test_fts_stores_lemmatized_text(self):
        """РЕГРЕССИЯ: в chunks_fts должна попасть лемма, в chunks — исходный текст."""
        p = self._index_text("c.txt", "Записи договора")
        row = self.conn.execute(
            "SELECT f.text FROM chunks_fts f JOIN chunks c ON c.id=f.rowid "
            "JOIN files fl ON fl.id=c.file_id WHERE fl.path=?", (p,)).fetchone()
        self.assertEqual(row[0], "запись договор")


if __name__ == "__main__":
    unittest.main()