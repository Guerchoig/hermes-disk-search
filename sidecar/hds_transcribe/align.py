"""Alignment: `native` (word_timestamps GigaAM), `whisperx` и `hybrid` (P2).

`native` — default: нативные границы GigaAM в P0 дали MAE 65 мс относительно
WhisperX при +23 с и +1,95 ГБ VRAM на 18 мин, поэтому пересчёт таймкодов —
режим качества, а не MVP.

`whisperx` — выравниваем **произнесённую форму** текста ASR
(`spoken.to_spoken`: числа словами, «1С» → «один эс», «МЧД» → «эм че дэ»,
латиница → транслит) по сегментам ASR, без повторного распознавания:
wav2vec2 XLSR-53-ru получает те же сегменты с `text/start/end`, аудио —
уже декодированный массив 16 кГц (второго декодирования нет).

`hybrid` — база native; из whisperx берём таймкод слова, если нативное слово
не `aligned` (короткое/разрыв/наложение) или расхождение больше
`align.max_shift_sec`. Порог уходит в `alignment_quality.max_shift_sec`.

Сопоставление слов ASR ↔ aligner'а — `difflib.SequenceMatcher` по токенам
произнесённой формы: таймкод копируется только из **равных** блоков, поэтому
«никогда не наследуем таймкод по позиции при изменении числа слов»
(PLAN §4.3). Слово, у которого совпали не все токены, остаётся нативным.

Если whisperx недоступен (dml/cpu-venv, нет пакета): явный `whisperx` —
ошибка `align_failed`; `hybrid` — warning и нативные таймкоды (решение
владельца §6 п. 1).
"""
from __future__ import annotations

import importlib.util
import re
from difflib import SequenceMatcher
from typing import Dict, List, Optional, Tuple

from . import quality, spoken
from .errors import JobError
from .params import AlignParams

SUPPORTED = ("native", "whisperx", "hybrid")
#: Загруженные aligner'ы (модель живёт между заданиями одного воркера).
_MODEL_CACHE: Dict[Tuple[str, str, str], Tuple[object, dict]] = {}

_ALIGNER_TOKEN_RE = re.compile(r"[^а-яёa-z0-9]+")


def available() -> bool:
    """Есть ли `whisperx` в текущем venv (без тяжёлого импорта)."""
    try:
        return importlib.util.find_spec("whisperx") is not None
    except Exception:  # noqa: BLE001 - сломанный пакет не должен ронять hello
        return False


def _device(requested: str) -> str:
    if requested in ("cuda", "cpu"):
        return requested
    try:
        import torch

        return "cuda" if torch.cuda.is_available() else "cpu"
    except Exception:  # noqa: BLE001
        return "cpu"


def _load_aligner(params: AlignParams, device: str, language: str):
    """`whisperx.load_align_model` с кэшем: модель ~1,2 ГБ, грузим один раз."""
    import whisperx

    key = (params.model, device, language or "ru")
    cached = _MODEL_CACHE.get(key)
    if cached is not None:
        return cached
    model, metadata = whisperx.load_align_model(
        language_code=language or "ru", device=device, model_name=params.model
    )
    _MODEL_CACHE[key] = (model, metadata)
    return model, metadata


def _aligner_tokens(text: str) -> List[str]:
    """Токены слова aligner'а (он уже в произносимой форме)."""
    return [tok for tok in _ALIGNER_TOKEN_RE.split((text or "").lower()) if tok]


def map_segment(asr_words, aligner_words) -> Dict[int, dict]:
    """Сопоставить слова ASR со словами aligner'а.

    Возвращает `{индекс слова ASR: {start, end, score}}` только для слов, у
    которых **все** токены произнесённой формы нашли равное соответствие.
    Остальные слова остаются с нативным таймкодом (никакого наследования по
    позиции при другом числе слов).
    """
    tokens: List[str] = []
    owners: List[int] = []
    for idx, word in enumerate(asr_words or []):
        for token in spoken.spoken_tokens(word.get("text") or ""):
            tokens.append(token)
            owners.append(idx)
    targets: List[str] = []
    awords: List[dict] = []
    for aword in aligner_words or []:
        for token in _aligner_tokens(aword.get("word") or aword.get("text") or ""):
            targets.append(token)
            awords.append(aword)
    if not tokens or not targets:
        return {}

    per_word: Dict[int, int] = {}
    for owner in owners:
        per_word[owner] = per_word.get(owner, 0) + 1

    matched: Dict[int, List[Tuple[float, float, Optional[float]]]] = {}
    matcher = SequenceMatcher(None, tokens, targets, autojunk=False)
    for tag, i1, i2, j1, _j2 in matcher.get_opcodes():
        if tag != "equal":
            continue
        for offset in range(i2 - i1):
            owner = owners[i1 + offset]
            aword = awords[j1 + offset]
            start, end = aword.get("start"), aword.get("end")
            if start is None or end is None or float(end) <= float(start):
                continue
            matched.setdefault(owner, []).append(
                (float(start), float(end), aword.get("score"))
            )

    out: Dict[int, dict] = {}
    for owner, items in matched.items():
        if len(items) < per_word.get(owner, 0):
            continue
        scores = [item[2] for item in items if item[2] is not None]
        out[owner] = {
            "start": min(item[0] for item in items),
            "end": max(item[1] for item in items),
            "score": (sum(scores) / len(scores)) if scores else None,
        }
    return out


