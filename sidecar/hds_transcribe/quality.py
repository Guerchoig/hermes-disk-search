"""Флаги качества слов и сводка `alignment_quality` (DoD §16 п. 4).

Флаги (`alignment`):
* `aligned` — таймкод есть, конечен, длительность ≥ `min_duration_sec`;
* `interpolated` — таймкода нет (ONNX без `word_timestamps`, слова без границ):
  распределён внутри сегмента, **не** доказательство смены спикера;
* `low_confidence` — слишком короткое слово, наложение на соседнее слово или
  разрыв больше `max_gap_sec` **внутри непрерывной речи**;
* `failed` — пустое слово, нечисловые/отрицательные границы: в блок попадает,
  но «точным» таймкодом не считается.

Причина флага пишется в `alignment_reason` (`short_duration`, `timing_overlap`,
`gap`, `empty`, `no_timestamps`, `non_monotonic`) — по ним
`alignment_quality.reasons` показывает, из чего сложились проценты.

**`timing_overlap` — это конфликт таймкодов** (слово начинается раньше, чем
закончилось предыдущее, больше чем на `overlap_tolerance_sec`), а **не**
перекрытая речь: перекрытие речи двух спикеров живёт в `Word.overlap`
(пересечение с turns ≥ 2 спикеров, ставит `merge`). Разные вещи — разные имена.

**P2.1 (исправление P1):** `prev_end` обновляется для каждого слова с конечным
`end`, а не только для `aligned`. В P1 один разрыв > `max_gap_sec` помечал
`low_confidence` и все следующие слова чанка (270 слов = 11,3 % на `meet0924`),
потому что `prev_end` оставался на слове до разрыва.

`failed`-словам не выдаём «точный» таймкод: границы остаются как есть (None),
рендер использует границы блока.
"""
from __future__ import annotations

import math
import re

from .params import AlignQuality

WORD_FLAGS = ("aligned", "interpolated", "low_confidence", "failed")

#: Машинные причины флагов (`Word.alignment_reason`).
REASON_EMPTY = "empty"
REASON_NO_TIMES = "no_timestamps"
REASON_NON_MONOTONIC = "non_monotonic"
REASON_SHORT = "short_duration"
#: Конфликт таймкодов двух слов (не перекрытая речь — она в `Word.overlap`).
REASON_OVERLAP = "timing_overlap"
REASON_GAP = "gap"

_TOKEN_RE = re.compile(r"\S+", re.UNICODE)


def _finite(value) -> bool:
    try:
        return value is not None and math.isfinite(float(value))
    except (TypeError, ValueError):
        return False


def tokenize(text: str):
    return _TOKEN_RE.findall(text or "")


def interpolate_words(text: str, start, end, word_id_start: int = 0):
    """Разложить текст сегмента по времени равномерно (без таймкодов ASR)."""
    tokens = tokenize(text)
    if not tokens:
        return []
    if not (_finite(start) and _finite(end)) or float(end) <= float(start):
        start, end = 0.0, 0.0
    start, end = float(start), float(end)
    step = (end - start) / len(tokens) if end > start else 0.0
    out = []
    for idx, token in enumerate(tokens):
        ws = start + idx * step
        we = start + (idx + 1) * step if step else ws
        out.append(
            {
                "word_id": word_id_start + idx,
                "text": token,
                "start": round(ws, 3),  # type: ignore[arg-type]
                "end": round(we, 3),
                "alignment": "interpolated",
            }
        )
    return out


def classify(start, end, prev_end, quality: AlignQuality):
    """Флаг слова по его границам: `(flag, reason)`.

    Одно место правил для `normalize_words` и `align.py` (hybrid решает,
    брать ли таймкод whisperx, по тем же критериям).
    """
    if not (_finite(start) and _finite(end)):
        return "interpolated", REASON_NO_TIMES
    start, end = float(start), float(end)
    if end < start:
        return "failed", REASON_NON_MONOTONIC
    if end - start < quality.min_duration_sec:
        return "low_confidence", REASON_SHORT
    if prev_end is not None and start < prev_end - quality.overlap_tolerance_sec:
        # Допуск 0,12 с = 3 кадра GigaAM: 0,04-дребезг не шумит, реальные
        # конфликты (0,2–0,9 с) видны (решение владельца 22.10.2026).
        return "low_confidence", REASON_OVERLAP
    if prev_end is not None and start - prev_end > quality.max_gap_sec:
        return "low_confidence", REASON_GAP
    return "aligned", None


