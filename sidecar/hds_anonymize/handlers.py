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
    strip_body_images,
)
from .name_fixer import fix_glued_full_names
from .office_ops import OfficeError, office_create, office_edit, office_read
from .replacer import TextReplacer, split_entities_by_segments
from .schemas import Entity
from .text_image import (
    ExtractRequired,
    build_text_image,
    extract_text,
    image_path as text_image_path,
    write_text_image,
)
from .vault import LABEL_RE, MAX_WARNINGS, Vault

# Форматы P1 + XML/MS Project (P1b; слой парсера уже готов)
FORMATS = ["docx", "xlsx", "txt", "md", "xml"]

# Маска существующих плейсхолдеров при детерминированном поиске значений
# (из anonymizer_proxy/proxy/utils.py — не переименовываем; с v2 — с суффиксом)
PLACEHOLDER_TOKEN_RE = LABEL_RE

# Число-подобное значение из списка «скрой указанное»: цифры, разрядные
# пробелы, точка/запятая (десятичный разделитель), знак
_NUMERIC_VALUE_RE = re.compile(r"[+-]?\d[\d \u00A0.,]*")

# «Похоже на имя человека»: буквы/точки/дефис/пробелы
# (перенос из anonymizer_proxy/proxy/command_args.py)
_NAME_ITEM_RE = re.compile(r"^[A-Za-zА-Яа-яЁё][A-Za-zА-Яа-яЁё.\- ]*$")

# Имя файла/папки в Windows (TR-7.1): `|` появляется от падежных меток,
# остальные символы — из исходных имён; управляющие тоже вырезаем. Разделители
# пути (`\`, `/`) в имени компонента недопустимы — они режут путь.
_INVALID_NAME_RE = re.compile(r'[<>:"/\\|?*\x00-\x1f]')
_PATH_SPLIT_RE = re.compile(r"[\\/]+")


def sanitize_component(name: str) -> str:
    """Санитайзинг компонента пути после подстановки меток (TR-7.1).

    Убирает недопустимые Windows-символы, схлопывает пробелы, снимает
    хвостовые точки/пробелы (Windows их молча отбрасывает — имя в реестре
    разошлось бы с файлом). Пустой результат заменяется `_`, чтобы компонент
    не исчез из пути.
    """
    cleaned = _INVALID_NAME_RE.sub("", name or "")
    cleaned = re.sub(r"\s+", " ", cleaned).strip()
    cleaned = cleaned.rstrip(". ")
    return cleaned or "_"



def is_name_like(item: str) -> bool:
    """Имя → PERSON, прочее (номера, даты, суммы) → MISC."""
    return bool(_NAME_ITEM_RE.match(item))


def _token_sort_key(token: str) -> tuple[str, int]:
    """Стабильный порядок токенов: по типу, затем по номеру."""
    m = LABEL_RE.match(token)
    if m:
        return (m.group(1), int(m.group(2)))
    return (token, 0)


class UnsupportedFormatError(ValueError):
    """Формат файла не входит в SUPPORTED_EXTENSIONS парсера."""


class FileBusyError(ValueError):
    """Файл занят (открыт в Word/LibreOffice) — Windows запрещает замену."""


class ModelMissingError(RuntimeError):
    """Веса NER-моделей отсутствуют в --models-dir (offline-first)."""


class OutOfDirError(ValueError):
    """Выходной путь выходит за разрешённый каталог (`..`, absolute-подмена)
    — kind: out_of_dir (§3; основная валидация — на стороне Rust)."""


