"""Глоссарий: загрузка `data/glossary.yaml` и детерминированные замены (P3.1/P3.2).

Формат — PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §4.2:

```yaml
terms:
  - canonical: "1С"
    variants: ["один эс", "индинес", "1c"]
    type: product            # product | org | person | abbreviation | domain
    context: ["бухгалтерия", "ERP"]
    priority: high           # high | normal | low
    case_sensitive: false
    allow_global: false
    pronunciations: ["один эс"]
forbid: []                   # фразы, запрещённые как replacements вообще
```

Главное правило: `variants` — **кандидаты**, а не гарантия. Замена выполняется,
только если рядом (окно ±`CONTEXT_WINDOW_WORDS` слов) есть слово из `context`
**или** `allow_global: true` **или** `priority: high` (и нет конкурирующего
термина). Иначе — `needs_review`: текст **не меняется**, случай попадает в
`edits.json` для ручной проверки.

Модуль чистый (без torch/pyannote/моделей): тестируется на словарях. Он же
готовит фразы для hotwords (P3.4): `Glossary.hotword_phrases()` и проверка
лимитов gigastt 64×64 (`validate_hotwords`).

Замены применяются **по словам** (`blocks[].words[].text`), затем `block.text`
пересобирается через `merge.join_words`; таймкоды, спикеры, `alignment` и
`word_id` не меняются — иначе `.md` и `transcript.json` разъедутся (P3.2).
"""
from __future__ import annotations

import os
import unicodedata
from dataclasses import dataclass, field
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

from .errors import JobError
from .merge import join_words

#: Окно контекста: ±N слов от найденного варианта (`context` в глоссарии).
CONTEXT_WINDOW_WORDS = 6
#: Максимум слов в одном варианте («один эс» — 2, «эр пи система» — 3).
MAX_VARIANT_WORDS = 4
#: Лимиты gigastt (жёсткие, проверяются до вызова CLI/сервера).
MAX_HOTWORD_PHRASES = 64
MAX_HOTWORD_CHARS = 64
#: Допустимые типы термина (`type`); неизвестный — warning, не падение.
TERM_TYPES = ("product", "org", "person", "abbreviation", "domain")
#: Приоритеты; `high` заменяет без контекста.
PRIORITIES = ("high", "normal", "low")
_PRIORITY_RANK = {"high": 3, "normal": 2, "low": 1}

#: Причины правки (`edits.json[].reason`).
REASON_CONTEXT = "context"
REASON_ALLOW_GLOBAL = "allow_global"
REASON_PRIORITY = "priority"
REASON_NO_CONTEXT = "no_context"
#: Конкурирующие термины на одном участке — замена не применяется.
REASON_AMBIGUOUS = "ambiguous"

#: Гомоглифы → общий скелет (строчная латиница): так `1C`/`1С` и `ERP`/`ЕРП`
#: нормализуются в одно и то же (`1c`, `erp`). Направление «кириллица → латиница»
#: выбрано потому, что термины домена — латинские аббревиатуры и «1С».
_HOMOGLYPHS = str.maketrans(
    {
        "а": "a", "в": "b", "е": "e", "ё": "e", "к": "k", "м": "m", "н": "h",
        "о": "o", "р": "p", "с": "c", "т": "t", "у": "y", "х": "x",
        "і": "i", "ѕ": "s", "ј": "j",
    }
)


def _is_word_char(ch: str) -> bool:
    return ch.isalnum() or ch in "-_"


def strip_punct(text: str) -> str:
    """Снять пунктуацию по краям, сохранив внутренние дефисы/подчёркивания."""
    value = (text or "").strip()
    start, end = 0, len(value)
    while start < end and not _is_word_char(value[start]):
        start += 1
    while end > start and not _is_word_char(value[end - 1]):
        end -= 1
    return value[start:end]


