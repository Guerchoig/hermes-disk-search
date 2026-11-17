"""Морфология базы меток v2: склейка словоформ, падежи, суффикс метки.

Требования — `PLAN_AGENT_PRIVACY_ZONE.md` TR-2.1…TR-2.4 (D7) с учётом фактов
спайка A0 (`tools/parity/AGENT_SPIKES.md` §3.9) и правок плана П-2, П-12:

* `norm_key` — нормальная форма значения (ключ склейки словоформ);
* порождённые падежные формы (`origin='derived'` в базе) — для фильтра
  исходящего и обратной подстановки;
* падежный суффикс `[PERSON_3|дат]` ставится **только** при однозначном
  определении падежа: в A0 «в лоб» по одному слову это 19,7 %, поэтому
  при сомнении суффикса нет и подстановка помечается «проверить» (TR-2.4).

Правила, найденные спайком A0 и обязательные здесь (П-2):

* `NamesExtractor(MorphVocab())` вызывается как `names(text)` (не `.extract`);
* фамилия приходит **в исходной форме** («Иванову») — лемматизировать надо
  отдельно, разбор с граммемой `Surn` приоритетнее (иначе прилагательная
  фамилия «Толстой» в косвенных падежах даёт ключ «толстый»);
* инициалы приходят без точки («И») — приводим к «и.»;
* дефисную фамилию `NamesExtractor` режет на два совпадения — склеиваем;
* дефисные фамилии склоняются по частям.

Модуль импортирует pymorphy2/natasha лениво: без них анонимизация работает
(ключ = нижний регистр, падежи не порождаются, суффикс не ставится), но
качество склейки падает — воркер всегда запускается с этими пакетами.
"""
from __future__ import annotations

import logging
import threading
from typing import Iterable, Optional

logger = logging.getLogger("hds_anonymize.morph")

# Падежи в порядке плана (TR-2.3) и суффиксы метки
CASES = ("nomn", "gent", "datv", "accs", "ablt", "loct")
CASE_SUFFIX = {
    "nomn": "",
    "gent": "род",
    "datv": "дат",
    "accs": "вин",
    "ablt": "твор",
    "loct": "пред",
}
SUFFIX_CASE = {suffix: case for case, suffix in CASE_SUFFIX.items() if suffix}

# Граммема множественного числа в суффиксе метки (TR-2.3): «,мн»
PLURAL_GRAM = "мн"

# Типы со словоформами (TR-2.1): для них считается norm_key и порождаются
# падежи. Остальные типы (PHONE/EMAIL/INN/...) — каноническая запись.
WORD_TYPES = frozenset(
    {"PERSON", "SURNAME", "ORG", "LOC", "POSITION", "DEPARTMENT", "PRODUCT"}
)
# Типы, у которых словоформы порождаются (TR-2.2: PERSON и однословные ORG/LOC)
INFLECTED_TYPES = frozenset({"PERSON", "SURNAME"})

# Организационно-правовые формы: часть названия, лемматизировать не нужно
_ORG_FORMS = frozenset(
    {"ооо", "оао", "зао", "пао", "ао", "ип", "нко", "нпф", "гк", "ано", "фонд", "банк"}
)
_QUOTE_CHARS = "«»\"'„“”"
_INITIAL_ENDINGS = (
    ("овна", "евна", "ична", "инична"),
    ("ович", "евич", "ич"),
)

_lock = threading.Lock()
_morph = None
_morph_ready = False
_names = None
_names_ready = False


# ── ленивая загрузка анализаторов ────────────────────────────────────────


def _patch_inspect_getargspec() -> None:
    """Совместимость pymorphy2 с Python 3.11+ (`inspect.getargspec` удалён).

    pymorphy2 (2018) в `units/base.py` вызывает `inspect.getargspec`, которого
    нет с 3.11; без шима `MorphAnalyzer()` падает на Python 3.12 из
    `venv-anonymize`. natasha импортируется раньше и патчит это сама, но
    полагаться на порядок импортов нельзя.
    """
    import inspect  # noqa: PLC0415

    if hasattr(inspect, "getargspec"):
        return
    from collections import namedtuple  # noqa: PLC0415

    arg_spec = namedtuple("ArgSpec", "args varargs keywords defaults")

    def getargspec(func):  # noqa: ANN001 — сигнатура как у старого API
        spec = inspect.getfullargspec(func)
        return arg_spec(spec.args, spec.varargs, spec.varkw, spec.defaults)

    inspect.getargspec = getargspec  # type: ignore[attr-defined]


