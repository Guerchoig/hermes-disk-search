"""Чанкинг сегментов текста с сохранением страниц/таймкодов."""
import os


def _split_long(text, size):
    """Разрезать длинный текст по границам строк/абзацев на куски <= size."""
    out = []
    pos, n = 0, len(text)
    while pos < n:
        end = min(pos + size, n)
        if end < n:
            nl = text.rfind("\n", pos + size // 2, end)
            if nl > pos:
                end = nl + 1
        out.append(text[pos:end])
        pos = end
    return out


def make_chunks(segments, size=1200, overlap=200):
    """segments: [{'text','page','t_start','t_end'}] -> [{'text','page','t_start','t_end'}]"""
    chunks = []
    buf = ""
    meta = {}
    tail = ""

    def flush():
        nonlocal buf, meta, tail
        text = buf.strip()
        if text:
            chunks.append({
                "text": text,
                "page": meta.get("page"),
                "t_start": meta.get("t_start"),
                "t_end": meta.get("t_end"),
            })
            tail = text[-overlap:] if overlap > 0 else ""
        buf = ""
        meta = {}

    for seg in segments:
        text = (seg.get("text") or "").strip()
        if not text:
            continue
        for piece in _split_long(text, size):
            piece = piece.strip()
            if not piece:
                continue
            if not buf:
                meta = {
                    "page": seg.get("page"),
                    "t_start": seg.get("t_start"),
                    "t_end": seg.get("t_end"),
                }
                buf = (tail + "\n" + piece) if tail else piece
            else:
                buf = buf + "\n" + piece
            if len(buf) >= size:
                flush()
    if buf.strip():
        flush()
    return chunks