def split_edge_punct(text: str) -> Tuple[str, str, str]:
    """Разделить слово на `(префикс, ядро, суффикс)`: `«1С,»` → `(«», «1С», «,»)`."""
    value = text or ""
    core = strip_punct(value)
    if not core:
        return value, "", ""
    pos = value.find(core)
    return value[:pos], core, value[pos + len(core):]


def normalize(text: str) -> str:
    """Нормализация для сопоставления: регистр, `ё/е`, латиница/кириллица.

    Пунктуация по краям отбрасывается, внутренние дефисы сохраняются.
    Канон **не** нормализуется — в текст уходит ровно то написание, что в файле.
    """
    core = strip_punct(unicodedata.normalize("NFKC", text or ""))
    core = core.lower().replace("ё", "е")
    return core.translate(_HOMOGLYPHS)


def tokens(text: str) -> List[str]:
    """Токены для многословных вариантов («один эс» → `["один", "эс"]`)."""
    core = unicodedata.normalize("NFKC", text or "")
    return [tok for tok in (normalize(part) for part in core.split()) if tok]


@dataclass
class GlossaryTerm:
    """Термин глоссария (поля — как в `glossary.yaml`, §4.2)."""

    canonical: str
    variants: List[str] = field(default_factory=list)
    type: str = ""
    context: List[str] = field(default_factory=list)
    priority: str = "normal"
    case_sensitive: bool = False
    allow_global: bool = False
    pronunciations: List[str] = field(default_factory=list)

    @property
    def norm_canonical(self) -> str:
        return normalize(self.canonical)

    @property
    def all_variants(self) -> List[str]:
        """`variants` + канон (канон тоже совпадение — для пропуска/идемпотентности)."""
        out = [self.canonical]
        seen = {normalize(self.canonical)}
        for variant in self.variants:
            key = normalize(variant)
            if key not in seen:
                seen.add(key)
                out.append(variant)
        return out

    def as_dict(self) -> Dict[str, Any]:
        return {
            "canonical": self.canonical,
            "variants": list(self.variants),
            "type": self.type,
            "context": list(self.context),
            "priority": self.priority,
            "case_sensitive": self.case_sensitive,
            "allow_global": self.allow_global,
            "pronunciations": list(self.pronunciations),
        }


