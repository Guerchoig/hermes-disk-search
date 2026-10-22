"""«Произнесённая форма» текста для CTC-aligner'а (WhisperX + wav2vec2).

`e2e_rnnt` отдаёт **нормализованный** текст: «1С», «25 %», «МЧД», «Excel»,
«2024». Словарь `jonatasgrosman/wav2vec2-large-xlsr-53-russian` — строчная
кириллица, поэтому подача e2e-текста в `align()` напрямую теряет именно те
слова, ради которых делается выравнивание (PLAN §4.3).

Модуль чистый (без torch/whisperx), поэтому тестируется без моделей:
* `to_spoken(text)` — текст в произносимой форме (строчная кириллица);
* `spoken_tokens(text)` — то же списком токенов (для `difflib`-сопоставления).

Правила:
* числа → количественные числительные («25» → «двадцать пять»);
* цифра+буквы («1С») → «один эс»;
* аббревиатуры заглавными («МЧД») → названия букв («эм че дэ»);
* латиница → транслитерация (для «Excel» — «эксель», словарь частых слов
  важнее буквального соответствия: `LATIN_WORDS`);
* символы `% № & $ € ₽ + = @ #` → слова;
* пунктуация выбрасывается, остальное — только кириллица в нижнем регистре.

Числительные — количественные, а не порядковые («в 2024 году» →
«в две тысячи двадцать четыре году»): CTC-модель выравнивает по буквам, а
разница в окончании затрагивает 2–3 символа.
"""
from __future__ import annotations

import re

#: Названия букв русского алфавита (для аббревиатур).
LETTER_NAMES = {
    "а": "а", "б": "бэ", "в": "вэ", "г": "гэ", "д": "дэ", "е": "е",
    "ё": "ё", "ж": "жэ", "з": "зэ", "и": "и", "й": "й", "к": "ка",
    "л": "эл", "м": "эм", "н": "эн", "о": "о", "п": "пэ", "р": "эр",
    "с": "эс", "т": "тэ", "у": "у", "ф": "эф", "х": "ха", "ц": "цэ",
    "ч": "че", "ш": "ша", "щ": "ща", "ъ": "твёрдый знак", "ы": "ы",
    "ь": "мягкий знак", "э": "э", "ю": "ю", "я": "я",
}

#: Транслитерация латиницы (приближение к русскому произношению).
LATIN_LETTERS = {
    "a": "а", "b": "б", "c": "к", "d": "д", "e": "е", "f": "ф", "g": "г",
    "h": "х", "i": "и", "j": "дж", "k": "к", "l": "л", "m": "м", "n": "н",
    "o": "о", "p": "п", "q": "к", "r": "р", "s": "с", "t": "т", "u": "у",
    "v": "в", "w": "в", "x": "кс", "y": "и", "z": "з",
}

#: Частые латинские слова/бренды в ИТ-записях: транслитерация по буквам даёт
#: «ексел» вместо «эксель», поэтому такие слова задаём явно.
LATIN_WORDS = {
    "excel": "эксель", "word": "ворд", "zoom": "зум", "teams": "тимс",
    "outlook": "аутлук", "jira": "джира", "confluence": "конфлюенс",
    "telegram": "телеграм", "whatsapp": "ватсап", "google": "гугл",
    "chrome": "хром", "windows": "виндоус", "linux": "линукс",
    "api": "апи", "url": "урл", "id": "ид", "pdf": "пдф", "csv": "цсв",
    "sql": "эс кю эл", "http": "аш ти ти пи", "https": "аш ти ти пи эс",
    "ok": "окей", "it": "ай ти", "qa": "кью эй", "hr": "эйч ар",
}

#: Названия латинских букв по-русски (аббревиатуры: «D» → «дэ», «G» → «гэ»).
LATIN_LETTER_NAMES = {
    "a": "эй", "b": "бэ", "c": "цэ", "d": "дэ", "e": "е", "f": "эф",
    "g": "гэ", "h": "аш", "i": "и", "j": "джей", "k": "ка", "l": "эл",
    "m": "эм", "n": "эн", "o": "о", "p": "пэ", "q": "кью", "r": "эр",
    "s": "эс", "t": "тэ", "u": "у", "v": "вэ", "w": "дабл ю", "x": "экс",
    "y": "игрек", "z": "зэт",
}

#: Символы, которые в речи произносятся словом.
SYMBOLS = {
    "%": "процентов", "№": "номер", "&": "и", "$": "долларов",
    "€": "евро", "₽": "рублей", "+": "плюс", "=": "равно", "@": "собака",
    "#": "решётка", "°": "градусов",
}

_UNITS = ("", "один", "два", "три", "четыре", "пять", "шесть", "семь", "восемь", "девять")
_UNITS_F = ("", "одна", "две", "три", "четыре", "пять", "шесть", "семь", "восемь", "девять")
_TEENS = (
    "десять", "одиннадцать", "двенадцать", "тринадцать", "четырнадцать",
    "пятнадцать", "шестнадцать", "семнадцать", "восемнадцать", "девятнадцать",
)
_TENS = (
    "", "", "двадцать", "тридцать", "сорок", "пятьдесят", "шестьдесят",
    "семьдесят", "восемьдесят", "девяносто",
)
_HUNDREDS = (
    "", "сто", "двести", "триста", "четыреста", "пятьсот", "шестьсот",
    "семьсот", "восемьсот", "девятьсот",
)
_SCALES = (
    (1_000_000_000, ("миллиард", "миллиарда", "миллиардов")),
    (1_000_000, ("миллион", "миллиона", "миллионов")),
    (1_000, ("тысяча", "тысячи", "тысяч")),
)

