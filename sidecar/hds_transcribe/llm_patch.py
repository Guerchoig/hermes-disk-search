"""LLM-постобработка (P4.1–P4.4): кандидаты → `llm_host` → валидатор → правки.

Слой идёт **после** детерминированного глоссария (`glossary.py`) и работает
только если `transcribe.llm.enabled: true`. Инференс живёт в `llm_host`
(`POST /v1/chat/completions`, OpenAI-совместимый), Python **не загружает** LLM —
VRAM не складывается (§4.6). Модуль чистый: без torch/pyannote, тестируется на
словарях и фейковом HTTP-сервере.

Главный принцип: LLM **не переписывает текст**, а предлагает JSON-патчи

```json
{"edits": [{"word_ids": [128, 129], "replacement": "1С",
            "reason": "глоссарий + контекст ERP", "needs_review": true,
            "allow_global": false}]}
```

Валидатор (без LLM) принимает или отклоняет патч **целиком**:
* `word_ids` существуют, лежат в одном блоке и у одного спикера;
* не меняются `speaker`/`start`/`end`/`alignment` (формат их не содержит, а
  попытка передать их в патче — отказ `metadata_change`);
* не исчезают отрицания и не меняются числа/даты (`negation_lost`,
  `numbers_changed`);
* длина замены ≤ 3× длины фрагмента (кроме канонов глоссария);
* замена обоснована контекстом термина глоссария либо `allow_global: true`
  (`no_glossary_context`);
* патчи не пересекаются с применёнными глоссарием/другими принятыми патчами;
* не больше `llm.max_edits_per_1000` правок на 1000 слов (`edit_limit`).

Недоступный `llm_host`, таймаут и мусор в ответе — **деградация**: warning в
журнал, результат как без LLM, задание не падает (гейт P4).
"""
from __future__ import annotations

import difflib
import gc
import json
import math
import re
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Dict, List, Optional, Sequence, Tuple

from . import glossary as glossary_mod
from .merge import join_words

#: Сколько слов контекста показывать LLM слева/справа от подозрительного слова.
CONTEXT_BEFORE = 6
CONTEXT_AFTER = 6
#: Максимум слов в одном окне (промпт не растёт квадратично).
MAX_WINDOW_WORDS = 25
#: Окон в одном HTTP-запросе и запросов на файл (ограничение стоимости/латентности).
MAX_WINDOWS_PER_REQUEST = 10
MAX_REQUESTS = 8
#: Предел роста длины замены (кроме канонов глоссария).
MAX_REPLACEMENT_RATIO = 3
#: Минимальная доля токенов исходного фрагмента в замене (кроме канонов глоссария).
MIN_REPLACEMENT_TOKEN_RATIO = 0.5
#: Слово считается содержательным, если оно длиннее (нормализованно) этого порога.
CONTENT_WORD_MIN_CHARS = 5
#: Сходство, при котором слово считается сохранённым в замене (опечатка/форма).
CONTENT_WORD_KEEP_RATIO = 0.80
#: Минимальное сходство исходного фрагмента с вариантом глоссария, при котором
#: каноническая замена (`replacement` содержит канон термина) разрешена **без**
#: контекста (решение владельца 23.10.2026, вариант C).
#:
#: Пример: `RP-система` ↔ вариант `rp система` = 0,90 → правка
#: `RP-система → ERP-система` проходит; `слайдик` ↔ лучший вариант = 0,29 →
#: замена `слайдик → слайд ERP` блокируется, хотя формально содержит канон.
VARIANT_FUZZY_MIN = 0.80

#: Флаги alignment, из-за которых слово считается подозрительным.
SUSPICIOUS_FLAGS = ("low_confidence", "failed")

#: Отрицания: исчезновение любого из них в замене — отказ (`negation_lost`).
NEGATIONS = ("не", "нет", "без", "ни", "нельзя", "никогда", "никто", "ничего", "нету")

#: Числительные словами (сравниваем вместе с цифрами: «пять» → «шесть» — изменение).
_NUMBER_WORDS = (
    "ноль", "один", "одна", "одно", "два", "две", "три", "четыре", "пять",
    "шесть", "семь", "восемь", "девять", "десять", "одиннадцать", "двенадцать",
    "тринадцать", "четырнадцать", "пятнадцать", "шестнадцать", "семнадцать",
    "восемнадцать", "девятнадцать", "двадцать", "тридцать", "сорок", "пятьдесят",
    "шестьдесят", "семьдесят", "восемьдесят", "девяносто", "сто", "двести",
    "триста", "четыреста", "пятьсот", "шестьсот", "семьсот", "восемьсот",
    "девятьсот", "тысяча", "тысячи", "тысяч", "миллион", "миллиона", "миллионов",
    "первый", "второй", "третий", "четвертый", "пятый", "шестой", "седьмой",
    "восьмой", "девятый", "десятый",
)
_NUMBER_WORD_SET = {glossary_mod.normalize(word) for word in _NUMBER_WORDS}
#: Отрицания в нормализованной форме (`glossary.normalize` меняет гомоглифы:
#: «не» совпадает с латинским «he» в скелете — сравнивать нужно одно и то же).
_NEGATION_SET = {glossary_mod.normalize(word) for word in NEGATIONS}
_DIGITS_RE = re.compile(r"\d+(?:[.,]\d+)*")
_CYR_RE = re.compile(r"[а-яё]", re.IGNORECASE)
_LAT_RE = re.compile(r"[a-z]", re.IGNORECASE)
_TOKEN_RE = re.compile(r"\S+", re.UNICODE)