@dataclass
class Glossary:
    """Набор терминов + `forbid` + предупреждения валидации."""

    terms: List[GlossaryTerm] = field(default_factory=list)
    forbid: List[str] = field(default_factory=list)
    warnings: List[str] = field(default_factory=list)
    source: str = ""

    # ------------------------------------------------------------- загрузка
    @classmethod
    def from_dict(cls, data: Any, source: str = "") -> "Glossary":
        """Собрать глоссарий из словаря/списка терминов (Rust `glossary.terms`)."""
        warnings: List[str] = []
        if isinstance(data, list):
            raw_terms, raw_forbid = data, []
        elif isinstance(data, dict):
            raw_terms = data.get("terms") or []
            raw_forbid = data.get("forbid") or []
            unknown = set(data) - {"terms", "forbid", "version", "description"}
            for key in sorted(unknown):
                warnings.append("glossary: неизвестный ключ верхнего уровня %r" % key)
        else:
            raw_terms, raw_forbid = [], []
            warnings.append("glossary: ожидался словарь или список терминов")
        terms: List[GlossaryTerm] = []
        for index, item in enumerate(raw_terms or []):
            term = _term_from_item(item, index, warnings)
            if term is None:
                continue
            if normalize(term.canonical) in {normalize(t.canonical) for t in terms}:
                warnings.append("glossary: дубликат канона %r — пропущен" % term.canonical)
                continue
            terms.append(term)
        forbid = [str(x).strip() for x in raw_forbid or [] if str(x).strip()]
        glossary = cls(terms=terms, forbid=forbid, warnings=warnings, source=source)
        glossary._drop_forbidden()
        return glossary

    @classmethod
    def load(cls, path: str, required: bool = False) -> "Glossary":
        """Прочитать `glossary.yaml`. Нет файла → пустой глоссарий (поведение P2).

        `required=True` — «путь задан явно, а файла нет»: это ошибка конфигурации,
        а не пустой глоссарий (`bad_request` + hint).
        """
        path = (path or "").strip()
        if not path:
            return cls()
        if not os.path.isfile(path):
            if required:
                raise JobError(
                    "bad_request",
                    "файл глоссария не найден: %s" % path,
                    "Проверьте transcribe.glossary.path (data/glossary.yaml) "
                    "или уберите ключ.",
                )
            return cls(warnings=["glossary: файл не найден — %s" % path], source=path)
        try:
            import yaml  # PyYAML: уже стоит в venv (зависимость pyannote/whisperx)
        except ImportError:  # pragma: no cover - окружение без PyYAML
            raise JobError(
                "bad_request",
                "нет PyYAML для чтения %s" % path,
                "Установите pyyaml в venv транскрибации (requirements.lock).",
            )
        try:
            with open(path, "r", encoding="utf-8-sig") as f:
                data = yaml.safe_load(f) or {}
        except (OSError, ValueError) as e:
            raise JobError(
                "bad_request",
                "глоссарий не читается (%s): %s" % (path, e),
                "Ожидается YAML по формату §4.2 (terms/forbid).",
            )
        return cls.from_dict(data, source=path)

    # ---------------------------------------------------------- производные
    def _drop_forbidden(self) -> None:
        """Убрать термины/варианты из `forbid` (слишком общие фразы)."""
        forbidden = {normalize(phrase) for phrase in self.forbid}
        if not forbidden:
            return
        kept: List[GlossaryTerm] = []
        for term in self.terms:
            if normalize(term.canonical) in forbidden:
                self.warnings.append(
                    "glossary: канон %r в forbid — термин не применяется" % term.canonical
                )
                continue
            blocked = [v for v in term.variants if normalize(v) in forbidden]
            if blocked:
                term.variants = [v for v in term.variants if normalize(v) not in forbidden]
                for variant in blocked:
                    self.warnings.append(
                        "glossary: вариант %r в forbid — пропущен (термин %r)"
                        % (variant, term.canonical)
                    )
            kept.append(term)
        self.terms = kept

    def hotword_phrases(
        self,
        limit: int = MAX_HOTWORD_PHRASES,
        max_chars: int = MAX_HOTWORD_CHARS,
    ) -> List[str]:
        """Фразы для hotwords: канон + `pronunciations` (что слышно).

        Gigastt бустит токены **целевой** фразы, поэтому канон нужен наравне с
        произношением. Порядок детерминирован (порядок терминов в файле),
        дубликаты убраны, лимиты 64×64 соблюдены.
        """
        out: List[str] = []
        seen = set()

        def push(value: str) -> None:
            phrase = (value or "").strip()
            if not phrase or len(phrase) > max_chars or normalize(phrase) in seen:
                return
            seen.add(normalize(phrase))
            out.append(phrase)

        for term in self.terms:
            push(term.canonical)
            for phrase in term.pronunciations:
                push(phrase)
            if len(out) >= limit:
                break
        return out[:limit]

    def stats(self) -> Dict[str, Any]:
        return {
            "path": self.source,
            "terms": len(self.terms),
            "forbid": len(self.forbid),
            "warnings": len(self.warnings),
        }

    # --------------------------------------------------------------- замены
    def apply(self, blocks: Sequence[dict], window: int = CONTEXT_WINDOW_WORDS):
        """Применить замены к блокам **на месте** (P3.2).

        Возвращает `(edits, stats)`; `blocks[].text` пересобирается через
        `merge.join_words` только для изменённых блоков. Таймкоды/спикеры/
        `alignment`/`word_id` не меняются.
        """
        edits: List[dict] = []
        applied = 0
        compiled = _compile(self.terms)
        for block_index, block in enumerate(blocks or []):
            words = block.get("words") or []
            if not words or not compiled:
                continue
            changed = _apply_block(block_index, words, compiled, window, edits)
            if changed:
                applied += changed
                block["text"] = join_words(words)
        reasons: Dict[str, int] = {}
        reviews = 0
        for edit in edits:
            reason = str(edit.get("reason") or "")
            reasons[reason] = reasons.get(reason, 0) + 1
            if edit.get("needs_review"):
                reviews += 1
        stats = {
            "terms": len(self.terms),
            "edits": len(edits),
            "applied": applied,
            "needs_review": reviews,
            "reasons": dict(sorted(reasons.items())),
        }
        return edits, stats