def _analyzer():
    """pymorphy2 MorphAnalyzer (или None, если пакета нет)."""
    global _morph, _morph_ready
    if _morph_ready:
        return _morph
    with _lock:
        if not _morph_ready:
            try:
                _patch_inspect_getargspec()
                import pymorphy2  # noqa: PLC0415 — тяжёлый импорт, только по требованию

                _morph = pymorphy2.MorphAnalyzer()
            except Exception as exc:  # noqa: BLE001 — без морфологии тоже работаем
                logger.warning("pymorphy2 недоступен (%s): ключи — нижний регистр", exc)
                _morph = None
            _morph_ready = True
    return _morph


def _names_extractor():
    """Natasha NamesExtractor (или None): разбор ФИО по грамматикам."""
    global _names, _names_ready
    if _names_ready:
        return _names
    with _lock:
        if not _names_ready:
            try:
                from natasha import MorphVocab, NamesExtractor  # noqa: PLC0415

                _names = NamesExtractor(MorphVocab())
            except Exception as exc:  # noqa: BLE001
                logger.warning("natasha недоступен (%s): ФИО разбираются эвристикой", exc)
                _names = None
            _names_ready = True
    return _names


def reset_caches() -> None:
    """Сбросить анализаторы (тесты/повторная конфигурация)."""
    global _morph, _names, _morph_ready, _names_ready
    with _lock:
        _morph, _names = None, None
        _morph_ready = _names_ready = False


def backend_info() -> str:
    """Строка состояния для логов/`hello` (без значений)."""
    return (
        f"pymorphy2: {'ok' if _analyzer() is not None else 'нет'}; "
        f"natasha: {'ok' if _names_extractor() is not None else 'нет'}"
    )


def is_available() -> bool:
    return _analyzer() is not None


# ── разбор слов ──────────────────────────────────────────────────────────


def _parses(word: str) -> list:
    analyzer = _analyzer()
    if analyzer is None or not word:
        return []
    try:
        return list(analyzer.parse(word))
    except Exception:  # noqa: BLE001 — битое слово не должно ронять акт
        return []


def _cases_of(parse) -> set[str]:
    tag = parse.tag
    return {case for case in CASES if case in tag}


def _is_surn(parse) -> bool:
    return "Surn" in parse.tag


def lemma(word: str, *, prefer_surn: bool = True) -> str:
    """Лемма слова в нижнем регистре.

    `prefer_surn` — приоритет разбору с граммемой `Surn` (фамилия):
    «Толстого» → «толстой», а не «толстый» (правка П-2).
    """
    word = word.strip()
    if not word:
        return ""
    # инициал («И», «И.») — не слово: приводим к «и.»
    if len(word.rstrip(".")) == 1:
        return f"{word[0].lower()}."
    parses = _parses(word)
    if not parses:
        return word.lower()
    if prefer_surn:
        for parse in parses:
            if _is_surn(parse):
                return parse.normal_form.lower()
        for parse in parses:
            # прилагательные фамилии («Белый») — мужской именительный
            if parse.tag.POS in ("ADJF", "PRTF") and "masc" in parse.tag:
                return parse.normal_form.lower()
    return parses[0].normal_form.lower()


def _lemma_dashed(word: str) -> str:
    """Дефисная фамилия: лемматизируются обе части."""
    return "-".join(lemma(part) for part in word.split("-"))


def _initials_key(word: str) -> str:
    """Инициал → «и.»; слово остаётся леммой (имя/отчество)."""
    word = (word or "").strip()
    if not word:
        return ""
    if len(word.rstrip(".")) == 1:
        return f"{word[0].lower()}."
    return _lemma_dashed(word)


def _initial_with_dot(word: str) -> str:
    """«И» → «И.»; слово длиннее одной буквы не трогаем."""
    word = (word or "").strip()
    if not word:
        return ""
    if len(word.rstrip(".")) == 1:
        return f"{word[0].upper()}."
    return word