#: Системный промпт: только JSON-патчи, без переписывания текста.
SYSTEM_PROMPT = (
    "Ты — корректор расшифровок русской речи (ASR). Тебе дают окна транскрипта: "
    "слова с идентификаторами word_id, спикера и подозрительные слова. "
    "Верни СТРОГО один JSON-объект без пояснений и markdown:\n"
    '{"edits": [{"word_ids": [1, 2], "replacement": "текст", '
    '"reason": "кратко", "needs_review": false, "allow_global": false}]}\n'
    "Правила:\n"
    "1. Меняй только явные ошибки распознавания в границах указанных word_ids "
    "(термины, аббревиатуры, имена из глоссария, ослышки). НЕ перефразируй, не "
    "сокращай и не добавляй смысл.\n"
    "2. НЕЛЬЗЯ менять числа, даты, суммы, отрицания (не/нет/без/ни) и имена "
    "собственные без явной ошибки распознавания. Если сомневаешься — "
    "needs_review: true.\n"
    "3. replacement — только текст замены для этих слов, без word_id и кавычек. "
    "Длина — не больше трёх длин исходного фрагмента.\n"
    "4. allow_global: true ставь, если исправление — каноническое написание "
    "термина из glossary (replacement содержит canonical, например canonical "
    "«ERP» → «ERP-система» для «RP-система»); иначе allow_global: false.\n"
    "5. word_ids должны идти подряд и принадлежать одному окну. Если правок нет — "
    'верни {"edits": []}. Не предлагай замену, если текст уже верный (в том числе '
    "если меняется только регистр букв у корректной аббревиатуры).\n"
    "6. Никаких других полей и текста вокруг JSON."
)


# --------------------------------------------------------------------- модель


@dataclass
class Window:
    """Окно кандидата: блок + диапазон позиций слов (включительно/exclusive)."""

    block_index: int
    start: int
    end: int
    reasons: List[str]


# ------------------------------------------------------------------ кандидаты


def _pattern_reason(text: str) -> str:
    """Подозрительный шаблон слова (кандидат для LLM): смешанные алфавиты, цифры.

    Это **не** решение о замене: только повод показать слово модели. Валидатор
    потом решает, можно ли патч применять.
    """
    core = glossary_mod.strip_punct(text or "")
    if not core:
        return ""
    if _CYR_RE.search(core) and _LAT_RE.search(core):
        return "mixed_script"
    if re.search(r"\d", core) and _CYR_RE.search(core):
        return "digit_in_word"
    return ""


def _close_to_variant(text: str, variant_norms: Sequence[str]) -> bool:
    """Похоже на вариант глоссария (опечатка ASR): кандидат, не замена."""
    core = glossary_mod.normalize(text)
    if len(core) < 4:
        return False
    for variant in variant_norms:
        if not variant or len(variant) < 4:
            continue
        if abs(len(core) - len(variant)) > 2:
            continue
        if difflib.SequenceMatcher(None, core, variant).ratio() >= 0.75:
            return True
    return False


def _variant_norms(glossary) -> List[str]:
    out: List[str] = []
    for term in glossary.terms:
        for variant in term.all_variants:
            norm = glossary_mod.normalize(variant)
            if norm and norm not in out:
                out.append(norm)
    return out