def _term_from_item(item: Any, index: int, warnings: List[str]) -> Optional[GlossaryTerm]:
    """Один элемент `terms:` → `GlossaryTerm` (неизвестные ключи — warning)."""
    if isinstance(item, str):
        item = {"canonical": item}
    if not isinstance(item, dict):
        warnings.append("glossary: термин #%d не словарь — пропущен" % index)
        return None
    canonical = str(item.get("canonical") or "").strip()
    if not canonical:
        warnings.append("glossary: термин #%d без canonical — пропущен" % index)
        return None
    known = {
        "canonical", "variants", "type", "context", "priority",
        "case_sensitive", "allow_global", "pronunciations",
    }
    for key in sorted(set(item) - known):
        warnings.append("glossary: %r: неизвестный ключ %r" % (canonical, key))
    priority = str(item.get("priority") or "normal").strip().lower()
    if priority not in PRIORITIES:
        warnings.append("glossary: %r: priority=%r → normal" % (canonical, priority))
        priority = "normal"
    term_type = str(item.get("type") or "").strip().lower()
    if term_type and term_type not in TERM_TYPES:
        warnings.append("glossary: %r: type=%r вне списка §4.2" % (canonical, term_type))
    return GlossaryTerm(
        canonical=canonical,
        variants=_strings(item.get("variants")),
        type=term_type,
        context=_strings(item.get("context")),
        priority=priority,
        case_sensitive=_as_bool(item.get("case_sensitive")),
        allow_global=_as_bool(item.get("allow_global")),
        pronunciations=_strings(item.get("pronunciations")),
    )


def _as_bool(value: Any) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    return str(value or "").strip().lower() in ("1", "true", "yes", "on", "да")


def _strings(value: Any) -> List[str]:
    if value is None:
        return []
    if isinstance(value, str):
        value = [value]
    return [str(x).strip() for x in value if str(x).strip()]


def _compile(terms: Iterable[GlossaryTerm]):
    """Предкомпиляция: `(term, контексты, варианты, case_sensitive)`.

    Вариант — пара `(нормализованные токены, исходные токены)`: вторые нужны для
    `case_sensitive`, где регистр варианта значим.
    """
    compiled = []
    for term in terms:
        variants: List[Tuple[List[str], List[str]]] = []
        seen = set()
        for variant in term.all_variants:
            toks = tokens(variant)
            if not toks or len(toks) > MAX_VARIANT_WORDS or tuple(toks) in seen:
                continue
            seen.add(tuple(toks))
            raw = [strip_punct(part) for part in variant.split()]
            variants.append((toks, raw))
        contexts = [tokens(phrase) for phrase in term.context]
        contexts = [ctx for ctx in contexts if ctx]
        if variants:
            compiled.append((term, contexts, variants, term.case_sensitive))
    return compiled


def _candidates(norm: Sequence[str], raw: Sequence[str], compiled):
    """Все совпадения вариантов: `(start, end, term_index, term, длина)`."""
    out = []
    for term_index, (term, _contexts, variants, case_sensitive) in enumerate(compiled):
        for toks, raw_tokens in variants:
            size = len(toks)
            for start in range(0, max(0, len(norm) - size + 1)):
                if list(norm[start:start + size]) != toks:
                    continue
                if case_sensitive:
                    actual = [strip_punct(x) for x in raw[start:start + size]]
                    if actual != raw_tokens:
                        continue
                out.append((start, start + size, term_index, term, size))
    return out