def normalize_words(segment, quality: AlignQuality, word_id_start: int = 0):
    """Слова сегмента ASR → слова с флагами, `word_id` и `display_span`.

    `segment` — `{start, end, text, words:[{text,start,end}]}`. Если слов нет
    (ONNX-путь), они интерполируются по тексту сегмента.

    `display_span` — смещения слова в тексте сегмента (`[начало, конец]`), по ним
    UI/постобработка понимают, какой фрагмент текста соответствует слову.
    """
    raw = segment.get("words") or []
    seg_start = segment.get("start")
    seg_end = segment.get("end")
    if not raw:
        return interpolate_words(segment.get("text", ""), seg_start, seg_end, word_id_start)

    words = []
    prev_end = None
    cursor = 0
    segment_text = segment.get("text") or ""
    for idx, raw_word in enumerate(raw):
        text = (raw_word.get("text") or "").strip()
        start = raw_word.get("start")
        end = raw_word.get("end")
        word = {
            "word_id": word_id_start + idx,
            "text": text,
            "start": None,
            "end": None,
            "alignment": "aligned",
        }
        if not text:
            word["alignment"] = "failed"
            word["alignment_reason"] = REASON_EMPTY
        else:
            flag, reason = classify(start, end, prev_end, quality)
            word["alignment"] = flag
            if reason:
                word["alignment_reason"] = reason
            if flag == "interpolated":
                word["start"] = round(float(seg_start), 3) if _finite(seg_start) else None
                word["end"] = round(float(seg_end), 3) if _finite(seg_end) else None
            elif flag == "failed":
                # «Точный» таймкод не выдаём, но исходные границы сохраняем.
                word["start"] = round(float(start), 3) if _finite(start) else None
                word["end"] = round(float(end), 3) if _finite(end) else None
            else:
                word["start"] = round(float(start), 3)
                word["end"] = round(float(end), 3)
                # P2.1: prev_end обновляем всегда (P1: только для aligned → «залипание»).
                prev_end = float(end) if prev_end is None else max(prev_end, float(end))
        if text:
            pos = segment_text.find(text, cursor)
            if pos >= 0:
                word["display_span"] = [pos, pos + len(text)]
                cursor = pos + len(text)
        words.append(word)
    return words


def summarize(words, quality: AlignQuality, phrases=None) -> dict:
    """Сводка `alignment_quality` по словам всех сегментов.

    `phrases` — списки слов (блоки/реплики). Разрывы считаются **внутри фразы**:
    в P1 `max_gap_sec` считался по всему файлу и показывал 21 с (паузы между
    чанками VAD и смены спикеров), а не качество выравнивания. Если `phrases`
    не переданы, весь список слов считается одной фразой (совместимость).

    `ok` — одновременно `aligned_ratio >= min_aligned_ratio` и
    `(low_confidence + failed) / words <= max_unreliable_ratio` (гейт P2 ≤ 2 %).
    """
    counts = {flag: 0 for flag in WORD_FLAGS}
    reasons = {}
    for word in words:
        flag = word.get("alignment", "failed")
        counts[flag if flag in counts else "failed"] += 1
        reason = word.get("alignment_reason")
        if reason:
            reasons[reason] = reasons.get(reason, 0) + 1

    groups = phrases if phrases is not None else [words]
    max_gap = 0.0
    gap_flags = 0
    for group in groups:
        prev_end = None
        for word in group or []:
            if word.get("alignment") != "aligned":
                continue
            start, end = word.get("start"), word.get("end")
            if prev_end is not None and _finite(start):
                gap = float(start) - prev_end
                if gap > max_gap:
                    max_gap = gap
                if gap > quality.max_gap_sec:
                    gap_flags += 1
            if _finite(end):
                prev_end = float(end)

    total = len(words) or 1
    aligned_ratio = counts["aligned"] / total
    unreliable_ratio = (counts["low_confidence"] + counts["failed"]) / total
    return {
        "words": len(words),
        "phrases": len(groups),
        "aligned": counts["aligned"],
        "interpolated": counts["interpolated"],
        "low_confidence": counts["low_confidence"],
        "failed": counts["failed"],
        "aligned_ratio": round(aligned_ratio, 4),
        "interpolated_ratio": round(counts["interpolated"] / total, 4),
        "low_confidence_ratio": round(counts["low_confidence"] / total, 4),
        "failed_ratio": round(counts["failed"] / total, 4),
        "unreliable_ratio": round(unreliable_ratio, 4),
        "max_gap_sec": round(max_gap, 3),
        "gap_flags": gap_flags,
        "reasons": dict(sorted(reasons.items())),
        "ok": bool(
            aligned_ratio >= quality.min_aligned_ratio
            and unreliable_ratio <= quality.max_unreliable_ratio
        ),
    }


def is_evidence(word) -> bool:
    """Служит ли слово акустическим доказательством смены спикера (§4.5 п. 2)."""
    return word.get("alignment") == "aligned"