def candidate_windows(blocks, glossary, review_spans=None) -> List[Window]:
    """Окна подозрительных фрагментов: флаги, глоссарий, паттерны.

    `review_spans` — `{(block_index, pos)}` из `needs_review`-правок глоссария:
    спорный случай тоже показываем модели (она может разрешить неоднозначность),
    но её патч всё равно проходит валидатор.
    """
    review_spans = review_spans or set()
    variants = _variant_norms(glossary)
    by_block: Dict[int, List[Window]] = {}
    for block_index, block in enumerate(blocks or []):
        words = block.get("words") or []
        candidates: Dict[int, List[str]] = {}
        for pos, word in enumerate(words):
            if not (word.get("text") or "").strip():
                continue
            reasons: List[str] = []
            flag = str(word.get("alignment") or "")
            if flag in SUSPICIOUS_FLAGS:
                reasons.append(flag)
            if (block_index, pos) in review_spans:
                reasons.append("glossary_review")
            pattern = _pattern_reason(word.get("text") or "")
            if pattern:
                reasons.append(pattern)
            elif variants and _close_to_variant(word.get("text") or "", variants):
                reasons.append("glossary_like")
            if reasons:
                candidates[pos] = reasons
        if not candidates:
            continue
        windows: List[Window] = []
        for pos in sorted(candidates):
            start = max(0, pos - CONTEXT_BEFORE)
            end = min(len(words), pos + CONTEXT_AFTER + 1)
            if end - start > MAX_WINDOW_WORDS:
                start = max(0, min(pos - CONTEXT_BEFORE, end - MAX_WINDOW_WORDS))
            merged = False
            for window in windows:
                if start <= window.end and end >= window.start:
                    window.start = min(window.start, start)
                    window.end = max(window.end, end)
                    for reason in candidates[pos]:
                        if reason not in window.reasons:
                            window.reasons.append(reason)
                    merged = True
                    break
            if not merged:
                windows.append(Window(block_index, start, end, list(candidates[pos])))
        for window in windows:
            if window.end - window.start > MAX_WINDOW_WORDS:
                window.end = window.start + MAX_WINDOW_WORDS
            window.reasons = sorted(set(window.reasons))
        by_block[block_index] = windows
    out: List[Window] = []
    for block_index in sorted(by_block):
        out.extend(by_block[block_index])
    return out



# -------------------------------------------------------------------- промпт


def _glossary_brief(glossary) -> List[dict]:
    brief = []
    for term in glossary.terms[:20]:
        item = {"canonical": term.canonical}
        if term.variants:
            item["variants"] = list(term.variants[:8])
        if term.context:
            item["context"] = list(term.context[:6])
        if term.allow_global or term.priority == "high":
            item["global"] = True
        brief.append(item)
    return brief


def _window_payload(block, block_index: int, window: Window) -> dict:
    words = block.get("words") or []
    span = words[window.start:window.end]
    return {
        "block": block_index,
        "speaker": block.get("speaker"),
        "start": block.get("start"),
        "end": block.get("end"),
        "words": [
            {"id": word.get("word_id"), "w": word.get("text") or ""}
            for word in span
        ],
        "suspicious": window.reasons,
    }


def build_messages(blocks, glossary, windows: Sequence[Window],
                   applied_word_ids) -> List[dict]:
    """Сообщения для `/v1/chat/completions` (system + user с JSON-окнами)."""
    payload = {
        "task": "исправь только явные ошибки ASR в указанных окнах",
        "glossary": _glossary_brief(glossary),
        "already_applied_word_ids": sorted(applied_word_ids),
        "windows": [
            _window_payload(blocks[window.block_index], window.block_index, window)
            for window in windows
        ],
    }
    user = (
        "Окна транскрипта (JSON). Подозрительные слова помечены в suspicious; "
        "меняй только явные ошибки, только в границах word_ids.\n"
        + json.dumps(payload, ensure_ascii=False)
    )
    return [
        {"role": "system", "content": SYSTEM_PROMPT},
        {"role": "user", "content": user},
    ]


# ---------------------------------------------------------------- HTTP-клиент


def _ms(started: float) -> float:
    return round((time.perf_counter() - started) * 1000.0, 1)


def _post_chat(params, messages: Sequence[dict]) -> Tuple[Optional[str], Optional[str], float]:
    """Один запрос к `llm_host`: `(content, error, latency_ms)`."""
    url = "%s/chat/completions" % (params.llm_base_url or "").rstrip("/")
    body = {
        "model": params.llm_model or "chat",
        # Постобработка детерминирована (§4.6 п. 6): temperature всегда 0.
        "temperature": 0,
        "max_tokens": int(params.llm_max_tokens or 2048),
        "stream": False,
        "messages": list(messages),
    }
    data = json.dumps(body, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json; charset=utf-8"},
        method="POST",
    )
    started = time.perf_counter()
    try:
        with urllib.request.urlopen(
            request, timeout=float(params.llm_timeout_sec or 300.0)
        ) as resp:
            raw = resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        detail = ""
        try:
            detail = e.read().decode("utf-8", "replace")[:300]
        except Exception:  # noqa: BLE001 - тело ошибки не обязательно
            pass
        return None, ("HTTP %s %s" % (e.code, detail)).strip(), _ms(started)
    except urllib.error.URLError as e:
        return None, "нет соединения с llm_host (%s)" % (e.reason,), _ms(started)
    except (TimeoutError, OSError) as e:
        return None, "таймаут/сеть llm_host (%s)" % (e,), _ms(started)
    latency = _ms(started)
    try:
        payload = json.loads(raw)
    except ValueError:
        return None, "ответ llm_host не JSON", latency
    content = _content_of(payload)
    if content is None:
        return None, "в ответе llm_host нет choices[0].message.content", latency
    return content, None, latency