def person_parts(value: str) -> dict:
    """Фамилия/имя/отчество и род из строки ФИО.

    Возвращает `{"last", "first", "middle", "gender"}`; недостающие части —
    пустые строки, `gender` — "masc"/"femn"/None. Дефисная фамилия
    склеивается из двух совпадений `NamesExtractor` (поведение A0).
    """
    value = (value or "").strip()
    if not value:
        return {"last": "", "first": "", "middle": "", "gender": None}

    matches = []
    extractor = _names_extractor()
    if extractor is not None:
        try:
            matches = list(extractor(value))
        except Exception:  # noqa: BLE001 — эвристика ниже
            matches = []

    last = first = middle = ""
    use_natasha = False
    if matches:
        full = [
            m for m in matches
            if getattr(m.fact, "first", None) and getattr(m.fact, "last", None)
        ]
        if full:
            chosen, use_natasha = full[0], True
        else:
            # Одно слово («Иванов»): принимаем совпадение только с фамилией,
            # иначе «Иванов И И» (без точек) разбирается как одно имя
            last_only = [m for m in matches if getattr(m.fact, "last", None)]
            if last_only and len([p for p in value.split() if p]) == 1:
                chosen, use_natasha = last_only[0], True
    if use_natasha:
        last = getattr(chosen.fact, "last", None) or ""
        first = getattr(chosen.fact, "first", None) or ""
        middle = getattr(chosen.fact, "middle", None) or ""
    else:
        parts = [p for p in value.split() if p]
        if len(parts) >= 3:
            last, first, middle = parts[0], parts[1], parts[2]
        elif len(parts) == 2:
            last, first = parts[0], parts[1]
        elif parts:
            last = parts[0]

    # Инициалы Natasha отдаёт без точки («И») — приводим к «И.» (П-2):
    # иначе сгенерированные падежи теряют точку и не совпадают с текстом
    first = _initial_with_dot(first)
    middle = _initial_with_dot(middle)

    # Дефисная фамилия: «Сидоров-Амурский» → «Сидоров» + «Амурский»
    if last and "-" in value:
        for token in value.split():
            token = token.strip(".,;:()" + _QUOTE_CHARS)
            if "-" not in token:
                continue
            head, tail = token.split("-", 1)
            if last in (head, tail):
                last = token
                break

    gender: Optional[str] = None
    middle_lower = middle.lower()
    if middle_lower.endswith(_INITIAL_ENDINGS[0]):
        gender = "femn"
    elif middle_lower.endswith(_INITIAL_ENDINGS[1]):
        gender = "masc"
    else:
        for parse in _parses(first)[:3]:
            if "femn" in parse.tag:
                gender = "femn"
                break
            if "masc" in parse.tag:
                gender = "masc"
                break

    return {"last": last, "first": first, "middle": middle, "gender": gender}


# ── norm_key (TR-2.1) ────────────────────────────────────────────────────


def norm_key(value: str, type_: str) -> str:
    """Нормальная форма значения — ключ склейки словоформ (TR-2.1).

    `PERSON` — `фамилия|имя|отчество` в именительном падеже, нижний регистр;
    инициалы — `и.`. `ORG`/`LOC` и прочие словесные типы — леммы слов; текст
    в кавычках и латиница не лемматизируются. Шаблонные типы — каноническая
    запись (только цифры / нижний регистр).
    """
    value = (value or "").strip()
    if not value:
        return ""

    if type_ in ("PERSON", "SURNAME"):
        parts = person_parts(value)
        if parts["last"]:
            words = [
                _lemma_dashed(parts["last"]),
                _initials_key(parts["first"]),
                _initials_key(parts["middle"]),
            ]
            return "|".join(word for word in words if word)
        # ФИО не распознано — ключ по леммам слов (не склеиваем зря)
        return "|".join(_lemma_dashed(word) for word in value.split())

    if type_ in WORD_TYPES:
        tokens = value
        for char in _QUOTE_CHARS:
            tokens = tokens.replace(char, " ")
        out = []
        for token in tokens.split():
            token = token.strip(".,;:()[]{}")
            if not token:
                continue
            if token.isascii() or token.lower() in _ORG_FORMS:
                out.append(token.lower())
            else:
                out.append(_lemma_dashed(token))
        return "|".join(out)

    # Шаблонные типы: словоформ нет — каноническая запись
    if type_ == "EMAIL":
        return value.lower()
    digits = "".join(char for char in value if char.isdigit())
    return digits or value.lower()


# ── генерация падежей (TR-2.2) ──────────────────────────────────────────


def _best_parse(word: str, gender: Optional[str] = None):
    """Разбор для склонения: согласованный по роду, предпочтительно Surn."""
    parses = _parses(word)
    if not parses:
        return None
    candidates = [
        parse for parse in parses
        if parse.tag.POS in ("NOUN", "ADJF", "ADJS", "PRTF", "PRTS")
    ] or parses
    if gender:
        for parse in candidates:
            if ("femn" in parse.tag) == (gender == "femn"):
                return parse
    for parse in candidates:
        if _is_surn(parse) and "masc" in parse.tag:
            return parse
    return candidates[0]