def _atomic_write_bytes(path: Path, data: bytes) -> None:
    """Атомарная перезапись файла (временный файл + os.replace).

    Перенос из anonymizer_proxy/proxy/handlers.py:90 — без правок семантики.
    Дополнение P1: родительский каталог создаётся (выходной out_dir может
    ещё не существовать — `data/anonymized` при первом акте).
    """
    path.parent.mkdir(parents=True, exist_ok=True)
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
    """Обработчики RPC: файловые акты, `anonymize_text`, база меток,
    Office-операции и текстовые образы (TR-1.7, TR-3.5, TR-7.4)."""

    def __init__(self, store: Vault, ner=None) -> None:
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
        project_id: Optional[str] = None,
        mode: str = "classic",
        artifact_id: Optional[str] = None,
        images_mode: str = "strip",
    ) -> dict:
        """Анонимизировать файл: NER (или детерминированный список `values`)
        → построчная замена → сборка (strip_hf_images=True, scrub_metadata=True)
        → атомарная запись. Уже анонимизированный вход — warning + обработка
        (решение №2 плана).

        `mode="agent"` (TR-7.4): метки проекта с падежными суффиксами,
        картинки тела DOCX и картинки листов XLSX заменяются `[IMAGE_n]`,
        метаданные чистятся, в свойства файла пишется маркер
        `hds-agent/<P>/<artifact_id>`, **оригинал не меняется** (в т.ч.
        исправление «слипшихся» ФИО — только в образе).
        """
        agent = mode == "agent"
        images_mode = (images_mode or "strip").strip().lower()
        if images_mode not in ("strip", "keep"):
            images_mode = "strip"
        if agent and not (project_id or "").strip():
            raise ValueError("mode=agent требует project_id")
        path = Path(in_path)
        if not path.is_file():
            raise FileNotFoundError(f"Файл не найден: {path}")
        self._check_supported(path)
        target = Path(out_path)

        warnings: list[str] = []
        if progress:
            progress(path.name, "parse", 0, 4)

        marker = read_anon_marker(path)
        if marker and not agent:
            warnings.append(
                f"файл уже анонимизирован (маркер {marker}) — обработка продолжается"
            )

        # Режим agent: картинки тела вырезаются ДО парсинга, на их месте —
        # метки [IMAGE_n], поэтому текст и структура остаются согласованными.
        # `agent.images: keep` оставляет картинки (не рекомендуется: риск утечки).
        content = self._read_bytes(path)
        images = 0
        if agent and images_mode == "strip":
            content, images = strip_body_images(content, path.name)
            if images:
                warnings.append(
                    f"картинки тела заменены метками [IMAGE_n]: {images}"
                )
        parsed = await self.file_parser.parse(content, path.name)

        # «Слипшиеся» ФИО («ВолодинВ.В.») чинятся ДО NER: без границы
        # фамилия/инициалы модель и regex-слой фамилию не видят.
        # В classic исправления вносятся и в исходный файл (требование
        # владельца); в режиме agent оригинал не меняется никогда (TR-7.4) —
        # правка живёт только в образе.
        fixed_text, name_fixes = fix_glued_full_names(parsed.text)
        if name_fixes:
            parsed.text = fixed_text
            if agent:
                warnings.append(
                    f"исправлено «слипшихся» ФИО: {name_fixes} — только в образе "
                    "(оригинал не изменён)"
                )
            else:
                fixed_content = await self.file_assembler.assemble(
                    content, path.name, fixed_text, parsed.structure,
                    strip_hf_images=False, scrub_metadata=False,
                )
                _atomic_write_bytes(path, fixed_content)
                warnings.append(
                    f"исправлено «слипшихся» ФИО: {name_fixes} — "
                    "исходный файл обновлён до анонимизации"
                )

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

        # D19: keep_terms не получают меток вовсе (список «не анонимизировать»)
        skipped = 0
        filtered: list[Entity] = []
        for entity in entities:
            if self.store.is_kept(entity.text):
                skipped += 1
                continue
            filtered.append(entity)
        if skipped:
            warnings.append(
                f"оставлено без метки (keep_terms): {skipped} вхождений"
            )
        entities = filtered

        # Анонимизируем ПОСЕГМЕНТНО: NER-сущность может пересекать границу
        # строк склеенного текста (багрепорт 2026-09-08) — перенос без правок.
        # add_mapping у TextReplacer — async (интерфейс оригинала); store
        # синхронный, оборачиваем.
        verify: list[str] = []

        async def add_mapping(value: str, entity_type: str) -> str:
            if agent:
                label, confident = self.store.project_label_ex(
                    project_id, value, entity_type
                )
                if label is None:
                    return value  # keep_terms (страховка: уже отфильтровано)
                if not confident:
                    verify.append(label)
                return label
            token = self.store.global_label(value, entity_type)
            return token or value

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
        marker_value = None
        if agent:
            marker_value = f"hds-agent/{project_id}"
            if artifact_id:
                marker_value = f"{marker_value}/{artifact_id}"
        anon_content = await self.file_assembler.assemble(
            content, path.name, anon_text, parsed.structure,
            strip_hf_images=True, scrub_metadata=True,
            marker=marker_value,
        )
        _atomic_write_bytes(target, anon_content)

        if verify:
            warnings.append(
                "падеж не определён однозначно у меток: "
                + ", ".join(sorted(set(verify))[:MAX_WARNINGS])
                + " — помечены «проверить»"
            )

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
            "mode": mode,
            "project_id": project_id,
            "images": images,
        }

    def label_for_value(
        self, project_id: str, value: str, entity_type: str, suffix: bool = True
    ) -> dict:
        """«Скрыть ещё…»: метка проекта для значения (TR-1.7)."""
        label, confident = self.store.project_label_ex(
            project_id, value, entity_type, suffix=suffix
        )
        return {
            "project_id": project_id,
            "value_type": entity_type,
            "label": label,
            "kept": label is None,
            "confident": confident,
        }

    # ── база меток: снимок и операции (TR-1.6/1.7, TR-2.6) ──────────────

    def vault_snapshot(self, project_id: str) -> dict:
        return self.store.snapshot(project_id)

    def vault_rev(self) -> dict:
        return {"rev": self.store.rev()}

    def merge_entities(self, project_id: str, keep: str, drop: list[str]) -> dict:
        return self.store.merge_entities(project_id, keep, drop)

    def vault_drop_project(self, project_id: str) -> dict:
        return self.store.drop_project(project_id)

    def keep_terms_list(self) -> dict:
        return {"keep": self.store.keep_list()}

    def keep_term_add(self, value: str) -> dict:
        return {"value": value, "added": self.store.add_keep_term(value)}

    def vault_stats(self) -> dict:
        return self.store.stats()

    # ── Office-документы (TR-3.5) ───────────────────────────────────────

    def office_read(self, path: str) -> dict:
        return office_read(path)

    def office_edit(self, path: str, ops: list[dict]) -> dict:
        return office_edit(path, ops)

    def office_create(self, path: str, kind: str, content: dict) -> dict:
        return office_create(path, kind, content)

    # ── текстовые образы pdf/pptx/mpp (D18) ─────────────────────────────

    async def text_image(
        self,
        in_path: str,
        out_path: Optional[str] = None,
        out_dir: Optional[str] = None,
        project_id: str = "",
        artifact_id: Optional[str] = None,
        text: Optional[str] = None,
        ner: bool = True,
    ) -> dict:
        """Собрать текстовый образ `<имя>.<ext>.md` (D18, TR-7.3a).

        `text` — готовый текст от `hds_extract` (Rust): у `mpp` другого пути
        нет; без `text` воркер извлекает сам (`pymupdf`/`python-pptx` или
        портативный CPython). Текст анонимизируется `anonymize_text` с
        метками проекта, файл помечается `hds-agent/<P>/<artifact_id>`.
        """
        project_id = (project_id or "").strip()
        if not project_id:
            raise ValueError("text_image: не задан project_id")
        source = Path(in_path)
        if not source.is_file():
            raise FileNotFoundError(f"Файл не найден: {source}")
        if out_path:
            target = Path(out_path)
        else:
            target = text_image_path(out_dir or str(source.parent), in_path)

        if text is None:
            text = extract_text(str(source))
        anonymized = await self.anonymize_text(project_id, text, use_llm=ner)
        body = build_text_image(
            anonymized["text"], in_path, project_id, artifact_id
        )
        write_text_image(str(target), body)
        warnings = list(anonymized["warnings"])
        warnings.append(
            "образ только для чтения (правки — новым документом, TR-7.3a)"
        )
        return {
            "out_path": str(target),
            "labels": anonymized["labels"],
            "project_id": project_id,
            "artifact_id": artifact_id,
            "marker": body.splitlines()[0],
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
        project_id: Optional[str] = None,
    ) -> dict:
        """Деанонимизировать файл: заменить плейсхолдеры на значения из БД
        (все или подмножество `tokens`) → сборка (strip_hf_images=False,
        scrub_metadata=False) → атомарная запись.

        `project_id` — подстановка меток проекта с падежными суффиксами
        (TR-2.4): «[PERSON_3|дат]» → форма в дательном падеже; неизвестные
        метки уходят в `unmatched`, сомнительные падежи — в `warnings`.
        """
        path = Path(in_path)
        if not path.is_file():
            raise FileNotFoundError(f"Файл не найден: {path}")
        self._check_supported(path)
        target = Path(out_path)

        if progress:
            progress(path.name, "parse", 0, 3)

        content, parsed = await self._load_parsed(path)
        warnings: list[str] = []

        if project_id:
            mappings_dict, unmatched, notes = self.store.mappings_for_text(
                project_id, parsed.text, tokens
            )
            if notes:
                warnings.append(
                    "проверить при подстановке: "
                    + "; ".join(sorted(set(notes))[:MAX_WARNINGS])
                )
        elif tokens is None:
            mappings_dict = self.store.get_all_mappings()
            unmatched = []
        else:
            # tokens могут прийти как «[PERSON_1]», так и «PERSON_1»
            norm = [t if t.startswith("[") else f"[{t}]" for t in tokens]
            mappings_dict, unmatched_raw = self.store.get_mappings_for_tokens(norm)
            unmatched = [tokens[i] for i, t in enumerate(norm) if t in set(unmatched_raw)]
        if not mappings_dict and tokens is None and not project_id:
            raise LookupError(
                "в БД нет маппингов — деанонимизация невозможна "
                "(БД не мигрируется из anonymizer_proxy)"
            )

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
            {m.group(0) for m in LABEL_RE.finditer(parsed.text) if m.group(0) in mappings_dict},
            key=_token_sort_key,
        )
        if progress:
            progress(path.name, "done", 3, 3)
        return {
            "out_path": str(target),
            "restored": restored_tokens,
            "unmatched": unmatched,
            "warnings": warnings,
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
        if not supported:
            return result
        if not path.is_file():
            raise FileNotFoundError(f"Файл не найден: {path}")

        result["marker_version"] = read_anon_marker(path)
        content = self._read_bytes(path)
        parsed = await self.file_parser.parse(content, path.name)

        counts: dict[str, int] = {}
        types: dict[str, str] = {}
        for m in LABEL_RE.finditer(parsed.text):
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

    # ── текст: реплики и вложения (TR-1.7) ──────────────────────────────

    async def anonymize_text(
        self,
        project_id: str,
        text: str,
        use_llm: bool = True,
        suffix: bool = True,
    ) -> dict:
        """Анонимизировать произвольный текст метками проекта (TR-1.7).

        Реплики и вложения чата, а также текстовые образы: NER + regex +
        известные формы проекта (`project_label_ex` переиспользует сущность
        по `norm_key`) + `keep_terms`. Метки — с падежными суффиксами, если
        падеж определён однозначно; иначе «проверить» (TR-2.4).

        `suffix=False` — метки без падежного суффикса: так `anonymize_path`
        готовит имена файлов (`|` в имени Windows запрещён, TR-7.1).
        """
        project_id = (project_id or "").strip()
        if not project_id:
            raise ValueError("anonymize_text: не задан project_id")
        text = text or ""
        entities, _, llm_failed = await self._ner_service().extract_entities_detailed(
            text, use_llm=use_llm
        )
        if llm_failed:
            raise ModelMissingError(
                "NER-модель не загрузилась — анонимизация текста не выполнена"
            )
        kept = [e for e in entities if self.store.is_kept(e.text)]
        entities = [e for e in entities if not self.store.is_kept(e.text)]

        verify: list[str] = []

        async def add_mapping(value: str, entity_type: str) -> str:
            label, confident = self.store.project_label_ex(
                project_id, value, entity_type, suffix=suffix
            )
            if label is None:
                return value
            if not confident:
                verify.append(label)
            return label

        segments = text.split("\n")
        segment_entities = split_entities_by_segments(entities, segments)
        anon_lines: list[str] = []
        mappings = []
        for line, line_ents in zip(segments, segment_entities):
            anon_line, line_maps = await self.text_replacer.anonymize(
                line, line_ents, add_mapping
            )
            anon_lines.append(anon_line)
            mappings.extend(line_maps)
        anon_text = "\n".join(anon_lines)

        token_types: dict[str, str] = {}
        token_counts: dict[str, int] = {}
        for entry in mappings:
            token_types[entry.token] = entry.entity_type
            token_counts[entry.token] = token_counts.get(entry.token, 0) + 1
        labels = [
            {"label": token, "type": token_types[token], "count": token_counts[token]}
            for token in sorted(token_counts, key=_token_sort_key)
        ]
        warnings = []
        if kept:
            warnings.append(f"оставлено без метки (keep_terms): {len(kept)}")
        if verify:
            warnings.append(
                "падеж не определён однозначно: "
                + ", ".join(sorted(set(verify))[:MAX_WARNINGS])
            )
        return {
            "text": anon_text,
            "labels": labels,
            "project_id": project_id,
            "warnings": warnings,
        }

    # ── имена файлов и папок (TR-7.1, A4.2) ─────────────────────────────

    async def anonymize_path(
        self,
        project_id: str,
        path: str,
        suffix: bool = False,
        ner: bool = True,
    ) -> dict:
        """Анонимизировать относительный путь: `Протоколы\\Встреча.docx`.

        NER по **компонентам** (на короткой строке он слабее, чем на тексте,
        но `project_label_ex` переиспользует уже известные формы проекта),
        метки — без падежного суффикса: `|` в имени Windows запрещён, поэтому
        `suffix=true` — ошибка, а не молчаливая порча имени (TR-7.1).

        Возвращает `path` (тот же разделитель, что во входе), `components`
        (Rust собирает путь своим разделителем) и `labels`.
        """
        project_id = (project_id or "").strip()
        if not project_id:
            raise ValueError("anonymize_path: не задан project_id")
        raw = (path or "").strip()
        if not raw:
            raise ValueError("anonymize_path: пустой path")
        if suffix:
            raise ValueError(
                "anonymize_path: suffix=true недопустим — `|` в имени файла "
                "запрещён Windows (TR-7.1)"
            )
        parts = [p for p in _PATH_SPLIT_RE.split(raw) if p not in ("", ".")]
        out: list[str] = []
        labels: dict[str, dict] = {}
        for index, part in enumerate(parts):
            if part == "..":
                # `..` не должен появляться (путь строит демон); не трогаем,
                # чтобы не превратить его в имя и не «спрятать» выход из папки.
                out.append(part)
                continue
            last = index == len(parts) - 1
            stem, ext = os.path.splitext(part) if last else (part, "")
            if not stem:
                out.append(sanitize_component(part))
                continue
            res = await self.anonymize_text(
                project_id, stem, use_llm=ner, suffix=False
            )
            out.append(sanitize_component(res["text"]) + ext)
            for item in res["labels"]:
                known = labels.get(item["label"])
                if known is None:
                    labels[item["label"]] = dict(item)
                else:
                    known["count"] = known.get("count", 0) + item.get("count", 0)
        sep = "\\" if "\\" in raw else "/"
        return {
            "path": sep.join(out),
            "components": out,
            "labels": list(labels.values()),
            "project_id": project_id,
        }