def _content_of(payload) -> Optional[str]:
    """Текст ответа: `choices[0].message.content`, `choices[0].text` и `content`."""
    if not isinstance(payload, dict):
        return None
    choices = payload.get("choices")
    if isinstance(choices, list) and choices:
        first = choices[0]
        if isinstance(first, dict):
            message = first.get("message")
            if isinstance(message, dict) and isinstance(message.get("content"), str):
                return message["content"]
            for key in ("text", "content"):
                if isinstance(first.get(key), str):
                    return first[key]
    for key in ("content", "response"):
        if isinstance(payload.get(key), str):
            return payload[key]
    return None



# ------------------------------------------------------------ разбор ответа


def _strip_fences(text: str) -> str:
    value = (text or "").strip()
    if value.startswith("```"):
        value = value.split("\n", 1)[1] if "\n" in value else ""
        if value.rstrip().endswith("```"):
            value = value.rstrip()[:-3]
    return value.strip()


def _json_candidates(text: str):
    """Терпимый разбор: целый JSON, затем первый сбалансированный объект/список."""
    value = _strip_fences(text)
    try:
        return json.loads(value)
    except ValueError:
        pass
    decoder = json.JSONDecoder()
    for index, char in enumerate(value):
        if char not in "{[":
            continue
        try:
            obj, _end = decoder.raw_decode(value[index:])
        except ValueError:
            continue
        return obj
    return None


def _as_int(value) -> Optional[int]:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, float) and value.is_integer():
        return int(value)
    if isinstance(value, str) and value.strip().lstrip("-").isdigit():
        return int(value.strip())
    return None


def parse_edits(text: str) -> Tuple[List[dict], Optional[str]]:
    """Терпимый разбор ответа LLM → `([правки], ошибка разбора)`.

    Принимаются markdown-обёртки, `{"edits": [...]}`, просто список правок и
    одиночная правка. Мусорные элементы помечаются `_malformed` и уходят в
    журнал как отклонённые (`malformed_edit`) — текст не меняется.
    """
    data = _json_candidates(text or "")
    if data is None:
        return [], "ответ LLM не удалось разобрать как JSON"
    raw_edits = None
    if isinstance(data, list):
        raw_edits = data
    elif isinstance(data, dict):
        if isinstance(data.get("edits"), list):
            raw_edits = data["edits"]
        elif "word_ids" in data:
            raw_edits = [data]
    if raw_edits is None:
        return [], "в ответе LLM нет списка edits"

    edits: List[dict] = []
    for item in raw_edits:
        if not isinstance(item, dict):
            edits.append({"_malformed": True, "_raw": str(item)[:200]})
            continue
        ids = []
        valid = True
        raw_ids = item.get("word_ids")
        if not isinstance(raw_ids, list):
            valid = False
        else:
            for value in raw_ids:
                number = _as_int(value)
                if number is None:
                    valid = False
                    break
                ids.append(number)
        replacement = item.get("replacement")
        if not isinstance(replacement, str):
            valid = False
        if not valid or not ids or not (replacement or "").strip():
            edits.append({"_malformed": True, "_raw": str(item)[:200]})
            continue
        edits.append(
            {
                "word_ids": sorted(set(ids)),
                "replacement": replacement.strip(),
                "reason": str(item.get("reason") or "").strip()[:300],
                "needs_review": bool(item.get("needs_review")),
                "allow_global": bool(item.get("allow_global")),
                # Метаданные не меняются: их наличие — попытка правки структуры.
                "metadata": {
                    key: item[key]
                    for key in ("speaker", "start", "end", "alignment")
                    if key in item
                },
            }
        )
    return edits, None



# ---------------------------------------------------------------- валидатор


def _span_text(words, start: int, end: int) -> str:
    parts = [(word.get("text") or "").strip() for word in words[start:end]]
    return " ".join(part for part in parts if part).strip()


def _numeric_tokens(text: str) -> Tuple[str, ...]:
    """Числа в тексте: цифровые группы и числительные словами (нормализовано)."""
    out: List[str] = []
    for match in _DIGITS_RE.finditer(text or ""):
        out.append(match.group(0).replace(",", "."))
    for token in _TOKEN_RE.findall(text or ""):
        norm = glossary_mod.normalize(token)
        if norm in _NUMBER_WORD_SET:
            out.append(norm)
    return tuple(sorted(out))


def _negations(text: str) -> set:
    out = set()
    for token in _TOKEN_RE.findall(text or ""):
        norm = glossary_mod.normalize(token)
        if norm in _NEGATION_SET:
            out.add(norm)
    return out