def _resolve(candidates) -> List[Tuple[int, int, GlossaryTerm, bool]]:
    """Разрешить пересечения: приоритет → длина → порядок в файле.

    Равные приоритет и длина у **разных** терминов на одном участке — `ambiguous`
    (замена не применяется, случай уходит в `needs_review`).
    """
    ranked = sorted(
        candidates,
        key=lambda c: (c[0], -_PRIORITY_RANK.get(c[3].priority, 0), -c[4], c[2]),
    )
    chosen: List[Tuple[int, int, GlossaryTerm, bool]] = []
    for start, end, _index, term, _size in ranked:
        overlap = [c for c in chosen if start < c[1] and end > c[0]]
        if overlap:
            other = overlap[0]
            same_span = other[0] == start and other[1] == end
            same_rank = (
                _PRIORITY_RANK.get(other[2].priority, 0)
                == _PRIORITY_RANK.get(term.priority, 0)
            )
            if same_rank and other[2].canonical != term.canonical:
                # Конкурирующие термины одного приоритета: замену не применяем,
                # оба кандидата уходят в `needs_review` (§4.2). При совпадении
                # span достаточно одной записи, иначе показываем обе границы.
                chosen[chosen.index(other)] = (other[0], other[1], other[2], True)
                if not same_span:
                    chosen.append((start, end, term, True))
            continue
        chosen.append((start, end, term, False))
    return sorted(chosen, key=lambda c: c[0])


def _already_canonical(before_words: Sequence[str], term: GlossaryTerm) -> bool:
    """Текст **уже ровно канон** — правку не пишем (идемпотентность).

    Сравнение по символам, а не по нормализованной форме: иначе вариант
    «зуп» считался бы «уже ЗУП» (нормализация у них общая) и замена
    регистра/написания никогда бы не применялась.
    """
    joined = " ".join(part for part in (strip_punct(x) for x in before_words) if part)
    return joined == term.canonical


def _decide(
    term: GlossaryTerm,
    norm: Sequence[str],
    start: int,
    end: int,
    window: int,
) -> Tuple[bool, str]:
    """Применять ли замену и с какой причиной (§4.2)."""
    if term.context and _context_hit(term.context, norm, start, end, window):
        return True, REASON_CONTEXT
    if term.allow_global:
        return True, REASON_ALLOW_GLOBAL
    if term.priority == "high":
        return True, REASON_PRIORITY
    return False, REASON_NO_CONTEXT


def _token_hit(window_token: str, phrase_token: str) -> bool:
    """Совпадение слова контекста: точное или грубый стемминг (≥5 общих символов).

    Глоссарий задаётся словарной формой («бухгалтерия»), а в тексте слово стоит
    в падеже («бухгалтерии»). Стемминг **детерминированный** (общий префикс,
    не меньше 5 символов и не меньше 70 % длины) — никакой морфологии/моделей.
    """
    if window_token == phrase_token:
        return True
    if len(window_token) < 5 or len(phrase_token) < 5:
        return False
    prefix = 0
    for left, right in zip(window_token, phrase_token):
        if left != right:
            break
        prefix += 1
    return prefix >= 5 and prefix >= 0.7 * min(len(window_token), len(phrase_token))


def _context_hit(
    context: Sequence[str],
    norm: Sequence[str],
    start: int,
    end: int,
    window: int,
) -> bool:
    """Есть ли фраза из `context` в окне ±`window` (само совпадение не считаем)."""
    lo = max(0, start - window)
    hi = min(len(norm), end + window)
    around = list(norm[lo:start]) + list(norm[end:hi])
    if not around:
        return False
    for phrase in context:
        toks = tokens(phrase)
        if not toks:
            continue
        size = len(toks)
        for pos in range(0, max(0, len(around) - size + 1)):
            if all(_token_hit(around[pos + i], toks[i]) for i in range(size)):
                return True
    return False


