"""Настройки sidecar-анонимизатора — замена `anonymizer_proxy/config.py`.

`config.yaml` воркер НЕ читает (контракт sidecar §5.2): все параметры приходят
аргументами командной строки от Rust-клиента (`worker.py` вызывает
`configure()` до первого использования движков). Сетевые провайдеры/ключи
из оригинального конфига не переносятся — их функции запрещены (решение
заказчика: без OpenAI-прокси и перехвата команд).
"""
from __future__ import annotations

import logging
import sys
from pathlib import Path

# ── Логирование (замена config.setup_logging) ─────────────────────────────
# Всё — в stderr: stdout принадлежит протоколу JSON-RPC (hds_sidecar.stdio).


def _build_logger() -> logging.Logger:
    logger = logging.getLogger("hds_anonymize")
    if not logger.handlers:
        handler = logging.StreamHandler(sys.stderr)
        handler.setFormatter(
            logging.Formatter(
                "%(asctime)s [%(levelname)s] %(name)s: %(message)s",
                datefmt="%Y-%m-%d %H:%M:%S",
            )
        )
        logger.addHandler(handler)
    logger.setLevel(logging.INFO)
    logger.propagate = False
    return logger


logger = _build_logger()

# Каталог весов по умолчанию (fallback, если воркер запущен без --models-dir):
# <repo>/models/anonymize — как `<models>/anonymize` из плана.
MODELS_FALLBACK_DIR = Path(__file__).resolve().parents[2] / "models" / "anonymize"

# NER-движок (GLiNER) — значения по умолчанию как в anonymizer_proxy
# (.env по умолчанию / config.yaml §2.6). `configure()` перезаписывает
# из аргументов воркера.
NER_ENGINE: dict = {
    "model": "knowledgator/gliner-pii-large-v1.0",
    "threshold": 0.3,
    "backend": "auto",  # auto|onnx — ONNX Runtime (CUDA → DirectML → CPU); torch — фоллбек
    "device": "cpu",
    # Окно модели — 768 токенов: 1500 символов гарантируют, что чанк не
    # выходит за окно (при больших значениях сущности в конце чанка терялись).
    "max_input_chars": 1500,
    # Перекрытие соседних чанков: защищает сущности на границе чанков.
    "chunk_overlap_chars": 200,
    # Таймаут NER-вызова (страховочный, применяется в GlinerEngine.predict)
    "timeout": 300.0,
    # Второй контур — Natasha/Slovnet: русский NER + yargy-ФИО
    "natasha": True,
    # Каталог весов Natasha/Slovnet и HF-кэша GLiNER (--models-dir)
    "models_dir": str(MODELS_FALLBACK_DIR),
    # Лимит памяти ORT-сессии (P3: lease у llm_host; в P1 принимается и хранится)
    "gpu_mem_limit_mb": 0,
}

# Категории PII для детекции (как в anonymizer_proxy/config.py)
PII_CATEGORIES = {
    "PERSON": "Имена и фамилии сотрудников",
    "SURNAME": "Фамилии (в том числе с инициалами, без имени)",
    "POSITION": "Должности (ген. директор, гл. бухгалтер)",
    "DEPARTMENT": "Подразделения (отдел кадров, цех №5)",
    "ORG": "Названия компаний и юрлиц",
    "LOC": "Географические названия (города, адреса)",
    "PRODUCT": "Названия продуктов и систем",
    "PASSPORT": "Паспортные данные (серия, номер, кем выдан)",
    "PHONE": "Телефоны",
    "EMAIL": "Email адреса",
    "WEB": "Адреса веб-сайтов (URL, домены)",
    "INN": "ИНН/ОГРН/КПП",
    "MONEY": "Суммы денег",
}


def configure(
    *,
    model: str | None = None,
    threshold: float | None = None,
    max_input_chars: int | None = None,
    chunk_overlap: int | None = None,
    timeout: float | None = None,
    natasha: bool | None = None,
    models_dir: str | None = None,
    device: str | None = None,
    gpu_mem_limit_mb: int | None = None,
) -> None:
    """Применить аргументы воркера к NER_ENGINE (вызывается один раз при старте)."""
    if model:
        NER_ENGINE["model"] = model
    if threshold is not None:
        NER_ENGINE["threshold"] = float(threshold)
    if max_input_chars is not None:
        NER_ENGINE["max_input_chars"] = int(max_input_chars)
    if chunk_overlap is not None:
        NER_ENGINE["chunk_overlap_chars"] = int(chunk_overlap)
    if timeout is not None:
        NER_ENGINE["timeout"] = float(timeout)
    if natasha is not None:
        NER_ENGINE["natasha"] = bool(natasha)
    if models_dir:
        NER_ENGINE["models_dir"] = models_dir
    if device:
        NER_ENGINE["device"] = device
    if gpu_mem_limit_mb:
        NER_ENGINE["gpu_mem_limit_mb"] = int(gpu_mem_limit_mb)