def _has_context(words, start: int, end: int, contexts: Sequence[str],
                 window: int = 6) -> bool:
    """Есть ли контекст термина рядом с фрагментом (как в `glossary.apply`)."""
    if not contexts:
        return False
    lo = max(0, start - window)
    hi = min(len(words), end + window)
    around = [glossary_mod.normalize(word.get("text") or "") for word in words[lo:hi]]
    around = [tok for tok in around if tok]
    for phrase in contexts:
        ctx = glossary_mod.tokens(phrase)
        if not ctx:
            continue
        size = len(ctx)
        for pos in range(0, max(0, len(around) - size + 1)):
            if around[pos:pos + size] == ctx:
                return True
    return False


def _contains_tokens(haystack: Sequence[str], needle: Sequence[str]) -> bool:
    """Есть ли `needle` подряд в `haystack` (сравнение нормализованных токенов)."""
    size = len(needle)
    if not size or size > len(haystack):
        return False
    for pos in range(0, len(haystack) - size + 1):
        if list(haystack[pos:pos + size]) == list(needle):
            return True
    return False


def _sub_tokens(text: str) -> List[str]:
    """Токены для проверки вхождения канона: делим ещё и по дефису.

    «ERP-система» должно содержать канон «ERP», хотя `glossary.tokens` держит
    дефис внутри слова (это важно для вариантов вроде «интернет-магазин»).
    """
    return [
        part for part in (glossary_mod.normalize(piece)
                          for piece in re.split(r"[\s\-–—]+", text or ""))
        if part
    ]


def _contains_canonical(term, text: str) -> bool:
    """Содержит ли текст канон термина (по под-токенам, с дефисами)."""
    return _contains_tokens(_sub_tokens(text), _sub_tokens(term.canonical))


def _variant_similarity(text: str, variants: Sequence[str]) -> float:
    """Максимальное сходство фрагмента с вариантами термина (`0..1`)."""
    core = glossary_mod.normalize(text)
    best = 0.0
    for variant in variants:
        norm = glossary_mod.normalize(variant)
        if not norm:
            continue
        best = max(best, difflib.SequenceMatcher(None, core, norm).ratio())
    return best


def _content_words(text: str) -> List[str]:
    """Содержательные слова фрагмента: нормализованные под-токены длиной ≥ 5."""
    return [word for word in _sub_tokens(text) if len(word) >= CONTENT_WORD_MIN_CHARS]


def _term_words(term) -> set:
    """Слова вариантов и канона термина — их «потеря» при замене допустима."""
    out = set()
    if term is None:
        return out
    out.update(_sub_tokens(term.canonical))
    for variant in term.all_variants:
        out.update(_sub_tokens(variant))
    return out


def _word_kept(word: str, replacement_tokens: Sequence[str]) -> bool:
    """Слово сохранено в замене: точное совпадение или близкая форма (≥0,80)."""
    if word in replacement_tokens:
        return True
    return any(
        difflib.SequenceMatcher(None, word, token).ratio() >= CONTENT_WORD_KEEP_RATIO
        for token in replacement_tokens
    )


def _loses_content(before: str, replacement: str, term) -> bool:
    """Теряет ли замена содержательные слова фрагмента (кроме слов термина).

    Пример: `сейчас написано RP-система` → `сейчас ERP` теряет «написано» —
    такой патч отклоняется (`content_lost`), хотя длина и числа формально в норме.
    """
    replacement_tokens = _sub_tokens(replacement)
    exempt = _term_words(term)
    for word in _content_words(before):
        if word in exempt:
            continue
        if not _word_kept(word, replacement_tokens):
            return True
    return False


def _match_term(glossary, before: str, replacement: str):
    """Термин глоссария, к которому относится патч.

    Три способа (в порядке силы): замена ровно канон; фрагмент ровно вариант;
    канон входит в замену («ERP» → «ERP-система»), а варианта не было в тексте.
    Последнее нужно, чтобы такие патчи получали контекст термина (§4.2), а не
    отклонялись как «нет контекста глоссария».
    """
    rep = glossary_mod.normalize(replacement)
    before_tokens = glossary_mod.tokens(before)
    if not rep:
        return None
    for term in glossary.terms:
        if rep == term.norm_canonical:
            return term
        for variant in term.all_variants:
            if glossary_mod.tokens(variant) == before_tokens:
                return term
    rep_tokens = _sub_tokens(replacement)
    before_parts = _sub_tokens(before)
    for term in glossary.terms:
        canonical = _sub_tokens(term.canonical)
        if _contains_tokens(rep_tokens, canonical) and not _contains_tokens(
            before_parts, canonical
        ):
            return term
    return None


def _same_number(left, right) -> bool:
    try:
        return abs(float(left) - float(right)) < 1e-6
    except (TypeError, ValueError):
        return left == right