_NUMBER_RE = re.compile(r"\d+(?:[.,]\d+)?")
_LATIN_RE = re.compile(r"[a-z]+", re.IGNORECASE)
_CYR_RE = re.compile(r"[а-яё]+", re.IGNORECASE)


def _plural(index: int, forms) -> str:
    """Форма числительного для `index` («1 миллион», «2 миллиона», «5 миллионов»)."""
    if 11 <= index % 100 <= 14:
        return forms[2]
    if index % 10 == 1:
        return forms[0]
    if 2 <= index % 10 <= 4:
        return forms[1]
    return forms[2]


def _triple_to_words(value: int, feminine: bool = False) -> list:
    """0..999 → слова (женский род — для тысяч: «одна тысяча»)."""
    out = []
    hundreds, rest = divmod(value, 100)
    if hundreds:
        out.append(_HUNDREDS[hundreds])
    if 10 <= rest <= 19:
        out.append(_TEENS[rest - 10])
        return out
    tens, unit = divmod(rest, 10)
    if tens:
        out.append(_TENS[tens])
    if unit:
        out.append((_UNITS_F if feminine else _UNITS)[unit])
    return out


def number_to_words(value: int) -> str:
    """Целое число → количественное числительное («1250» → «одна тысяча двести пятьдесят»)."""
    if value == 0:
        return "ноль"
    if value >= 10 ** 12:
        return " ".join("ноль" if ch == "0" else _UNITS[int(ch)] for ch in str(value))
    out = []
    rest = value
    for scale, forms in _SCALES:
        if rest >= scale:
            count, rest = divmod(rest, scale)
            out.extend(_triple_to_words(count, feminine=(scale == 1_000)))
            out.append(_plural(count, forms))
    if rest:
        out.extend(_triple_to_words(rest))
    return " ".join(out)


def _digits_to_words(digits: str) -> str:
    """Дробная часть: «25» → «двадцать пять», но «007» → «ноль ноль семь»."""
    if len(digits) > 1 and digits.startswith("0"):
        return " ".join("ноль" if ch == "0" else _UNITS[int(ch)] for ch in digits)
    return number_to_words(int(digits))


def _letter_name(ch: str) -> str:
    low = ch.lower()
    if low in LETTER_NAMES:
        return LETTER_NAMES[low]
    return LATIN_LETTER_NAMES.get(low, LATIN_LETTERS.get(low, ""))


def _latin_to_spoken(word: str) -> str:
    low = word.lower()
    if low in LATIN_WORDS:
        return LATIN_WORDS[low]
    if len(word) > 1 and word.isupper():
        # «API»-подобная латиница: названия букв («дэ», «гэ», «эс»)
        return " ".join(LATIN_LETTER_NAMES.get(ch, "") for ch in low)
    return "".join(LATIN_LETTERS.get(ch, "") for ch in low)


def _is_abbreviation(word: str) -> bool:
    """Похоже ли слово на аббревиатуру заглавными («МЧД», «РП»)."""
    letters = [ch for ch in word if ch.isalpha()]
    return len(letters) >= 2 and all(ch.isupper() for ch in letters)


def _cyrillic_word(word: str) -> str:
    if _is_abbreviation(word):
        return " ".join(LETTER_NAMES.get(ch.lower(), "") for ch in word if ch.isalpha())
    return word.lower()


def _expand_digit_letter(match) -> str:
    digit, letter = match.group(1), match.group(2)
    return "%s %s" % (number_to_words(int(digit)), _letter_name(letter))


def to_spoken(text: str) -> str:
    """Нормализованный текст → произносимая форма (строчная кириллица)."""
    text = text or ""
    for symbol, word in SYMBOLS.items():
        text = text.replace(symbol, " %s " % word)
    # «1 250 000» → «1250000» (разряды пробелом; иначе получаем «один двести…»)
    text = re.sub(r"(?<=\d)[\s\u00a0]+(?=\d{3}(?!\d))", "", text)
    # «1С» → «один эс», «2D» → «два дэ», «5G» → «пять гэ»
    text = re.sub(r"(\d)([A-Za-zА-Яа-яЁё])", _expand_digit_letter, text)
    text = re.sub(
        r"([A-Za-zА-Яа-яЁё])(\d)",
        lambda m: "%s %s" % (m.group(1), number_to_words(int(m.group(2)))),
        text,
    )

    out = []
    for token in re.split(r"[^0-9A-Za-zА-Яа-яЁё.,]+", text):
        token = token.strip(".,")
        if not token:
            continue
        if _NUMBER_RE.fullmatch(token):
            whole, _, frac = token.replace(",", ".").partition(".")
            out.append(number_to_words(int(whole)))
            if frac:
                out.append("запятая")
                out.append(_digits_to_words(frac))
            continue
        latin = "".join(_LATIN_RE.findall(token))
        cyr = _CYR_RE.findall(token)
        if latin and not cyr:
            spoken_part = _latin_to_spoken(latin)
            if spoken_part:
                out.append(spoken_part)
            continue
        if cyr:
            out.append(" ".join(_cyrillic_word(part) for part in cyr))
    return " ".join(part for part in out if part).strip()


def spoken_tokens(text: str) -> list:
    """Произносимая форма списком токенов."""
    return [token for token in to_spoken(text).split() if token]
