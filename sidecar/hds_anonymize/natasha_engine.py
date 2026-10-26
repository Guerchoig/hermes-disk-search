"""
Natasha/Slovnet NER-движок — специализированный детектор русского текста.

Второй контур детекции PII рядом с GLiNER (см. gliner_engine.py). GLiNER-PII
обучен в основном на английском и пропускает русские ФИО (особенно формат
«Фамилия И.О.» в подписях документов); Slovnet NER обучен на русском
(F1 PER ~0.97 на Collection5/factRuEval-2016), а yargy-NamesExtractor из
Natasha детерминированно извлекает русские ФИО по грамматикам.

Два источника сущностей, объединяемые внутри движка:
- NewsNERTagger (Slovnet): PER -> PERSON, ORG -> ORG, LOC -> LOC;
- NamesExtractor (yargy + pymorphy2): ФИО во всех формах, в том числе
  «Фамилия И.О.», «И.О. Фамилия» -> PERSON.

Модели компактны (Slovnet NER ~2 МБ, Navec ~50 МБ) и скачиваются один раз
в data/models/; дальше работа офлайн. Инференс только на NumPy, без torch.
"""

import asyncio
import logging
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from .schemas import Entity
from .settings import NER_ENGINE, MODELS_FALLBACK_DIR

logger = logging.getLogger("hds_anonymize.natasha_engine")

# Маппинг типов Slovnet -> категории проекта
_TYPE_MAP = {
    "PER": "PERSON",
    "ORG": "ORG",
    "LOC": "LOC",
}


class NatashaEngine:
    """Ленивая обёртка над Natasha/Slovnet (второй NER-контур)."""

    def __init__(self, models_dir: str | None = None) -> None:
        self._models_dir = Path(
            models_dir or NER_ENGINE.get("models_dir") or MODELS_FALLBACK_DIR
        )
        self._timeout: float = float(NER_ENGINE.get("timeout", 300))
        self._segmenter = None
        self._ner_tagger = None
        self._names_extractor = None
        self._doc_cls = None
        self._executor = ThreadPoolExecutor(
            max_workers=1, thread_name_prefix="natasha"
        )
        self._loaded: bool = False
        self._load_error: str | None = None

    # ── загрузка ────────────────────────────────────────────────────────

    def _ensure_downloaded(self) -> tuple[Path, Path]:
        """Проверить наличие весов (offline-first: скачивания НЕТ).

        Канонические URL дистрибутива (для справки; воркер в сеть не ходит):
        https://storage.yandexcloud.net/natasha-navec/packs/
            navec_news_v1_1B_250K_300d_100q.tar
        https://storage.yandexcloud.net/natasha-slovnet/packs/slovnet_ner_news_v1.tar
        Веса ставит установщик (`installers/fetch_anonymize_models.ps1`) в
        `--models-dir`; нет весов — FileNotFoundError (воркер отдаст
        kind: model_missing).
        """
        navec_path = self._models_dir / "navec_news_v1_1B_250K_300d_100q.tar"
        ner_path = self._models_dir / "slovnet_ner_news_v1.tar"
        for path in (navec_path, ner_path):
            if not path.is_file() or path.stat().st_size == 0:
                raise FileNotFoundError(
                    f"Нет весов Natasha/Slovnet: {path}. "
                    "Установите модели: installers/fetch_anonymize_models.ps1"
                )
        return navec_path, ner_path

    def _load(self) -> None:
        try:
            from natasha import (
                Doc,
                MorphVocab,
                NamesExtractor,
                NewsEmbedding,
                NewsNERTagger,
                Segmenter,
            )
            from navec import Navec
            from slovnet import NER as SlovnetNER
        except ImportError as exc:
            self._load_error = str(exc)
            raise ImportError(
                f"Пакеты natasha/slovnet/navec не установлены ({exc}). "
                "Установите: pip install natasha navec"
            ) from exc

        navec_path, ner_path = self._ensure_downloaded()

        navec = Navec.load(str(navec_path))
        ner_model = SlovnetNER.load(str(ner_path))
        ner_model.navec(navec)

        # NewsEmbedding в natasha 1.6 принимает ПУТЬ к .tar с весами Navec
        emb = NewsEmbedding(str(navec_path))
        self._segmenter = Segmenter()
        self._ner_tagger = NewsNERTagger(emb)
        self._names_extractor = NamesExtractor(MorphVocab())
        self._doc_cls = Doc
        self._loaded = True
        self._load_error = None
        logger.info("Natasha/Slovnet загружены (модели: %s)", self._models_dir)

    def _ensure_loaded(self) -> None:
        if not self._loaded:
            self._load()

    # ── инференс ────────────────────────────────────────────────────────

    def _predict(self, text: str) -> list[Entity]:
        """Синхронный инференс (вызывается в executor-потоке)."""
        self._ensure_loaded()
        entities: list[Entity] = []
        seen: set[tuple[int, int]] = set()

        def _add(start: int, stop: int, category: str, confidence: float) -> None:
            # Спаны Slovnet иногда захватывают хвост следующей строки
            # («Нестеркин Ю.В.\nТел») — обрезаем по первому переносу,
            # чтобы не «съедать» служебные слова документа
            nl = text.find("\n", start, stop)
            if nl != -1:
                stop = nl
            key = (start, stop)
            ent_text = text[start:stop]
            if key in seen or not ent_text.strip():
                return
            seen.add(key)
            entities.append(Entity(
                text=ent_text,
                type=category,
                start=start,
                end=stop,
                confidence=confidence,
            ))

        # 1) Slovnet NER: PER/ORG/LOC по контексту
        doc = self._doc_cls(text)
        doc.segment(self._segmenter)
        doc.tag_ner(self._ner_tagger)
        for span in doc.spans:
            category = _TYPE_MAP.get(span.type)
            if category:
                _add(span.start, span.stop, category, 0.9)

        # 2) yargy-NamesExtractor: детерминированные русские ФИО
        for match in self._names_extractor(text):
            span = getattr(match, "span", None)
            if span is not None:
                _add(span.start, span.stop, "PERSON", 0.95)

        entities.sort(key=lambda e: e.start)
        return entities

    async def predict(self, text: str, categories=None) -> list[Entity]:
        """
        Асинхронный инференс: синхронный вызов в executor-потоке.
        Параметр categories принят для совместимости с GlinerEngine.predict
        (у Natasha фиксированная таксономия).
        """
        loop = asyncio.get_running_loop()
        future = loop.run_in_executor(self._executor, self._predict, text)
        if self._timeout and self._timeout > 0:
            try:
                return await asyncio.wait_for(future, timeout=self._timeout)
            except asyncio.TimeoutError as exc:
                raise TimeoutError(
                    f"Natasha-инференс не уложился в {self._timeout:.0f} с"
                ) from exc
        return await future

    async def warmup(self) -> None:
        """Загрузить модели и веса (скачать при первом запуске)."""
        await asyncio.get_running_loop().run_in_executor(
            self._executor, self._ensure_loaded
        )

    def is_available(self) -> bool:
        return self._loaded

    def describe(self) -> str:
        if not self._loaded:
            return "не загружен"
        return "slovnet-ner + names-extractor"

    async def close(self) -> None:
        self._executor.shutdown(wait=False)
        self._loaded = False