class _Validator:
    """Проверка патчей против транскрипта (без LLM) — гейт P4.2."""

    def __init__(self, blocks, glossary, applied_spans, max_edits: int):
        self.blocks = blocks
        self.glossary = glossary
        self.applied_spans = applied_spans or {}
        self.max_edits = max_edits
        self.accepted_spans: Dict[int, List[Tuple[int, int]]] = {}
        self.applied = 0
        self.words_by_id: Dict[int, Tuple[int, int]] = {}
        for block_index, block in enumerate(blocks or []):
            for pos, word in enumerate(block.get("words") or []):
                word_id = word.get("word_id")
                if isinstance(word_id, int):
                    self.words_by_id[word_id] = (block_index, pos)


    def check(self, edit: dict) -> Tuple[Optional[str], Optional[dict]]:
        """`(причина отказа | None, контекст)`. Причина — машинный код.

        Контекст возвращается и при отказе (кроме совсем нерезолвимых id): в
        журнале нужен `before`, иначе непонятно, что именно отклонено.
        """
        ids = edit.get("word_ids") or []
        if not ids:
            return "empty_word_ids", None
        positions = []
        for word_id in ids:
            if word_id not in self.words_by_id:
                return "unknown_word_ids", None
            positions.append(self.words_by_id[word_id])
        block_index = positions[0][0]
        if any(pos[0] != block_index for pos in positions):
            return "cross_block", None
        block = self.blocks[block_index]
        words = block.get("words") or []
        local = sorted(pos[1] for pos in positions)
        start, end = local[0], local[-1] + 1
        context = {
            "block_index": block_index,
            "block": block,
            "words": words,
            "start": start,
            "end": end,
            "before": _span_text(words, start, end) if end - start == len(local) else "",
            "term": "",
        }
        if end - start != len(local):
            return "non_contiguous_word_ids", context

        speakers = {words[pos].get("speaker") for pos in local}
        speakers.discard(None)
        if len(speakers) > 1:
            return "speaker_mismatch", context

        metadata = edit.get("metadata") or {}
        first, last = words[start], words[end - 1]
        if "speaker" in metadata and metadata["speaker"] not in (
            None, first.get("speaker"), block.get("speaker")
        ):
            return "metadata_change", context
        if "start" in metadata and not _same_number(metadata["start"], first.get("start")):
            return "metadata_change", context
        if "end" in metadata and not _same_number(metadata["end"], last.get("end")):
            return "metadata_change", context
        if "alignment" in metadata and metadata["alignment"] != first.get("alignment"):
            return "metadata_change", context

        if self._overlaps(block_index, start, end):
            return "overlaps_applied", context

        before = context["before"]
        replacement = edit.get("replacement") or ""
        if not replacement:
            return "empty_replacement", context
        if "\n" in replacement or "\r" in replacement:
            return "multiline_replacement", context
        # Точное совпадение — не правка. Краевая пунктуация не считается правкой
        # (она сохраняется при применении), а смена регистра/написания канона
        # («1c» → «1С») — законная нормализация, поэтому сравниваем ядро слова
        # без нормализации: иначе такие патчи молча отбрасывались бы.
        if replacement.strip() in (before.strip(), glossary_mod.strip_punct(before)):
            return "no_change", context

        term = _match_term(self.glossary, before, replacement)
        context["term"] = term.canonical if term else ""
        is_canonical = bool(
            term and glossary_mod.normalize(replacement) == term.norm_canonical
        )
        before_len = len(before) or 1
        if not is_canonical and len(replacement) > MAX_REPLACEMENT_RATIO * before_len:
            return "too_long", context
        before_tokens = max(1, len(_TOKEN_RE.findall(before)))
        if (not is_canonical
                and len(_TOKEN_RE.findall(replacement)) > MAX_REPLACEMENT_RATIO * before_tokens):
            return "too_long", context
        # Удаление содержания: нельзя заменять три слова одним не-каноном.
        min_tokens = max(1, int(math.ceil(MIN_REPLACEMENT_TOKEN_RATIO * before_tokens)))
        if not is_canonical and len(_TOKEN_RE.findall(replacement)) < min_tokens:
            return "too_short", context

        forbidden = {glossary_mod.normalize(phrase) for phrase in self.glossary.forbid}
        if glossary_mod.normalize(replacement) in forbidden:
            return "forbidden", context

        if _negations(before) - _negations(replacement):
            return "negation_lost", context
        if _numeric_tokens(before) != _numeric_tokens(replacement):
            return "numbers_changed", context
        if _loses_content(before, replacement, term):
            return "content_lost", context

        # needs_review: текст не меняется, контекст не требуется (решает человек).
        if not edit.get("needs_review"):
            if not self._context_ok(term, edit, words, start, end):
                return "no_glossary_context", context
            if self.applied >= self.max_edits:
                return "edit_limit", context
        return None, context

    def _context_ok(self, term, edit, words, start: int, end: int) -> bool:
        """Обоснование замены (§4.2 + решение C от 23.10.2026).

        `allow_global` от модели, `allow_global`/`priority: high` термина,
        контекст термина рядом — или каноническая замена, когда исходный
        фрагмент похож на вариант термина (≥ `VARIANT_FUZZY_MIN`). Последнее
        закрывает `RP-система → ERP-система` без контекста, но не пропускает
        посторонние вставки канона (`слайдик → слайд ERP`).
        """
        if edit.get("allow_global"):
            return True
        if term is None:
            return False
        if term.allow_global or term.priority == "high":
            return True
        if _has_context(words, start, end, term.context):
            return True
        replacement = edit.get("replacement") or ""
        if _contains_canonical(term, replacement) and _variant_similarity(
            _span_text(words, start, end), term.all_variants
        ) >= VARIANT_FUZZY_MIN:
            return True
        return False

    def _overlaps(self, block_index: int, start: int, end: int) -> bool:
        spans = list(self.applied_spans.get(block_index) or [])
        spans.extend(self.accepted_spans.get(block_index) or [])
        return any(start < other_end and end > other_start
                   for other_start, other_end in spans)

    def accept(self, block_index: int, start: int, end: int) -> None:
        self.accepted_spans.setdefault(block_index, []).append((start, end))
        self.applied += 1