def _inflect_word(
    word: str,
    case: str,
    gender: Optional[str] = None,
    plural: bool = False,
) -> Optional[str]:
    """Слово в нужном падеже/числе; None — не удалось (или слово не русское)."""
    word = (word or "").strip()
    if not word:
        return None
    if word.isascii() or len(word.rstrip(".")) == 1:
        return word  # латиница и инициалы не склоняются
    if "-" in word:
        return "-".join(
            _inflect_word(part, case, gender, plural) or part
            for part in word.split("-")
        )
    parse = _best_parse(word, gender)
    if parse is None:
        return None
    gram = {"plur", case} if plural else {case}
    try:
        inflected = parse.inflect(gram)
    except Exception:  # noqa: BLE001 — странный разбор: слово остаётся как есть
        return word
    if inflected is None:
        return None if plural else word
    result = inflected.word
    if word[:1].isupper():
        result = result[:1].upper() + result[1:]
    return result


def _word_forms(word: str, gender: Optional[str]) -> dict[tuple[str, bool], str]:
    """Все падежи одного слова: {(падеж, мн.ч.): форма}."""
    out: dict[tuple[str, bool], str] = {}
    for plural in (False, True):
        for case in CASES:
            form = _inflect_word(word, case, gender, plural)
            if form:
                out[(case, plural)] = form
    return out


def generate_forms(value: str, type_: str) -> list[tuple[str, str]]:
    """Порождённые падежные формы значения (TR-2.2).

    Возвращает список `(форма, gram)`, gram — падеж (`gent`) или `падеж,мн`
    (`gent,мн`) для множественного числа. Для `PERSON` склоняются все части
    ФИО (инициалы не меняются); для однословных `ORG`/`LOC` — само слово.
    Многословные названия и текст в кавычках не порождаются.
    """
    value = (value or "").strip()
    if not value:
        return []
    forms: dict[tuple[str, bool], str] = {}

    if type_ in ("PERSON", "SURNAME"):
        parts = person_parts(value)
        gender = parts["gender"]
        words = [w for w in (parts["last"], parts["first"], parts["middle"]) if w]
        if not words:
            words = value.split()
        per_word = [_word_forms(word, gender) for word in words]
        for plural in (False, True):
            for case in CASES:
                pieces = []
                for index, forms_of_word in enumerate(per_word):
                    piece = forms_of_word.get((case, plural))
                    if piece is None:
                        piece = words[index]  # инициал/несклоняемое
                    pieces.append(piece)
                candidate = " ".join(pieces)
                if candidate:
                    forms[(case, plural)] = candidate
    elif type_ in ("ORG", "LOC"):
        # однословные названия склоняются (TR-2.2); многословные и кавычки — нет
        if any(char in value for char in _QUOTE_CHARS) or len(value.split()) != 1:
            return []
        forms.update(_word_forms(value, None))

    out: list[tuple[str, str]] = []
    seen: set[str] = set()
    for (case, plural), form in forms.items():
        gram = case if not plural else f"{case},{PLURAL_GRAM}"
        if form in seen:
            continue
        seen.add(form)
        out.append((form, gram))
    return out


# ── падеж исходной формы и суффикс метки (TR-2.3, П-12) ─────────────────


def _case_intersection(words: Iterable[str]) -> tuple[Optional[str], bool]:
    """Падеж, если все значимые слова согласованы однозначно.

    Верхние разборы каждого слова (до трёх) дают множества падежей;
    пересечение должно дать ровно один падеж. Инициалы и латиница
    пропускаются.
    """
    common: Optional[set[str]] = None
    for word in words:
        word = word.strip()
        if not word or word.isascii() or len(word.rstrip(".")) == 1:
            continue
        cases: set[str] = set()
        for parse in _parses(word)[:3]:
            cases |= _cases_of(parse)
        if not cases:
            continue
        common = cases if common is None else (common & cases)
    if not common or len(common) != 1:
        return None, False
    return next(iter(common)), True


def _is_plural(value: str, type_: str) -> bool:
    """Множественное число исходной формы — только по ведущему разбору.

    Верхние разборы русских имён часто содержат «мусорную» форму
    множественного числа («Иван» → «иваны»), поэтому смотрим только первый
    разбор: иначе именительное ФИО считалось бы множественным и суффикс
    «,мн» ставился бы ошибочно.
    """
    if type_ in ("PERSON", "SURNAME"):
        parts = person_parts(value)
        words = [w for w in (parts["last"], parts["first"]) if w]
    else:
        words = value.split()
    for word in words:
        if not word or word.isascii() or len(word.rstrip(".")) == 1:
            continue
        parses = _parses(word)
        if not parses:
            continue
        parse = parses[0]
        if "plur" in parse.tag and (
            _is_surn(parse) or parse.tag.POS in ("NOUN", "ADJF")
        ):
            return True
    return False


