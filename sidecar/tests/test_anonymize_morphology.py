"""Морфология (TR-2.1…TR-2.4, правки П-2/П-12) на фикстурах спайка A0.

Фикстуры и ожидаемые падежные таблицы — те же, что в
`tools/parity/out/agent_a0/morph_spike.py` (8 персон + 3 ORG/LOC), чтобы
результат A1 сравнивался с замером A0 один в один.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize import morph  # noqa: E402

CASES = ("nomn", "gent", "datv", "accs", "ablt", "loct")

PERSONS = [
    {
        "value": "Иванов Иван Иванович",
        "expected": {
            "nomn": "Иванов Иван Иванович",
            "gent": "Иванова Ивана Ивановича",
            "datv": "Иванову Ивану Ивановичу",
            "accs": "Иванова Ивана Ивановича",
            "ablt": "Ивановым Иваном Ивановичем",
            "loct": "Иванове Иване Ивановиче",
        },
    },
    {
        "value": "Иванов И. И.",
        "expected": {
            "nomn": "Иванов И. И.",
            "gent": "Иванова И. И.",
            "datv": "Иванову И. И.",
            "accs": "Иванова И. И.",
            "ablt": "Ивановым И. И.",
            "loct": "Иванове И. И.",
        },
    },
    {
        "value": "Иванова Мария Петровна",
        "expected": {
            "nomn": "Иванова Мария Петровна",
            "gent": "Ивановой Марии Петровны",
            "datv": "Ивановой Марии Петровне",
            "accs": "Иванову Марию Петровну",
            "ablt": "Ивановой Марией Петровной",
            "loct": "Ивановой Марии Петровне",
        },
    },
    {
        "value": "Шевченко Ольга Ивановна",
        "expected": {
            "nomn": "Шевченко Ольга Ивановна",
            "gent": "Шевченко Ольги Ивановны",
            "datv": "Шевченко Ольге Ивановне",
            "accs": "Шевченко Ольгу Ивановну",
            "ablt": "Шевченко Ольгой Ивановной",
            "loct": "Шевченко Ольге Ивановне",
        },
    },
    {
        "value": "Дюма Александр",
        "expected": {
            "nomn": "Дюма Александр",
            "gent": "Дюма Александра",
            "datv": "Дюма Александру",
            "accs": "Дюма Александра",
            "ablt": "Дюма Александром",
            "loct": "Дюма Александре",
        },
    },
]

PERSONS_HARD = [
    {
        "value": "Сидоров-Амурский Пётр Николаевич",
        "expected": {
            "nomn": "Сидоров-Амурский Пётр Николаевич",
            "gent": "Сидорова-Амурского Петра Николаевича",
            "datv": "Сидорову-Амурскому Петру Николаевичу",
            "accs": "Сидорова-Амурского Петра Николаевича",
            "ablt": "Сидоровым-Амурским Петром Николаевичем",
            "loct": "Сидорове-Амурском Петре Николаевиче",
        },
    },
    {
        "value": "Белый Андрей Викторович",
        "expected": {
            "nomn": "Белый Андрей Викторович",
            "gent": "Белого Андрея Викторовича",
            "datv": "Белому Андрею Викторовичу",
            "accs": "Белого Андрея Викторовича",
            "ablt": "Белым Андреем Викторовичем",
            "loct": "Белом Андрее Викторовиче",
        },
    },
    {
        "value": "Толстой Лев Николаевич",
        "expected": {
            "nomn": "Толстой Лев Николаевич",
            "gent": "Толстого Льва Николаевича",
            "datv": "Толстому Льву Николаевичу",
            "accs": "Толстого Льва Николаевича",
            "ablt": "Толстым Львом Николаевичем",
            "loct": "Толстом Льве Николаевиче",
        },
    },
]

PLACES_ORGS = [
    {
        "value": "Тверь",
        "type": "LOC",
        "expected": {
            "nomn": "Тверь", "gent": "Твери", "datv": "Твери",
            "accs": "Тверь", "ablt": "Тверью", "loct": "Твери",
        },
    },
    {
        "value": "Орёл",
        "type": "LOC",
        "expected": {
            "nomn": "Орёл", "gent": "Орла", "datv": "Орлу",
            "accs": "Орёл", "ablt": "Орлом", "loct": "Орле",
        },
    },
    {
        "value": "Ромашка",
        "type": "ORG",
        "expected": {
            "nomn": "Ромашка", "gent": "Ромашки", "datv": "Ромашке",
            "accs": "Ромашку", "ablt": "Ромашкой", "loct": "Ромашке",
        },
    },
]


@unittest.skipUnless(morph.is_available(), "нужен pymorphy2 (venv-anonymize)")
class MorphologyTest(unittest.TestCase):
    # ── (а) norm_key: одна сущность — один ключ (TR-2.1) ────────────────

    def test_norm_key_same_for_all_cases(self):
        for item in PERSONS + PERSONS_HARD:
            keys = {
                morph.norm_key(form, "PERSON")
                for form in item["expected"].values()
            }
            self.assertEqual(len(keys), 1, f"{item['value']}: ключи {sorted(keys)}")
            self.assertNotIn("", keys)

    def test_norm_key_places_and_orgs(self):
        for item in PLACES_ORGS:
            keys = {
                morph.norm_key(form, item["type"])
                for form in item["expected"].values()
            }
            self.assertEqual(len(keys), 1, f"{item['value']}: ключи {sorted(keys)}")

    def test_norm_key_initials_and_dash_surname(self):
        """A0/П-2: инициалы с точкой; дефисная фамилия — один ключ."""
        self.assertEqual(
            morph.norm_key("Иванов И. И.", "PERSON"),
            morph.norm_key("Иванов И И", "PERSON"),
        )
        self.assertIn("и.", morph.norm_key("Иванов И. И.", "PERSON"))
        self.assertEqual(
            morph.norm_key("Сидоров-Амурский Пётр Николаевич", "PERSON"),
            morph.norm_key("Сидорова-Амурского Петра Николаевича", "PERSON"),
        )
        self.assertIn(
            "сидоров-амурский",
            morph.norm_key("Сидоров-Амурский Пётр Николаевич", "PERSON"),
        )

    def test_norm_key_adjective_surname(self):
        """П-2: «Толстой» в косвенных падежах не превращается в «толстый»."""
        key = morph.norm_key("Толстой Лев Николаевич", "PERSON")
        self.assertIn("толстой", key)
        self.assertEqual(key, morph.norm_key("Толстого Льва Николаевича", "PERSON"))

    def test_norm_key_pattern_types(self):
        """Шаблонные типы: словоформ нет — каноническая запись (TR-2.1)."""
        self.assertEqual(morph.norm_key("+7 912 345-67-89", "PHONE"), "79123456789")
        self.assertEqual(
            morph.norm_key("Ivanov@Example.RU", "EMAIL"), "ivanov@example.ru")
        self.assertEqual(morph.norm_key("ИНН 7701234567", "INN"), "7701234567")

    # ── (б) генерация всех падежей (TR-2.2) ─────────────────────────────

    def test_generation_matches_expected_table(self):
        """Генерация совпадает с таблицей A0 (в A0 — 98,5 %)."""
        failures = []
        for item in PERSONS + PERSONS_HARD:
            for case, expected in item["expected"].items():
                got = morph.inflect_value(item["value"], "PERSON", case)
                if (got or "").lower() != expected.lower():
                    failures.append((item["value"], case, expected, got))
        self.assertEqual(failures, [], "расхождения генерации падежей")

    def test_generation_single_word_org_loc(self):
        # Известное расхождение A0 (§3.9): pymorphy считает «Орёл»
        # одушевлённым, винительный даёт «Орла» вместо «Орёл»
        known = {("Орёл", "accs"): "Орла"}
        for item in PLACES_ORGS:
            for case, expected in item["expected"].items():
                got = morph.inflect_value(item["value"], item["type"], case)
                expected = known.get((item["value"], case), expected)
                self.assertEqual(
                    (got or "").lower(), expected.lower(),
                    f"{item['value']} [{case}]")

    def test_multiword_org_not_generated(self):
        """Многословные названия и кавычки не склоняются (TR-2.2)."""
        self.assertEqual(morph.generate_forms("ООО «Северный ветер»", "ORG"), [])
        self.assertEqual(morph.generate_forms("Северный ветер", "ORG"), [])


    # ── (в) ложные склейки: сущности не слипаются ───────────────────────

    def test_no_false_merges(self):
        pairs = [
            ("Иванов Иван Иванович", "PERSON", "Иванова Мария Петровна", "PERSON"),
            ("Иванов Иван Иванович", "PERSON",
             "Сидоров-Амурский Пётр Николаевич", "PERSON"),
            ("Белый Андрей Викторович", "PERSON", "Орёл", "LOC"),
            ("Северный ветер", "ORG", "ООО «Северный ветер»", "ORG"),
        ]
        for a, ta, b, tb in pairs:
            self.assertNotEqual(
                morph.norm_key(a, ta), morph.norm_key(b, tb),
                f"ложная склейка: {a} ↔ {b}",
            )

    def test_same_surname_different_initials_not_merged(self):
        self.assertNotEqual(
            morph.norm_key("Иванов И. И.", "PERSON"),
            morph.norm_key("Иванов П. П.", "PERSON"),
        )

    # ── (г) падежный суффикс: только при однозначности (TR-2.3, П-12) ───

    def test_suffix_only_when_unambiguous(self):
        """Суффикс ставится для однозначных форм и не ставится при сомнении."""
        # значение: (суффикс, падеж определён однозначно)
        cases = {
            "Иванов Иван Иванович": ("", True),      # именительный — без суффикса
            "Иванову Ивану Ивановичу": ("дат", True),
            "Ивановым Иваном Ивановичем": ("твор", True),
            "Иванове Иване Ивановиче": ("пред", True),
            "Толстому Льву Николаевичу": ("дат", True),
            # «Иванову И. И.»: род/вин у одушевлённых неразличимы — без суффикса
            "Иванову И. И.": ("", False),
            "Твери": ("", False),
            "Ромашке": ("", False),
        }
        for value, (expected, confident_expected) in cases.items():
            suffix, confident = morph.case_suffix(value, "PERSON")
            self.assertEqual(suffix, expected, f"{value}: {suffix!r}")
            self.assertEqual(confident, confident_expected, value)

    def test_gram_suffix_roundtrip(self):
        """Суффикс метки ↔ граммема базы (TR-2.3/TR-2.4)."""
        self.assertEqual(morph.gram_from_suffix("дат"), "datv")
        self.assertEqual(morph.gram_from_suffix("род,мн"), "gent,мн")
        self.assertIsNone(morph.gram_from_suffix(""))
        self.assertEqual(morph.parse_gram("gent"), ("gent", False))
        self.assertEqual(morph.parse_gram("gent,мн"), ("gent", True))

    # ── (д) обратная подстановка (TR-2.4) ───────────────────────────────

    def test_restore_returns_original_form(self):
        for item in PERSONS + PERSONS_HARD:
            value = item["value"]
            for case, expected in item["expected"].items():
                self.assertEqual(
                    (morph.inflect_value(value, "PERSON", case) or "").lower(),
                    expected.lower(),
                    f"{value} [{case}]",
                )


if __name__ == "__main__":
    unittest.main()
