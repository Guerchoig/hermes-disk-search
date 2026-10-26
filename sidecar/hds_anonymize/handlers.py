"""Оркестрация файловых актов анонимизации/деанонимизации (P1).

Перенос из `anonymizer_proxy/proxy/handlers.py`, адаптация под stateless
(решения плана §1): сессий нет, reuse-копий нет, review-md в хранилище нет,
чат-команд нет — только входной и выходной файл.

Поток анонимизации (§2.1):
    parse → NER по `parsed.text` → split `\\n` → `split_entities_by_segments`
    → построчный `TextReplacer.anonymize` → join → `FileAssembler.assemble`
    (strip_hf_images=True, scrub_metadata=True + маркер) → атомарная запись.

Деанонимизация: то же, но assemble с `strip_hf_images=False,
scrub_metadata=False` и замена плейсхолдеров на значения.
"""
from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Callable, Optional

from .file_parser import (
    ANON_MARKER,
    FileAssembler,
    FileParser,
    read_anon_marker,
)
from .mapping_store import MappingStore, TOKEN_PATTERN
from .replacer import TextReplacer, split_entities_by_segments
from .schemas import Entity

# Форматы P1 (XML/MS Project — P1b; слой парсера уже готов)
FORMATS = ["docx", "xlsx", "txt", "md"]

# Маска существующих плейсхолдеров при детерминированном поиске значений
# (из anonymizer_proxy/proxy/utils.py — не переименовываем)
PLACEHOLDER_TOKEN_RE = re.compile(r"\[[A-Z_]+_\d+\]")

# Число-подобное значение из списка «скрой указанное»: цифры, разрядные
# пробелы, точка/запятая (десятичный разделитель), знак
_NUMERIC_VALUE_RE = re.compile(r"[+-]?\d[\d \u00A0.,]*")

# «Похоже на имя человека»: буквы/точки/дефис/пробелы
# (перенос из anonymizer_proxy/proxy/command_args.py)
_NAME_ITEM_RE = re.compile(r"^[A-Za-zА-Яа-яЁё][A-Za-zА-Яа-яЁё.\- ]*$")


def is_name_like(item: str) -> bool:
    """Имя → PERSON, прочее (номера, даты, суммы) → MISC."""
    return bool(_NAME_ITEM_RE.match(item))


def _token_sort_key(token: str) -> tuple[str, int]:
    """Стабильный порядок токенов: по типу, затем по номеру."""
    m = TOKEN_PATTERN.match(token)
    if m:
        return (m.group(1), int(m.group(2)))
    return (token, 0)


class UnsupportedFormatError(ValueError):
    """Формат файла не входит в SUPPORTED_EXTENSIONS парсера."""


class FileBusyError(ValueError):
    """Файл занят (открыт в Word/LibreOffice) — Windows запрещает замену."""


class ModelMissingError(RuntimeError):
    """Веса NER-моделей отсутствуют в --models-dir (offline-first)."""


def _atomic_write_bytes(path: Path, data: bytes) -> None:
    """Атомарная перезапись файла (временный файл + os.replace).

    Перенос из anonymizer_proxy/proxy/handlers.py:90 — без правок семантики.
    """
    tmp = path.with_name(path.name + ".anonymizer-tmp")
    try:
        tmp.write_bytes(data)
        os.replace(tmp, path)
    except PermissionError as exc:
        # Файл занят: открыт в Word/LibreOffice — ОС запрещает замену.
        # Текст осмысленный (не сырой PermissionError) → kind: file_busy.
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        raise FileBusyError(
            f"не удалось записать файл: занят (вероятно, открыт в "
            f"Word/LibreOffice) — {path.name}. Закройте его в редакторе "
            "и повторите операцию"
        ) from exc
    except Exception:
        # Не оставлять осиротевший tmp-файл после любого сбоя записи
        try:
            tmp.unlink(missing_ok=True)
        except OSError:
            pass
        raise