def case_of(value: str, type_: str) -> tuple[Optional[str], bool]:
    """Падеж исходной формы: `(падеж, однозначно)`.

    Падеж считается однозначным, только если (а) верхние разборы всех
    значимых слов согласованы и (б) обратная генерация из нормальной формы
    даёт исходную запись. Иначе суффикс не ставится, подстановка помечается
    «проверить» (TR-2.4, правка П-12).
    """
    value = (value or "").strip()
    if not value:
        return None, False

    if type_ in ("PERSON", "SURNAME"):
        parts = person_parts(value)
        words = [w for w in (parts["last"], parts["first"], parts["middle"]) if w]
        if not words:
            words = value.split()
        gender = parts["gender"]
    elif type_ in ("ORG", "LOC"):
        if any(char in value for char in _QUOTE_CHARS) or len(value.split()) != 1:
            return None, False
        words = [value]
        gender = None
    else:
        return None, False

    case, confident = _case_intersection(words)
    if not confident or case is None:
        return None, False

    plural = _is_plural(value, type_)
    # Проверка обратимости: генерация этой формы должна совпасть с исходной
    # записью. Отсекает «Иванова» (род/вин), «Твери» (род/дат/пред) и прочие
    # неоднозначные разборы.
    if type_ in ("PERSON", "SURNAME"):
        pieces = [
            _word_forms(word, gender).get((case, plural)) or word
            for word in words
        ]
        generated = " ".join(pieces)
    else:
        generated = _word_forms(value, gender).get((case, plural)) or ""
    if generated.lower() != value.lower():
        return None, False
    return case, True


def plural_of(value: str, type_: str) -> bool:
    """Признак множественного числа исходной формы (для суффикса «,мн»)."""
    return _is_plural(value, type_)


def gram_of(value: str, type_: str) -> Optional[str]:
    """Граммема исходной формы: `gent` / `gent,мн` / None (неоднозначно)."""
    case, confident = case_of(value, type_)
    if not confident or case is None:
        return None
    return f"{case},{PLURAL_GRAM}" if plural_of(value, type_) else case


def suffix_for(case: Optional[str], plural: bool = False) -> str:
    """Суффикс метки по падежу: ``|дат`` / ``|род,мн`` (пусто для им. ед.ч.)."""
    if case is None:
        return ""
    base = CASE_SUFFIX.get(case, "")
    if plural:
        return f"{base},{PLURAL_GRAM}" if base else f",{PLURAL_GRAM}"
    return base


def case_suffix(value: str, type_: str) -> tuple[str, bool]:
    """Суффикс падежа значения: `(суффикс, однозначно)` (TR-2.3, П-12)."""
    case, confident = case_of(value, type_)
    if not confident or case is None:
        return "", False
    return suffix_for(case, _is_plural(value, type_)), True


def parse_gram(gram: Optional[str]) -> tuple[Optional[str], bool]:
    """`'gent,мн'` → `('gent', True)`; ``None``/пусто → `(None, False)`."""
    if not gram:
        return None, False
    parts = [part.strip() for part in gram.split(",") if part.strip()]
    case = next((part for part in parts if part in CASES), None)
    return case, PLURAL_GRAM in parts


def gram_from_suffix(suffix: Optional[str]) -> Optional[str]:
    """Суффикс метки → граммема базы: `'дат'` → `'datv'`, `'род,мн'` → `'gent,мн'`.

    В метках и файлах суффикс русский (TR-2.3: `|дат`), а в `forms.gram`
    хранится код pymorphy — это разные пространства, и путать их нельзя.
    """
    if not suffix:
        return None
    parts = [part.strip() for part in suffix.split(",") if part.strip()]
    case = next((SUFFIX_CASE[part] for part in parts if part in SUFFIX_CASE), None)
    if case is None:
        return None
    return f"{case},{PLURAL_GRAM}" if PLURAL_GRAM in parts else case


def inflect_value(
    value: str, type_: str, case: Optional[str], plural: bool = False
) -> Optional[str]:
    """Значение в заданном падеже/числе (для `derived`-подстановки)."""
    if not value or case is None:
        return value
    if type_ in ("PERSON", "SURNAME"):
        parts = person_parts(value)
        gender = parts["gender"]
        words = [w for w in (parts["last"], parts["first"], parts["middle"]) if w]
        if not words:
            return None
        pieces = []
        for word in words:
            pieces.append(_word_forms(word, gender).get((case, plural)) or word)
        return " ".join(pieces)
    if len(value.split()) != 1:
        return None
    return _word_forms(value, None).get((case, plural))