def _run_whisperx(segments, params: AlignParams, audio, language: str) -> List[list]:
    """WhisperX по произнесённой форме сегментов → слова по сегментам."""
    import numpy as np
    import whisperx

    device = _device(params.device)
    model, metadata = _load_aligner(params, device, language)
    payload = []
    slots = []
    for seg in segments:
        text = spoken.to_spoken(seg.get("text") or "")
        if not text:
            slots.append(None)
            continue
        item = {
            "start": float(seg.get("start") or 0.0),
            "end": float(seg.get("end") or 0.0),
            "text": text,
        }
        slots.append(item)
        payload.append(item)
    if not payload:
        return [[] for _ in segments]

    result = whisperx.align(
        payload,
        model,
        metadata,
        np.asarray(audio, dtype=np.float32),
        device,
        return_char_alignments=False,
        print_progress=False,
    )
    aligned = list(result.get("segments") or [])
    if len(aligned) != len(payload):
        raise RuntimeError(
            "whisperx вернул %d сегментов вместо %d" % (len(aligned), len(payload))
        )
    by_slot = iter(aligned)
    return [list(next(by_slot).get("words") or []) if slot else [] for slot in slots]


def _apply_times(segments, aligned, params: AlignParams, mode: str) -> dict:
    """Перенести таймкоды whisperx в слова ASR (для `hybrid` — по правилу)."""
    total = 0
    shifted = 0
    shifts: List[float] = []
    for seg, aligner_words in zip(segments, aligned):
        words = seg.get("words") or []
        mapping = map_segment(words, aligner_words)
        prev_end = None
        for idx, word in enumerate(words):
            total += 1
            start, end = word.get("start"), word.get("end")
            got = mapping.get(idx)
            if got is not None and (
                got["end"] - got["start"] < params.quality.min_duration_sec
            ):
                # Вырожденный таймкод aligner'а (короче кадра): слово остаётся
                # нативным — иначе сами создаём `low_confidence` (P2.2, whisperx:
                # 60 слов < 20 мс = 2,5 %).
                got = None
            use = False
            if got is not None:
                if mode == "whisperx":
                    use = True
                else:
                    flag, _reason = quality.classify(start, end, prev_end, params.quality)
                    shift = None
                    if start is not None and end is not None:
                        shift = max(abs(got["start"] - start), abs(got["end"] - end))
                    use = flag != "aligned" or shift is None or shift > params.max_shift_sec
            if use and got is not None:
                if start is not None:
                    shifts.append(got["start"] - start)
                word["start"] = round(got["start"], 3)
                word["end"] = round(got["end"], 3)
                word["alignment_source"] = "whisperx"
                if got.get("score") is not None:
                    word["aligner_score"] = round(float(got["score"]), 3)
                shifted += 1
            end = word.get("end")
            if end is not None:
                prev_end = end if prev_end is None else max(prev_end, end)
    share = shifted / total if total else 0.0
    return {
        "whisperx_share": round(share, 4),
        "words_shifted": shifted,
        "mean_shift_ms": round(sum(shifts) / len(shifts) * 1000.0, 1) if shifts else 0.0,
    }


def apply(
    segments: List[dict],
    params: AlignParams,
    audio=None,
    language: str = "ru",
) -> Tuple[List[dict], List[str], dict]:
    """Применить alignment к сегментам ASR.

    Возвращает `(segments, warnings, info)`; `info` попадает в
    `alignment_quality` (`mode_used`, `whisperx_share`, `mean_shift_ms`,
    `max_shift_sec`).
    """
    warnings: List[str] = []
    mode = (params.mode or "native").lower()
    info = {
        "mode_requested": mode,
        "mode_used": "native",
        "max_shift_sec": params.max_shift_sec,
        "whisperx_share": 0.0,
        "words_shifted": 0,
        "mean_shift_ms": 0.0,
    }
    if mode == "native":
        return segments, warnings, info
    if mode not in SUPPORTED:
        warnings.append("align.mode=%s неизвестен: используются нативные таймкоды" % mode)
        return segments, warnings, info

    message = None
    if not available():
        message = "align.mode=%s недоступен: whisperx не установлен в venv профиля" % mode
    elif audio is None:
        message = "align.mode=%s недоступен: нет декодированного аудио" % mode
    if message:
        if mode == "whisperx":
            raise JobError(
                "align_failed",
                message,
                "Установите whisperx в CUDA-venv или выберите align.mode: hybrid/native.",
            )
        warnings.append(message + " — fallback на native")
        return segments, warnings, info

    try:
        aligned = _run_whisperx(segments, params, audio, language)
    except JobError:
        raise
    except Exception as e:  # noqa: BLE001 - hybrid деградирует, whisperx падает
        text = "whisperx: %s: %s" % (type(e).__name__, e)
        if mode == "whisperx":
            raise JobError(
                "align_failed",
                text,
                "Проверьте align.model, кэш HF (HF_HOME) и доступность CUDA.",
            ) from e
        warnings.append(text + " — fallback на native")
        return segments, warnings, info

    info.update(_apply_times(segments, aligned, params, mode))
    info["mode_used"] = mode
    return segments, warnings, info