def _atomic_write_text(path: Path, text: str) -> None:
    _atomic_write_bytes(path, text.encode("utf-8"))


ProgressFn = Callable[[str, str, int, int], None]


class AnonymizeHandlers:
    """Обработчики RPC: anonymize / anonymize_values / deanonymize /
    deanonymize_values / inspect."""

    def __init__(self, store: MappingStore, ner=None) -> None:
        self.store = store
        self._ner = ner  # NERService (ленивый); None → создаётся при первом NER
        self.file_parser = FileParser()
        self.file_assembler = FileAssembler()
        self.text_replacer = TextReplacer()

    # ── NER (лениво) ────────────────────────────────────────────────────

    def _ner_service(self):
        if self._ner is None:
            from .ner_service import NERService

            self._ner = NERService()
        return self._ner

    # ── вспомогательное ─────────────────────────────────────────────────

    @staticmethod
    def _check_supported(path: Path) -> None:
        if not FileParser.is_supported(path.name):
            raise UnsupportedFormatError(
                f"Неподдерживаемый формат файла: {path.suffix or path.name} "
                f"(P1: {', '.join(FORMATS)})"
            )

    @staticmethod
    def _read_bytes(path: Path) -> bytes:
        try:
            return path.read_bytes()
        except PermissionError as exc:
            raise FileBusyError(
                f"не удалось прочитать файл (вероятно, занят): {path.name}"
            ) from exc

    async def _load_parsed(self, path: Path):
        """Прочитать и разобрать файл (parse — async в парсере)."""
        content = self._read_bytes(path)
        parsed = await self.file_parser.parse(content, path.name)
        return content, parsed

    # ── анонимизация ────────────────────────────────────────────────────

    async def anonymize(
        self,
        in_path: str,
        out_path: str,
        values: Optional[list[str]] = None,
        progress: Optional[ProgressFn] = None,
    ) -> dict:
        """Анонимизировать файл: NER (или детерминированный список `values`)
        → построчная замена → сборка (strip_hf_images=True, scrub_metadata=True)
        → атомарная запись. Уже анонимизированный вход — warning + обработка
        (решение №2 плана)."""
        path = Path(in_path)
        if not path.is_file():
            raise FileNotFoundError(f"Файл не найден: {path}")
        self._check_supported(path)
        target = Path(out_path)

        warnings: list[str] = []
        if progress:
            progress(path.name, "parse", 0, 4)

        marker = read_anon_marker(path)
        if marker:
            warnings.append(
                f"файл уже анонимизирован (маркер {marker}) — обработка продолжается"
            )

        content, parsed = await self._load_parsed(path)

        if progress:
            progress(path.name, "ner", 1, 4)

        # Для TXT/MD/XML маркера нет (§3): признак «уже анонимизирован» —
        # плейсхолдеры в тексте.
        if not marker and PLACEHOLDER_TOKEN_RE.search(parsed.text):
            warnings.append(
                "в файле есть плейсхолдеры [TYPE_N] — вероятно, файл уже "
                "анонимизирован; обработка продолжается"
            )

        if values:
            entities = self._find_values(parsed.text, values)
            matched = self._matched_values(parsed.text, values)
            unmatched = [v for v in values if v not in matched]
        else:
            entities, _, llm_failed = await self._ner_service().extract_entities_detailed(
                parsed.text, use_llm=True
            )
            if llm_failed:
                raise ModelMissingError(
                    "NER-модель не загрузилась или не вернула результат "
                    "— анонимизация файла не выполнена"
                )
            matched, unmatched = [], []

        # Анонимизируем ПОСЕГМЕНТНО: NER-сущность может пересекать границу
        # строк склеенного текста (багрепорт 2026-09-08) — перенос без правок.
        # add_mapping у TextReplacer — async (интерфейс оригинала); store
        # синхронный, оборачиваем.
        async def add_mapping(value: str, entity_type: str) -> str:
            return self.store.get_or_create_token(value, entity_type)

        segments = parsed.text.split("\n")
        segment_entities = split_entities_by_segments(entities, segments, separator="\n")
        if progress:
            progress(path.name, "replace", 2, 4)

        anon_lines: list[str] = []
        mappings = []
        for line, line_ents in zip(segments, segment_entities):
            anon_line, line_maps = await self.text_replacer.anonymize(
                line, line_ents, add_mapping
            )
            anon_lines.append(anon_line)
            mappings.extend(line_maps)
        anon_text = "\n".join(anon_lines)

        if progress:
            progress(path.name, "assemble", 3, 4)
        anon_content = await self.file_assembler.assemble(
            content, path.name, anon_text, parsed.structure,
            strip_hf_images=True, scrub_metadata=True,
        )
        _atomic_write_bytes(target, anon_content)

        # Счётчики токенов (в ответ и лог — только токены, без значений)
        token_types: dict[str, str] = {}
        token_counts: dict[str, int] = {}
        for m in mappings:
            token_types[m.token] = m.entity_type
            token_counts[m.token] = token_counts.get(m.token, 0) + 1
        tokens = [
            {"token": t, "type": token_types[t], "count": token_counts[t]}
            for t in sorted(token_counts, key=_token_sort_key)
        ]
        if progress:
            progress(path.name, "done", 4, 4)
        return {
            "out_path": str(target),
            "tokens": tokens,
            "matched": matched,
            "unmatched": unmatched,
            "warnings": warnings,
        }

    # ── детерминированный поиск значений («скрой указанное») ────────────

    @staticmethod
    def _numeric_patterns(name: str) -> list[re.Pattern]:
        """Паттерны поиска числа-подобного значения (варианты записи:
        без разрядных пробелов, запятая↔точка; поглощение дробной части).
        Перенос из anonymizer_proxy/proxy/handlers.py — без правок."""
        pats = []
        variants = {
            name,
            name.replace(" ", "").replace("\u00A0", ""),
            name.replace(",", "."),
            name.replace(" ", "").replace("\u00A0", "").replace(",", "."),
        }
        for v in variants:
            pats.append(re.compile(
                r"(?<![0-9.,])" + re.escape(v) + r"(?:[.,]\d+)?(?![0-9])"))
        return pats

    @staticmethod
    def _find_value_spans(masked: str, name: str) -> list[tuple[int, int]]:
        """Вхождения значения в замаскированный текст (регистронезависимо,
        границы слова; для чисел — варианты записи)."""
        if _NUMERIC_VALUE_RE.fullmatch(name):
            patterns = AnonymizeHandlers._numeric_patterns(name)
        else:
            patterns = [re.compile(
                r"(?<![A-Za-zА-Яа-яЁё0-9_])" + re.escape(name)
                + r"(?![A-Za-zА-Яа-яЁё0-9_])", re.IGNORECASE)]
        spans: list[tuple[int, int]] = []
        for pattern in patterns:
            for m in pattern.finditer(masked):
                spans.append((m.start(), m.end()))
        return spans

    def _find_values(self, text: str, values: list[str]) -> list[Entity]:
        """Сущности для значений из списка (порт _extra_anonymize_one_file:
        маска существующих плейсхолдеров, покрытые спаны, тип PERSON/MISC)."""
        masked = PLACEHOLDER_TOKEN_RE.sub(lambda m: " " * len(m.group(0)), text)
        entities: list[Entity] = []
        covered_spans: set[tuple[int, int]] = set()
        for name in values:
            for start, end in self._find_value_spans(masked, name):
                if any(s < end and e > start for s, e in covered_spans):
                    continue  # совпадение уже покрыто другим вариантом
                covered_spans.add((start, end))
                entities.append(Entity(
                    text=text[start:end],
                    type="PERSON" if is_name_like(name) else "MISC",
                    start=start, end=end, confidence=1.0))
        entities.sort(key=lambda e: e.start)
        return entities

    def _matched_values(self, text: str, values: list[str]) -> list[str]:
        """Какие из перечисленных значений реально найдены в тексте."""
        masked = PLACEHOLDER_TOKEN_RE.sub(lambda m: " " * len(m.group(0)), text)
        return [name for name in values if self._find_value_spans(masked, name)]

    # ── деанонимизация ──────────────────────────────────────────────────

    async def deanonymize(
        self,
        in_path: str,
        out_path: str,
        tokens: Optional[list[str]] = None,
        progress: Optional[ProgressFn] = None,
    ) -> dict:
        """Деанонимизировать файл: заменить плейсхолдеры на значения из БД
        (все или подмножество `tokens`) → сборка (strip_hf_images=False,
        scrub_metadata=False) → атомарная запись."""
        path = Path(in_path)
        if not path.is_file():
            raise FileNotFoundError(f"Файл не найден: {path}")
        self._check_supported(path)
        target = Path(out_path)

        if progress:
            progress(path.name, "parse", 0, 3)

        if tokens is None:
            mappings_dict = self.store.get_all_mappings()
            unmatched: list[str] = []
        else:
            # tokens могут прийти как «[PERSON_1]», так и «PERSON_1»
            norm = [t if t.startswith("[") else f"[{t}]" for t in tokens]
            mappings_dict, unmatched_raw = self.store.get_mappings_for_tokens(norm)
            unmatched = [tokens[i] for i, t in enumerate(norm) if t in set(unmatched_raw)]
        if not mappings_dict and tokens is None:
            raise LookupError(
                "в БД нет маппингов — деанонимизация невозможна "
                "(БД не мигрируется из anonymizer_proxy)"
            )

        content, parsed = await self._load_parsed(path)
        deanonymized_text = await self.text_replacer.deanonymize(
            parsed.text, mappings_dict
        )

        if progress:
            progress(path.name, "assemble", 1, 3)
        assembled = await self.file_assembler.assemble(
            content, path.name, deanonymized_text, parsed.structure,
            strip_hf_images=False, scrub_metadata=False,
        )

        if progress:
            progress(path.name, "write", 2, 3)
        _atomic_write_bytes(target, assembled)

        # Токены, реально встретившиеся в тексте и найденные в БД
        restored_tokens = sorted(
            {m.group(0) for m in TOKEN_PATTERN.finditer(parsed.text) if m.group(0) in mappings_dict},
            key=_token_sort_key,
        )
        if progress:
            progress(path.name, "done", 3, 3)
        return {
            "out_path": str(target),
            "restored": restored_tokens,
            "unmatched": unmatched,
            "warnings": [],
        }

    async def inspect(self, in_path: str) -> dict:
        """Разведка файла БЕЗ значений (PII не отдаём; §3 плана):
        поддержка формата, версия маркера, плейсхолдеры с типами/счётчиками,
        какие токены есть в БД."""
        path = Path(in_path)
        supported = FileParser.is_supported(path.name)
        result: dict = {
            "supported": supported,
            "marker_version": None,
            "placeholders": [],
            "resolved": [],
        }
        if not supported or not path.is_file():
            return result

        result["marker_version"] = read_anon_marker(path)
        content = self._read_bytes(path)
        parsed = await self.file_parser.parse(content, path.name)

        counts: dict[str, int] = {}
        types: dict[str, str] = {}
        for m in TOKEN_PATTERN.finditer(parsed.text):
            token = m.group(0)
            counts[token] = counts.get(token, 0) + 1
            types.setdefault(token, m.group(1))
        placeholders = [
            {"token": t, "type": types[t], "count": counts[t]}
            for t in sorted(counts, key=_token_sort_key)
        ]
        result["placeholders"] = placeholders
        if placeholders:
            found, _ = self.store.get_mappings_for_tokens(list(counts))
            result["resolved"] = [
                {"token": p["token"], "in_db": p["token"] in found}
                for p in placeholders
            ]
        return result
