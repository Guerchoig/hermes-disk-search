"""Опциональные категории скрытия (--hide-dates/--hide-numbers/--hide-software/
--hide-departments): по умолчанию все ОТКРЫТЫ; флаг включается в аргументах
воркера и распространяется И на regex-слой, И на модельный (GLiNER).

NER-модели не нужны: движок подменяется заглушкой, regex-слой — реальный.

Запуск: python -m unittest discover -s sidecar/tests -t sidecar -v
"""
from __future__ import annotations

import asyncio
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds_anonymize import settings  # noqa: E402
from hds_anonymize.ner_service import NERService  # noqa: E402
from hds_anonymize.schemas import Entity  # noqa: E402


class StubEngine:
    """Заглушка GLiNER: отдаёт заданные сущности, запоминает categories."""

    def __init__(self, entities=None):
        self.entities = list(entities or [])
        self.last_categories = None

    async def predict(self, text, categories):
        self.last_categories = list(categories)
        return list(self.entities)

    async def close(self):
        pass


def make_service(engine_entities=None) -> tuple[NERService, StubEngine]:
    svc = NERService()
    stub = StubEngine(engine_entities)
    svc._engine = stub
    svc._natasha = None
    return svc, stub


def ent(text_all: str, value: str, etype: str) -> Entity:
    """Сущность с корректными оффсетами вхождения value в text_all."""
    start = text_all.find(value)
    assert start != -1, value
    return Entity(text=value, type=etype, start=start,
                  end=start + len(value), confidence=0.9)


def run_extract(svc: NERService, text: str) -> list:
    ents, _, failed = asyncio.run(svc.extract_entities_detailed(text, use_llm=True))
    assert not failed
    return ents


class DefaultsOpenTest(unittest.TestCase):
    """По умолчанию все опциональные категории открыты."""

    def setUp(self):
        settings.configure(hide_dates=False, hide_numbers=False,
                           hide_software=False, hide_departments=False)

    def test_dates_and_numbers_not_detected_by_regex(self):
        svc, _ = make_service()
        text = "Отчёт от 12.05.2023: объём 7 432 480 штук."
        self.assertEqual(run_extract(svc, text), [])

    def test_regex_money_without_currency_skipped(self):
        svc, _ = make_service()
        # «7 432 480» — сгруппированные разряды без валюты: раньше
        # маскировался MONEY-паттерном всегда, теперь (числа открыты)
        # пост-фильтр его снимает
        ents = run_extract(svc, "Итого: 7 432 480 единиц")
        self.assertEqual(ents, [])

    def test_money_with_currency_still_masked(self):
        svc, _ = make_service()
        ents = svc._extract_regex_entities("Итого: 51 903,14 руб.")
        self.assertTrue(any(e.type == "MONEY" for e in ents))

    def test_model_product_department_filtered_out(self):
        text = "Внедрена 1С:Предприятие в отделе кадров."
        svc, stub = make_service([
            ent(text, "1С:Предприятие", "PRODUCT"),
            ent(text, "отделе кадров", "DEPARTMENT"),
        ])
        self.assertEqual(run_extract(svc, text), [])
        # категории вообще не запрашивались у модели
        self.assertNotIn("PRODUCT", stub.last_categories)
        self.assertNotIn("DEPARTMENT", stub.last_categories)

    def test_model_bare_money_and_dates_dropped(self):
        # модель вешает «money amount» на любые числа и даты — без флагов
        # они остаются открытыми
        text = "Сумма 7432480 на 12.05.2023, всего 51 903,14 руб."
        svc, _ = make_service([
            ent(text, "7432480", "MONEY"),
            ent(text, "12.05.2023", "MONEY"),
            ent(text, "51 903,14 руб.", "MONEY"),
        ])
        kept = run_extract(svc, text)
        self.assertEqual([e.text for e in kept], ["51 903,14 руб."])


class HideEnabledTest(unittest.TestCase):
    """Включённые флаги скрывают категории (regex + модель)."""

    def test_hide_dates(self):
        settings.configure(hide_dates=True, hide_numbers=False,
                           hide_software=False, hide_departments=False)
        svc, stub = make_service()
        text = "Отчёт от 12.05.2023 и от 3 января 2024 г."
        ents = run_extract(svc, text)
        texts = {(e.type, e.text) for e in ents}
        self.assertIn(("DATE", "12.05.2023"), texts)
        self.assertTrue(
            any(e.type == "DATE" and "января 2024" in e.text for e in ents))
        # метка даты уходила в модель
        self.assertIn("DATE", stub.last_categories)

    def test_hide_numbers(self):
        settings.configure(hide_dates=False, hide_numbers=True,
                           hide_software=False, hide_departments=False)
        svc, _ = make_service()
        text = "Объём 7 432 480 штук."
        ents = run_extract(svc, text)
        # сгруппированное число замаскировано (MONEY-паттерн снова активен)
        self.assertTrue(
            any(e.type == "MONEY" and "7 432 480" in e.text for e in ents),
            ents)
        # и модельные «просто числа» теперь скрываются
        text2 = "Количество 7432480."
        svc2, _ = make_service([ent(text2, "7432480", "MONEY")])
        self.assertTrue(any(e.type == "MONEY" for e in run_extract(svc2, text2)))

    def test_hide_numbers_masks_plain_numbers(self):
        settings.configure(hide_dates=False, hide_numbers=True,
                           hide_software=False, hide_departments=False)
        svc, stub = make_service()
        ents = run_extract(svc, "Партия 256 штук.")
        self.assertTrue(any(e.type == "NUMBER" and e.text == "256" for e in ents),
                        ents)
        self.assertIn("NUMBER", stub.last_categories)

    def test_hide_software(self):
        settings.configure(hide_dates=False, hide_numbers=False,
                           hide_software=True, hide_departments=False)
        text = "Внедрена 1С:Предприятие."
        svc, stub = make_service([ent(text, "1С:Предприятие", "PRODUCT")])
        kept = run_extract(svc, text)
        self.assertTrue(any(e.type == "PRODUCT" for e in kept))
        self.assertIn("PRODUCT", stub.last_categories)

    def test_hide_departments(self):
        settings.configure(hide_dates=False, hide_numbers=False,
                           hide_software=False, hide_departments=True)
        text = "Приказ по отделу кадров."
        svc, stub = make_service([ent(text, "отделу кадров", "DEPARTMENT")])
        kept = run_extract(svc, text)
        self.assertTrue(any(e.type == "DEPARTMENT" for e in kept))
        self.assertIn("DEPARTMENT", stub.last_categories)

    def test_dates_remain_open_when_numbers_hidden(self):
        settings.configure(hide_dates=False, hide_numbers=True,
                           hide_software=False, hide_departments=False)
        svc, _ = make_service()
        # дата не должна быть раздроблена NUMBER-паттерном и не маскируется
        ents = run_extract(svc, "Отчёт от 12.05.2023 готов.")
        self.assertEqual(ents, [])


class SettingsMappingTest(unittest.TestCase):
    """Флаги воркера → NER_ENGINE["hide_categories"]."""

    def tearDown(self):
        settings.configure(hide_dates=False, hide_numbers=False,
                           hide_software=False, hide_departments=False)

    def test_flags_map_to_categories(self):
        settings.configure(hide_dates=True, hide_numbers=True,
                           hide_software=True, hide_departments=True)
        self.assertEqual(settings.hidden_categories(),
                         {"DATE", "NUMBER", "PRODUCT", "DEPARTMENT"})

    def test_defaults_are_empty(self):
        settings.configure()
        self.assertEqual(settings.hidden_categories(), set())


if __name__ == "__main__":
    unittest.main()

