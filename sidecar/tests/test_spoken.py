"""«Произнесённая форма» для CTC-aligner'а (P2.2): чистые функции, без моделей."""
from __future__ import annotations

import unittest

from hds_transcribe import spoken


class ToSpokenTest(unittest.TestCase):
    def test_numbers_become_words(self):
        self.assertEqual(spoken.to_spoken("25 %"), "двадцать пять процентов")
        self.assertEqual(spoken.to_spoken("2024"), "две тысячи двадцать четыре")
        self.assertEqual(spoken.to_spoken("1 250 000"), "один миллион двести пятьдесят тысяч")

    def test_decimal(self):
        self.assertEqual(spoken.to_spoken("12,5"), "двенадцать запятая пять")

    def test_digit_plus_letter(self):
        self.assertEqual(spoken.to_spoken("1С"), "один эс")
        self.assertEqual(spoken.to_spoken("2D"), "два дэ")

    def test_abbreviation_letter_names(self):
        self.assertEqual(spoken.to_spoken("МЧД"), "эм че дэ")

    def test_latin_words(self):
        self.assertEqual(spoken.to_spoken("Excel"), "эксель")
        self.assertEqual(spoken.to_spoken("API"), "апи")

    def test_punctuation_and_quotes_dropped(self):
        self.assertEqual(spoken.to_spoken("«Да», — сказал он."), "да сказал он")

    def test_spoken_tokens(self):
        self.assertEqual(spoken.spoken_tokens("МЧД 25 %"),
                         ["эм", "че", "дэ", "двадцать", "пять", "процентов"])

    def test_empty(self):
        self.assertEqual(spoken.to_spoken(""), "")
        self.assertEqual(spoken.spoken_tokens("..."), [])


if __name__ == "__main__":
    unittest.main()
