"""Схемы данных sidecar-анонимизатора.

Замена pydantic-моделей `anonymizer_proxy/models/schemas.py`: в окружении
sidecar (`sidecar/python`) pydantic не ставится, а нужны только два типа —
`Entity` (результат NER) и `MappingEntry` (токен ↔ значение). Поля и
конструктор по ключевым словам — 1:1 с оригиналом (копии модулей
`replacer.py`/`ner_service.py` создают их без изменений).
"""
from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime


@dataclass
class Entity:
    """Обнаруженная сущность в тексте."""

    text: str
    type: str  # PERSON, ORG, ...
    start: int
    end: int
    confidence: float = 1.0


@dataclass
class MappingEntry:
    """Запись маппинга токен ↔ оригинальное значение."""

    token: str  # например [PERSON_1]
    original_value: str
    entity_type: str
    session_id: str = ""  # сессий нет (решение №1); поле для совместимости
    created_at: datetime = field(default_factory=datetime.now)