# -------------------------------------------------------------- применение


def _apply(block: dict, start: int, end: int, replacement: str) -> None:
    """Заменить слова `[start, end)` одной строкой, сохранив краевую пунктуацию."""
    words = block.get("words") or []
    first_text = words[start].get("text") or ""
    last_text = words[end - 1].get("text") or ""
    prefix, _, _ = glossary_mod.split_edge_punct(first_text)
    _, _, suffix = glossary_mod.split_edge_punct(last_text)
    words[start]["text"] = prefix + replacement + suffix
    for pos in range(start + 1, end):
        words[pos]["text"] = ""
    block["text"] = join_words(words)


# ---------------------------------------------------------------- VRAM (P4.4)


def release_gpu() -> None:
    """Перед HTTP-вызовом отдать GPU: `gc.collect()` + `torch.cuda.empty_cache()`.

    Модели ASR/alignment/диаризации к этому моменту уже выгружены (`unload` в
    `finally`), но кэш-аллокатор torch держит блоки. На DML ORT-сессии закрыты
    тем же `asr_onnx.unload`; вызов `torch.cuda.*` там не делаем.
    """
    gc.collect()
    try:
        import torch  # noqa: PLC0415 - тяжёлый импорт только при включённой LLM

        if torch.cuda.is_available():
            torch.cuda.empty_cache()
    except Exception:  # noqa: BLE001 - нет torch/драйвера: LLM всё равно в llm_host
        pass


# -------------------------------------------------------------------- прогон


def _allowance(words_count: int, per_1000: float) -> int:
    """Лимит правок: `max_edits_per_1000`, но не меньше 1 на коротких файлах."""
    if per_1000 <= 0:
        return 0
    limit = int(math.ceil(per_1000 * max(1, words_count) / 1000.0))
    return max(1, min(words_count, limit))


def _journal(edit: dict, context: dict, status: str, reason: str, model: str,
             latency_ms: float) -> dict:
    words = context.get("words") or []
    start, end = context.get("start") or 0, context.get("end") or 0
    if words and end > start:
        first, last = words[start], words[end - 1]
        word_ids = [words[pos].get("word_id") for pos in range(start, end)]
        word_start, word_end = first.get("start"), last.get("end")
    else:
        first = {}
        word_ids = list(edit.get("word_ids") or [])
        word_start = word_end = None
    return {
        "block_index": context.get("block_index"),
        "word_id": first.get("word_id"),
        "word_ids": word_ids,
        "before": context.get("before") or "",
        "after": edit.get("replacement"),
        "term": context.get("term") or "",
        "reason": reason,
        "needs_review": status == "needs_review",
        "span": [start, end],
        "start": word_start,
        "end": word_end,
        "source": "llm",
        "status": status,
        "model": model,
        "llm_reason": edit.get("reason") or "",
        "allow_global": bool(edit.get("allow_global")),
        "latency_ms": latency_ms,
    }


def _rejected_raw(item: dict, model: str, latency_ms: float) -> dict:
    return {
        "block_index": None,
        "word_id": None,
        "word_ids": [],
        "before": item.get("_raw") or "",
        "after": None,
        "term": "",
        "reason": "malformed_edit",
        "needs_review": False,
        "span": [],
        "start": None,
        "end": None,
        "source": "llm",
        "status": "rejected",
        "model": model,
        "llm_reason": "",
        "allow_global": False,
        "latency_ms": latency_ms,
    }


def _stats(edits: Sequence[dict]) -> dict:
    reasons: Dict[str, int] = {}
    applied = rejected = review = 0
    for edit in edits:
        reason = str(edit.get("reason") or "")
        reasons[reason] = reasons.get(reason, 0) + 1
        status = edit.get("status")
        if status == "applied":
            applied += 1
        elif status == "needs_review":
            review += 1
        else:
            rejected += 1
    return {
        "edits": len(edits),
        "applied": applied,
        "rejected": rejected,
        "needs_review": review,
        "reasons": dict(sorted(reasons.items())),
    }



