"""Word-level merge: слова ASR × turns диаризации → блоки реплик (§4.5).

Чистые функции без моделей: тестируются на словарях (`pytest`/`unittest`).
Детерминированность важнее «красоты»: одинаковый вход → одинаковый `.md`.

Правила (§4.5, подтверждены владельцем):
1. `speaker = argmax(overlap)` по exclusive-turns;
2. максимальное пересечение `< 0.5 × длительности слова` → `UNASSIGNED`;
   разница первого и второго кандидата `< 10 %` длительности → `ambiguous`;
3. слово, пересекающее turns ≥2 спикеров, получает `overlap=true` (в `.md` —
   один `primary`, оба — в JSON);
4. `interpolated`/`low_confidence`/`failed` **не двигают** спикера: берём
   ближайшего соседа с `aligned`;
5. блоки режутся по смене спикера, паузе `> pause_sec` (1.2 с), при
   `join_sentences: false` — ещё и по границе предложения, и принудительно
   при `> max_block_sec`/`> max_block_chars` (45 с / 800 символов); соседние
   блоки одного спикера склеиваются при паузе `≤ join_gap_sec` (1.2 с), по
   умолчанию — и через границу предложения;
6. короткие «островки» чужого спикера (`< min_island_sec` или `≤ max_island_words`
   слов) между двумя словами одного спикера перекрашиваются в него
   (`speaker_source: smoothed`): pyannote иногда режет фразу посредине
   («То» / «есть, мы…»). Серии с `overlap=true` не трогаем.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, List, Optional, Sequence

from .quality import is_evidence

UNASSIGNED = "UNASSIGNED"


@dataclass
class MergeOptions:
    #: Пауза, после которой начинается новая реплика того же спикера.
    pause_sec: float = 1.2
    #: Пауза, при которой соседние блоки одного спикера склеиваются обратно.
    join_gap_sec: float = 1.2
    #: Склеивать блоки и через границу предложения (монолог — одним абзацем).
    join_sentences: bool = True
    max_block_sec: float = 45.0
    max_block_chars: int = 800
    #: «Островок» чужого спикера короче этого или не длиннее `max_island_words`
    #: слов перекрашивается в окружающего спикера.
    min_island_sec: float = 0.8
    max_island_words: int = 2
    min_speaker_share: float = 0.5
    ambiguity_margin: float = 0.10


def _intersection(a_start, a_end, b_start, b_end) -> float:
    if None in (a_start, a_end, b_start, b_end):
        return 0.0
    lo = max(float(a_start), float(b_start))
    hi = min(float(a_end), float(b_end))
    return max(0.0, hi - lo)


def _duration(word) -> float:
    start, end = word.get("start"), word.get("end")
    if start is None or end is None:
        return 0.0
    return max(0.0, float(end) - float(start))


def overlaps_by_speaker(word, turns) -> Dict[str, float]:
    """Суммарное пересечение слова с turns каждого спикера."""
    out: Dict[str, float] = {}
    for turn in turns or []:
        speaker = turn.get("speaker")
        if not speaker:
            continue
        value = _intersection(word.get("start"), word.get("end"), turn.get("start"), turn.get("end"))
        if value > 0.0:
            out[speaker] = out.get(speaker, 0.0) + value
    return out


def _direct_speaker(word, turns, opts: MergeOptions):
    """Прямое решение по пересечению: `(speaker, candidates, ambiguous)`."""
    overlaps = overlaps_by_speaker(word, turns)
    if not overlaps:
        return None, [], False
    ranked = sorted(overlaps.items(), key=lambda kv: (-kv[1], kv[0]))
    best, best_value = ranked[0]
    duration = _duration(word) or best_value
    candidates = [spk for spk, value in ranked if value > 0.0]
    if best_value < opts.min_speaker_share * duration:
        return None, candidates, False
    ambiguous = False
    if len(ranked) > 1:
        margin = best_value - ranked[1][1]
        if margin < opts.ambiguity_margin * duration:
            ambiguous = True
    return best, candidates, ambiguous


def _neighbour_speaker(words, index: int) -> Optional[str]:
    """Ближайший `aligned`-сосед со спикером: сначала назад, потом вперёд."""
    for step in (-1, 1):
        pos = index + step
        while 0 <= pos < len(words):
            candidate = words[pos]
            if is_evidence(candidate) and candidate.get("speaker"):
                return candidate["speaker"]
            pos += step
    return None


def assign_speakers(words, exclusive_turns, regular_turns=None, opts=None):
    """Проставить словам `speaker`, `speakers`, `ambiguous`, `overlap`.

    Возвращает `(words, stats)`; слова модифицируются на месте (копия не нужна:
    merge — последний шаг конвейера).
    """
    opts = opts or MergeOptions()
    regular_turns = regular_turns or []
    evidence_speakers = set()

    # Проход 1: прямые решения только для `aligned`-слов.
    for word in words:
        word.setdefault("alignment", "aligned")
        word["speaker"] = None
        word["speakers"] = []
        word["ambiguous"] = False
        word["overlap"] = False
        if not is_evidence(word):
            continue
        speaker, candidates, ambiguous = _direct_speaker(word, exclusive_turns, opts)
        word["speakers"] = candidates
        word["ambiguous"] = ambiguous
        word["overlap"] = len(candidates) >= 2
        if ambiguous or speaker is None:
            word["speaker"] = None
        else:
            word["speaker"] = speaker
            evidence_speakers.add(speaker)

    # Проход 2: неуверенные/не-aligned слова не двигают спикера — берут соседа.
    # Исключение (P3.1): если `aligned`-слов нет вообще (ONNX-путь не отдаёт
    # word timestamps, все слова `interpolated`), соседа-доказательства не
    # существует. Тогда атрибуция идёт по пересечению **сегмента** слова с
    # turns: она не считается доказательством смены спикера, но иначе весь
    # текст уходил бы в `UNASSIGNED` (DML/ONNX-профиль).
    fallback_words = 0
    if not evidence_speakers and exclusive_turns:
        for word in words:
            if word.get("speaker") is not None:
                continue
            speaker, candidates, ambiguous = _direct_speaker(word, exclusive_turns, opts)
            word["speakers"] = candidates
            word["ambiguous"] = ambiguous
            word["overlap"] = len(candidates) >= 2
            if speaker is not None and not ambiguous:
                word["speaker"] = speaker
                word["speaker_source"] = "segment"
                fallback_words += 1
    for index, word in enumerate(words):
        if word.get("speaker") is None:
            neighbour = _neighbour_speaker(words, index)
            word["speaker"] = neighbour
            if neighbour is not None:
                word.setdefault("speaker_source", "neighbour")

    unassigned = sum(1 for w in words if not w.get("speaker"))
    ambiguous = sum(1 for w in words if w.get("ambiguous"))
    overlap = sum(1 for w in words if w.get("overlap"))
    speakers = sorted({w["speaker"] for w in words if w.get("speaker")}) or sorted(evidence_speakers)
    stats = {
        "words": len(words),
        "unassigned_words": unassigned,
        "ambiguous_words": ambiguous,
        "overlap_words": overlap,
        "speaker_fallback_words": fallback_words,
        "speakers": speakers,
    }
    return words, stats


def _run_duration(words: Sequence[dict]) -> float:
    """Длительность серии слов по крайним таймкодам (0.0 — их нет)."""
    starts = [w.get("start") for w in words if w.get("start") is not None]
    ends = [w.get("end") for w in words if w.get("end") is not None]
    if not starts or not ends:
        return 0.0
    return max(0.0, float(max(ends)) - float(min(starts)))


def smooth_islands(words, opts: MergeOptions = None) -> int:
    """Перекрасить короткие «островки» чужого спикера (правило 6).

    pyannote ставит границу turn'а иногда **внутри** фразы: одно-два слова
    получают соседнего спикера, и `build_blocks` разрезает реплику надвое
    (пример: «То» — SPEAKER_02, «есть, мы, получается…» — SPEAKER_01).

    Серия слов спикера `S` перекрашивается в `P`, если:
    * она короче `min_island_sec` **или** не длиннее `max_island_words` слов;
    * слева и справа от неё — один и тот же спикер `P` (`P != S`);
    * ни одно слово серии не помечено `overlap` (там действительно перебивают).

    Возвращает число перекрашенных слов; у них выставляется
    `speaker_source = "smoothed"` (не путать с доказательством `aligned`).
    """
    opts = opts or MergeOptions()
    if not words:
        return 0
    fixed = 0
    # До трёх проходов: после перекраски двух соседних островков может
    # появиться новый (редко, но детерминированно).
    for _ in range(3):
        changed = 0
        index = 0
        total = len(words)
        while index < total:
            speaker = words[index].get("speaker")
            end = index + 1
            while end < total and words[end].get("speaker") == speaker:
                end += 1
            run = words[index:end]
            if speaker and index > 0 and end < total:
                left = words[index - 1].get("speaker")
                right = words[end].get("speaker")
                too_short = (
                    _run_duration(run) < opts.min_island_sec
                    or len(run) <= opts.max_island_words
                )
                overlaps = any(w.get("overlap") for w in run)
                # Завершённая фраза («Да.») — это полноценный ответ, не «островок».
                complete = _ends_sentence(run[-1].get("text") or "")
                if left and left == right and left != speaker and too_short \
                        and not overlaps and not complete:
                    for word in run:
                        word["speaker"] = left
                        word["speaker_source"] = "smoothed"
                    changed += len(run)
            index = end
        fixed += changed
        if not changed:
            break
    return fixed


def smooth_turn_tails(words, opts: MergeOptions = None) -> int:
    """Отдать следующему спикеру «хвост» реплики, продолжающий его фразу.

    pyannote иногда относит последнее слово turn'а к предыдущему спикеру:
    «…Угу. **То**» (SPEAKER_02) / «есть, мы, получается…» (SPEAKER_01). Внутри
    одной серии такое слово не «островок» (слева та же серия), поэтому правило
    [`smooth_islands`] его не видит.

    Условия переноса (все обязательны):
    * на границе смены спикера хвост предыдущей серии — слова без завершающей
      пунктуации (`.!?…`), не длиннее `max_island_words` и короче `min_island_sec`;
    * первое слово следующей серии начинается со **строчной** буквы (продолжение
      фразы, а не новое предложение);
    * в хвосте нет `overlap`.

    Возвращает число перенесённых слов; у них `speaker_source = "smoothed"`.
    """
    opts = opts or MergeOptions()
    if len(words) < 2:
        return 0
    moved = 0
    for index in range(1, len(words)):
        prev_speaker = words[index - 1].get("speaker")
        next_speaker = words[index].get("speaker")
        if not prev_speaker or not next_speaker or prev_speaker == next_speaker:
            continue
        first_text = (words[index].get("text") or "").lstrip()
        if not first_text or not first_text[0].islower():
            continue
        tail = []
        cursor = index - 1
        while cursor >= 0 and words[cursor].get("speaker") == prev_speaker:
            text = (words[cursor].get("text") or "").strip()
            if _ends_sentence(text) or words[cursor].get("overlap"):
                break
            tail.insert(0, words[cursor])
            if len(tail) > opts.max_island_words:
                break
            if _run_duration(tail) >= opts.min_island_sec:
                break
            cursor -= 1
        if not tail or len(tail) > opts.max_island_words:
            continue
        if _run_duration(tail) >= opts.min_island_sec:
            continue
        for word in tail:
            word["speaker"] = next_speaker
            word["speaker_source"] = "smoothed"
        moved += len(tail)
    return moved


def join_words(words: Sequence[dict]) -> str:
    """Собрать текст блока из слов, не ломая пунктуацию (`слово ,` → `слово,`)."""
    text = " ".join((w.get("text") or "").strip() for w in words if (w.get("text") or "").strip())
    for punct in (",", ".", "!", "?", ";", ":", "…", "»", ")"):
        text = text.replace(" " + punct, punct)
    for punct in ("«", "("):
        text = text.replace(punct + " ", punct)
    return text.strip()


def _ends_sentence(text: str) -> bool:
    return text.strip().endswith((".", "!", "?", "…"))


def _block_of(words: Sequence[dict]) -> dict:
    starts = [w["start"] for w in words if w.get("start") is not None]
    ends = [w["end"] for w in words if w.get("end") is not None]
    speaker = words[0].get("speaker") if words else None
    return {
        "speaker": speaker,
        "start": round(float(min(starts)), 3) if starts else None,
        "end": round(float(max(ends)), 3) if ends else None,
        "text": join_words(words),
        "words": list(words),
        "overlap": any(w.get("overlap") for w in words),
    }


def _fits(block: dict, extra_words: Sequence[dict], opts: MergeOptions) -> bool:
    """Помещается ли продолжение в блок (лимиты 30 с / 500 символов)."""
    if not block["words"] or not extra_words:
        return True
    first = block["words"][0]
    last = extra_words[-1]
    start = first.get("start")
    end = last.get("end")
    if start is not None and end is not None and float(end) - float(start) > opts.max_block_sec:
        return False
    text = join_words(list(block["words"]) + list(extra_words))
    return len(text) <= opts.max_block_chars


def build_blocks(words, opts: MergeOptions = None) -> List[dict]:
    """Разбить слова на блоки-реплики по правилам §4.5 п. 4."""
    opts = opts or MergeOptions()
    blocks: List[dict] = []
    current: List[dict] = []

    for word in words:
        if not current:
            current = [word]
            continue
        prev = current[-1]
        split = False
        if word.get("speaker") != prev.get("speaker"):
            split = True
        else:
            prev_end, start = prev.get("end"), word.get("start")
            if prev_end is not None and start is not None and float(start) - float(prev_end) > opts.pause_sec:
                split = True
            elif not opts.join_sentences and _ends_sentence(prev.get("text") or ""):
                split = True
            elif not _fits(_block_of(current), [word], opts):
                split = True
        if split:
            blocks.append(_block_of(current))
            current = [word]
        else:
            current.append(word)
    if current:
        blocks.append(_block_of(current))

    return _join_adjacent(blocks, opts)


def _join_adjacent(blocks: List[dict], opts: MergeOptions) -> List[dict]:
    """Склейка соседних блоков одного спикера при паузе ≤ `join_gap_sec`."""
    merged: List[dict] = []
    for block in blocks:
        if not merged:
            merged.append(block)
            continue
        prev = merged[-1]
        same = prev.get("speaker") == block.get("speaker")
        prev_end, start = prev.get("end"), block.get("start")
        small_gap = (
            prev_end is not None
            and start is not None
            and float(start) - float(prev_end) <= opts.join_gap_sec
        )
        # Границу предложения не «сшиваем» обратно только при
        # `join_sentences: false`: по умолчанию монолог идёт одним абзацем.
        sentence_break = (not opts.join_sentences) and _ends_sentence(prev.get("text") or "")
        if same and small_gap and not sentence_break \
                and _fits(prev, block["words"], opts):
            prev["words"] = prev["words"] + block["words"]
            prev["end"] = block["end"] if block["end"] is not None else prev["end"]
            prev["text"] = join_words(prev["words"])
            prev["overlap"] = prev.get("overlap") or block.get("overlap")
        else:
            merged.append(block)
    return merged


def merge_transcript(words, exclusive_turns, regular_turns=None, opts: MergeOptions = None):
    """Полный merge: спикеры по словам + склейка блоков. Возвращает `(blocks, stats)`."""
    opts = opts or MergeOptions()
    words, stats = assign_speakers(words, exclusive_turns, regular_turns, opts)
    # Правило 6: короткие «островки» чужого спикера внутри фразы — к соседям,
    # затем «хвосты» реплик, продолжающие фразу следующего спикера («То» / «есть»).
    stats["smoothed_words"] = smooth_islands(words, opts)
    stats["smoothed_tail_words"] = smooth_turn_tails(words, opts)
    blocks = build_blocks(words, opts)
    stats["blocks"] = len(blocks)
    stats["speakers"] = sorted(
        {b["speaker"] for b in blocks if b.get("speaker")} or set(stats["speakers"])
    )
    return blocks, stats