def _edit(
    block_index: int,
    words: Sequence[dict],
    start: int,
    end: int,
    before: str,
    term: str,
    reason: str,
    needs_review: bool,
) -> dict:
    """Запись правки для `edits.json` (P3.2): `word_id` первого слова."""
    first = words[start] if start < len(words) else {}
    last = words[end - 1] if 0 <= end - 1 < len(words) else {}
    return {
        "block_index": block_index,
        "word_id": first.get("word_id"),
        "before": before,
        "after": term,
        "term": term,
        "reason": reason,
        "needs_review": bool(needs_review),
        "span": [start, end],
        "start": first.get("start"),
        "end": last.get("end"),
    }


def _apply_block(
    block_index: int,
    words: List[dict],
    compiled,
    window: int,
    edits: List[dict],
) -> int:
    """Замены внутри одного блока; возвращает число применённых правок."""
    raw = [str(w.get("text") or "") for w in words]
    norm = [normalize(text) for text in raw]
    chosen = _resolve(_candidates(norm, raw, compiled))
    changed = 0
    for start, end, term, ambiguous in chosen:
        before_words = raw[start:end]
        before = " ".join(part for part in before_words if part).strip()
        if ambiguous:
            edits.append(
                _edit(block_index, words, start, end, before, term.canonical,
                      REASON_AMBIGUOUS, True)
            )
            continue
        if _already_canonical(before_words, term):
            continue
        applied, reason = _decide(term, norm, start, end, window)
        if not applied:
            edits.append(
                _edit(block_index, words, start, end, before, term.canonical, reason, True)
            )
            continue
        prefix, _, _ = split_edge_punct(raw[start])
        _, _, suffix = split_edge_punct(raw[end - 1])
        words[start]["text"] = prefix + term.canonical + suffix
        for pos in range(start + 1, end):
            words[pos]["text"] = ""
        for pos in range(start, end):
            norm[pos] = normalize(words[pos]["text"])
        edits.append(
            _edit(block_index, words, start, end, before, term.canonical, reason, False)
        )
        changed += 1
    return changed


def validate_hotwords(
    phrases: Sequence[str],
    max_phrases: int = MAX_HOTWORD_PHRASES,
    max_phrase_chars: int = MAX_HOTWORD_CHARS,
) -> List[str]:
    """Проверить фразы hotwords до вызова gigastt (лимиты 64 × 64).

    Ошибка — `bad_request` с hint, где названы машинные коды gigastt
    (`too_many_hotwords` / `hotword_phrase_too_long`), чтобы Rust и журнал
    показывали причину без чтения stderr CLI.
    """
    clean = [(phrase or "").strip() for phrase in phrases or []]
    clean = [phrase for phrase in clean if phrase]
    if max_phrases and len(clean) > max_phrases:
        raise JobError(
            "bad_request",
            "hotwords: %d фраз > лимита %d (too_many_hotwords)" % (len(clean), max_phrases),
            "Сократите transcribe.hotwords.phrases или глоссарий (pronunciations): "
            "gigastt принимает не больше %d фраз." % max_phrases,
        )
    for phrase in clean:
        if max_phrase_chars and len(phrase) > max_phrase_chars:
            raise JobError(
                "bad_request",
                "hotwords: фраза длиннее %d символов (hotword_phrase_too_long): %r"
                % (max_phrase_chars, phrase),
                "Сократите фразу до %d символов — иначе gigastt её молча отбросит."
                % max_phrase_chars,
            )
    return clean


def write_hotwords_file(path: str, phrases: Sequence[str]) -> str:
    """Записать hotword-файл **UTF-8 без BOM** (иначе gigastt молча всё отбросит)."""
    directory = os.path.dirname(os.path.abspath(path))
    if directory:
        os.makedirs(directory, exist_ok=True)
    body = "".join("%s\n" % phrase for phrase in phrases)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(body)
    return path