def run(blocks, params, glossary, applied_spans=None, review_spans=None) -> dict:
    """LLM-слой: кандидаты → запросы → валидатор → правки `blocks` на месте.

    Возвращает `{"edits", "stats", "summary", "warnings"}`.
    `applied_spans` — `{block_index: [(start, end)]}` применённых глоссарием
    правок: пересекаться с ними запрещено (патч отклоняется, текст не портится).
    `review_spans` — `{(block_index, pos)}` спорных случаев глоссария: они тоже
    показываются модели как кандидаты.
    """
    started = time.perf_counter()
    edits: List[dict] = []
    warnings: List[str] = []
    summary = {
        "enabled": bool(params.llm_enabled),
        "model": params.llm_model,
        "host": params.llm_base_url,
        "temperature": 0,
        "windows": 0,
        "requests": 0,
        "proposed": 0,
        "applied": 0,
        "rejected": 0,
        "needs_review": 0,
        "allowance": 0,
        "latency_ms": 0.0,
        "errors": [],
    }
    if not params.llm_enabled:
        return {"edits": edits, "stats": _stats(edits), "summary": summary,
                "warnings": warnings}

    words_count = sum(len(block.get("words") or []) for block in blocks or [])
    allowance = _allowance(words_count, float(params.llm_max_edits_per_1000 or 0.0))
    summary["allowance"] = allowance
    if allowance <= 0:
        warnings.append(
            "LLM: max_edits_per_1000=0 — патчи не применяются (только глоссарий)"
        )
        summary["errors"].append("edit_limit_zero")
        summary["latency_ms"] = _ms(started)
        return {"edits": edits, "stats": _stats(edits), "summary": summary,
                "warnings": warnings}

    windows = candidate_windows(blocks, glossary, review_spans)
    if not windows:
        summary["latency_ms"] = _ms(started)
        return {"edits": edits, "stats": _stats(edits), "summary": summary,
                "warnings": warnings}

    # P4.4: отдать GPU перед HTTP-вызовом (кэш аллокатора torch), но только
    # когда кандидаты реально есть — иначе лишний импорт torch на маленьких файлах.
    release_gpu()

    max_windows = MAX_WINDOWS_PER_REQUEST * MAX_REQUESTS
    if len(windows) > max_windows:
        warnings.append(
            "LLM: подозрительных окон %d > %d — обработаны первые (лимит стоимости), "
            "остальные остаются на ручную проверку" % (len(windows), max_windows)
        )
        windows = windows[:max_windows]
    summary["windows"] = len(windows)

    validator = _Validator(blocks, glossary, applied_spans, allowance)
    applied_word_ids = set()
    for start in range(0, len(windows), MAX_WINDOWS_PER_REQUEST):
        batch = windows[start:start + MAX_WINDOWS_PER_REQUEST]
        messages = build_messages(blocks, glossary, batch, applied_word_ids)
        content, error, latency = _post_chat(params, messages)
        summary["requests"] += 1
        if error:
            # Деградация: недоступный хост/таймаут — дальше не долбим, текст как
            # без LLM (гейт P4: задание не падает).
            summary["errors"].append(error)
            warnings.append(
                "LLM: %s — деградация до глоссария (LLM-правки не применены)" % error
            )
            break
        summary["latency_ms"] = round(summary["latency_ms"] + latency, 1)
        parsed, parse_error = parse_edits(content or "")
        if parse_error:
            summary["errors"].append(parse_error)
            warnings.append("LLM: %s — батч пропущен" % parse_error)
            continue
        summary["proposed"] += len(parsed)
        for edit in parsed:
            if edit.get("_malformed"):
                summary["rejected"] += 1
                edits.append(_rejected_raw(edit, params.llm_model, latency))
                continue
            reason, context = validator.check(edit)
            if reason:
                summary["rejected"] += 1
                edits.append(_journal(edit, context or {}, "rejected", reason,
                                      params.llm_model, latency))
                continue
            if edit.get("needs_review"):
                summary["needs_review"] += 1
                edits.append(_journal(edit, context, "needs_review", "llm_review",
                                      params.llm_model, latency))
                continue
            _apply(context["block"], context["start"], context["end"],
                   edit["replacement"])
            validator.accept(context["block_index"], context["start"], context["end"])
            summary["applied"] += 1
            applied_word_ids.update(edit.get("word_ids") or [])
            edits.append(_journal(edit, context, "applied", "llm_patch",
                                  params.llm_model, latency))

    summary["latency_ms"] = round(summary["latency_ms"] or _ms(started), 1)
    return {
        "edits": edits,
        "stats": _stats(edits),
        "summary": summary,
        "warnings": warnings,
    }

