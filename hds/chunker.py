"""Структурный чанкинг сегментов текста (Фаза 2 плана PLAN_INDEX_QUALITY.md).

Отличия от прежнего чанкера:
- рекурсивное разбиение «абзац → строка → предложение → слово» с целью ~size
  символов (границы по структуре, а не «ближайший \n во второй половине окна»);
- overlap — целые предложения (10–20 % размера чанка), а не «хвост N символов»,
  дублирующий текст посреди слова;
- чанк не склеивает сегменты с разными page/t_start/t_end — номер страницы PDF
  и таймкоды транскриптов остаются точными;
- сегмент может нести поле head (путь заголовков md/docx, например
  «# Раздел / ## Подраздел») — попадает в начало каждого чанка секции.
"""
import re

# разделители рекурсивного разбиения: от крупной структуры к мелкой
_SEPS = ("\n\n", "\n", ". ", "! ", "? ", " ", "")
_SENT_END_RE = re.compile(r"(?<=[.!?…])\s+|\n+")


def _split_keep(sep, text):
    """split по sep с сохранением разделителя в конце куска (без потерь)."""
    if not sep:
        return list(text)
    parts, start = [], 0
    while True:
        idx = text.find(sep, start)
        if idx == -1:
            parts.append(text[start:])
            return parts
        parts.append(text[start:idx + len(sep)])
        start = idx + len(sep)


def _split_text(text, size, sep_idx=0):
    """Рекурсивно разрезать text на куски <= size (одиночное «слово» длиннее
    size не режется посимвольно — отдаётся целиком)."""
    if len(text) <= size:
        return [text]
    for i in range(sep_idx, len(_SEPS)):
        parts = _split_keep(_SEPS[i], text)
        if len(parts) > 1:
            return _merge_parts(parts, size, i)
    return [text]


def _merge_parts(parts, size, sep_idx):
    """Склеить мелкие куски до <= size; крупные резать следующим разделителем."""
    out, buf = [], ""
    for p in parts:
        if not p:
            continue
        if len(p) > size:
            if buf:
                out.append(buf)
                buf = ""
            out.extend(_split_text(p, size, sep_idx + 1))
            continue
        if buf and len(buf) + len(p) > size:
            out.append(buf)
            buf = p
        else:
            buf += p
    if buf:
        out.append(buf)
    return [p for p in out if p.strip()]


def _overlap_tail(text, overlap, size):
    """Хвост предыдущего чанка целыми предложениями суммарной длины <= overlap.

    Последнее предложение берём, даже если оно чуть длиннее overlap (целостность
    важнее точного лимита), но не длиннее size//2 — иначе дубликат съест чанк."""
    if overlap <= 0:
        return ""
    sents = [s.strip() for s in _SENT_END_RE.split(text) if s and s.strip()]
    if not sents:
        return ""
    tail, total = [], 0
    for s in reversed(sents):
        if total + len(s) > overlap:
            break
        tail.append(s)
        total += len(s) + 1
    if not tail and len(sents[-1]) <= size // 2:
        tail.append(sents[-1])
    return " ".join(reversed(tail))


def make_chunks(segments, size=800, overlap=120):
    """segments: [{'text','page','t_start','t_end','head'?}] -> [{'text', ...}].

    Чанк не смешивает сегменты с разными page/t_start/t_end и сегменты разных
    секций (head) — метаданные и путь заголовков остаются точными."""
    chunks = []
    st = {"body": [],     # куски текущего чанка (head добавляется при записи)
          "len": 0,       # длина тела текущего чанка
          "meta": None,   # метаданные сегмента, открывшего чанк
          "head": None,   # путь заголовков текущей секции
          "hbudget": 0}   # длина head, учитываемая в бюджете каждого чанка секции

    def flush():
        text = "".join(st["body"]).strip()
        if text:
            if st["head"]:
                text = st["head"] + "\n" + text
            m = st["meta"]
            chunks.append({"text": text, "page": m["page"],
                           "t_start": m["t_start"], "t_end": m["t_end"]})
        st["body"], st["len"] = [], 0

    for seg in segments:
        text = (seg.get("text") or "").strip()
        if not text:
            continue
        page, t_start, t_end = seg.get("page"), seg.get("t_start"), seg.get("t_end")
        head = (seg.get("head") or "").strip() or None
        keys = (page, t_start, t_end)
        m = st["meta"]
        # смена страницы/таймкода/секции -> новый чанк (не смешивать метаданные)
        if m is None or keys != m["keys"] or head != st["head"]:
            if m is not None:
                flush()  # пустой буфер — ничего не запишет, только сброс
            st["meta"] = {"page": page, "t_start": t_start, "t_end": t_end,
                          "keys": keys}
            st["head"] = head
            st["hbudget"] = (len(head) + 1) if head else 0
        for piece in _split_text(text, size):
            if st["body"] and st["len"] + st["hbudget"] + len(piece) > size:
                flush()
                if chunks and overlap > 0:
                    tail = _overlap_tail(chunks[-1]["text"], overlap, size)
                    if tail:
                        st["body"].append(tail + "\n")
                        st["len"] += len(tail) + 1
            st["body"].append(piece)
            st["len"] += len(piece)
    if st["body"]:
        flush()
    return chunks