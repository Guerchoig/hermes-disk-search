"""Лемматизация текста для FTS-индекса и запросов (pymorphy3, русская морфология).

chunks_fts хранит лемматизированный текст, запрос лемматизируется тем же способом
— разные словоформы («настройки»/«настройка», «скрипта»/«скрипты») находят друг
друга. Если pymorphy3 не установлен — деградация без падения: текст возвращается
как есть, а `hds.cli check` и UI показывают предупреждение (см. hds/diag.py).
"""
import re
import sys

TOKEN_RE = re.compile(r"[\w]{2,}", re.UNICODE)

_morph = None       # MorphAnalyzer или None
_state = None       # None = не инициализирован; True/False = доступен/нет
_cache = {}
_CACHE_MAX = 50000  # ограничение кэша словоформ


def _init():
    """Ленивая инициализация MorphAnalyzer (словари ~10 МБ, загрузка ~1 с)."""
    global _morph, _state
    if _state is not None:
        return _state
    try:
        import pymorphy3

        _morph = pymorphy3.MorphAnalyzer()
        _state = True
    except Exception as e:  # noqa: BLE001
        print("[lemmatizer] pymorphy3 недоступен (%s) — FTS без русской морфологии; "
              "установите: pip install pymorphy3 pymorphy3-dicts-ru" % e,
              file=sys.stderr)
        _state = False
    return _state


def available():
    """True, если pymorphy3 установлен и словарь загружается."""
    return bool(_init())


def lemmatize_token(token):
    """Лемма одного токена; при недоступности pymorphy3 — сам токен."""
    tok = token.lower()
    hit = _cache.get(tok)
    if hit is not None:
        return hit
    out = tok
    if _init() and len(tok) >= 2:
        try:
            parses = _morph.parse(tok)
            if parses and parses[0].normal_form:
                out = parses[0].normal_form.lower()
        except Exception:  # noqa: BLE001
            out = tok
    if len(_cache) < _CACHE_MAX:
        _cache[tok] = out
    return out


def normalize(text):
    """Лемматизированный текст через пробел (для chunks_fts и запросов)."""
    return " ".join(lemmatize_token(t) for t in TOKEN_RE.findall(text or ""))