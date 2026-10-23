"""Unit-тесты merge (§4.5) — без моделей и аудио."""
from __future__ import annotations

import unittest

from hds_transcribe import merge


def word(text, start, end, alignment="aligned"):
    return {"word_id": 0, "text": text, "start": start, "end": end, "alignment": alignment}


TURNS = [
    {"start": 0.0, "end": 2.0, "speaker": "SPEAKER_00"},
    {"start": 2.0, "end": 4.0, "speaker": "SPEAKER_01"},
]


class AssignSpeakerTest(unittest.TestCase):
    def test_max_overlap_wins(self):
        words = [word("раз", 0.1, 0.6), word("два", 2.2, 2.6)]
        out, stats = merge.assign_speakers(words, TURNS)
        self.assertEqual([w["speaker"] for w in out], ["SPEAKER_00", "SPEAKER_01"])
        self.assertEqual(stats["unassigned_words"], 0)
        self.assertFalse(any(w["overlap"] for w in out))

    def test_word_outside_turns_is_unassigned(self):
        words = [word("тишина", 10.0, 10.5)]
        out, stats = merge.assign_speakers(words, TURNS)
        self.assertIsNone(out[0]["speaker"])
        self.assertEqual(stats["unassigned_words"], 1)

    def test_weak_overlap_is_unassigned(self):
        # 0,2 с из 2,2 с (< 50 %) — слово в основном вне turns
        turns = [{"start": 0.0, "end": 1.0, "speaker": "SPEAKER_00"}]
        out, _ = merge.assign_speakers([word("эхо", 0.8, 3.0)], turns)
        self.assertIsNone(out[0]["speaker"])

    def test_ambiguous_tie_is_flagged(self):
        # слово 1,0 с: 0,5 с у SPEAKER_00 и 0,48 с у SPEAKER_01 → разница < 10 %
        turns = [
            {"start": 0.0, "end": 1.5, "speaker": "SPEAKER_00"},
            {"start": 1.5, "end": 3.0, "speaker": "SPEAKER_01"},
        ]
        out, stats = merge.assign_speakers([word("спор", 1.0, 2.0)], turns)
        self.assertTrue(out[0]["ambiguous"])
        self.assertEqual(stats["ambiguous_words"], 1)
        self.assertIsNone(out[0]["speaker"])

    def test_low_confidence_does_not_move_speaker(self):
        words = [
            word("первое", 0.1, 0.6),
            word("шум", 3.0, 3.01, alignment="low_confidence"),
        ]
        out, _ = merge.assign_speakers(words, TURNS)
        self.assertEqual(out[0]["speaker"], "SPEAKER_00")
        self.assertEqual(out[1]["speaker"], "SPEAKER_00")  # сосед, не SPEAKER_01

    def test_interpolated_borrows_neighbour(self):
        words = [
            word("первое", 0.1, 0.6),
            word("вставка", None, None, alignment="interpolated"),
            word("третье", 2.2, 2.7),
        ]
        out, _ = merge.assign_speakers(words, TURNS)
        self.assertEqual(out[1]["speaker"], "SPEAKER_00")

    def test_overlap_flag_on_simultaneous_speech(self):
        turns = [
            {"start": 0.0, "end": 2.0, "speaker": "SPEAKER_00"},
            {"start": 0.0, "end": 2.0, "speaker": "SPEAKER_01"},
        ]
        out, stats = merge.assign_speakers([word("вместе", 0.2, 1.2)], turns)
        self.assertTrue(out[0]["overlap"])
        self.assertGreaterEqual(stats["overlap_words"], 1)

    def test_interpolated_only_uses_segment_fallback(self):
        # ONNX-путь: все слова interpolated, aligned-доказательств нет.
        # Без fallback весь текст уходил бы в UNASSIGNED (DML-профиль).
        words = [
            word("раз", 0.1, 0.6, alignment="interpolated"),
            word("два", 2.2, 2.6, alignment="interpolated"),
        ]
        out, stats = merge.assign_speakers(words, TURNS)
        self.assertEqual([w["speaker"] for w in out], ["SPEAKER_00", "SPEAKER_01"])
        self.assertEqual([w["speaker_source"] for w in out], ["segment", "segment"])
        self.assertEqual(stats["speaker_fallback_words"], 2)
        self.assertEqual(stats["unassigned_words"], 0)

    def test_segment_fallback_does_not_override_word_evidence(self):
        words = [
            word("первое", 0.1, 0.6),
            word("вставка", 3.0, 3.4, alignment="interpolated"),
        ]
        out, stats = merge.assign_speakers(words, TURNS)
        self.assertEqual(out[0]["speaker"], "SPEAKER_00")
        self.assertNotIn("speaker_source", out[0])
        self.assertEqual(stats["speaker_fallback_words"], 0)


class BuildBlocksTest(unittest.TestCase):
    def test_speaker_change_splits(self):
        words = [
            {**word("a", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("b", 0.6, 1.0), "speaker": "SPEAKER_01"},
        ]
        blocks = merge.build_blocks(words)
        self.assertEqual([b["speaker"] for b in blocks], ["SPEAKER_00", "SPEAKER_01"])

    def test_pause_splits(self):
        # Пауза больше `pause_sec` (дефолт 1.2 с) — новая реплика.
        words = [
            {**word("a", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("b", 1.9, 2.4), "speaker": "SPEAKER_00"},
        ]
        blocks = merge.build_blocks(words)
        self.assertEqual(len(blocks), 2)

    def test_short_gap_joins(self):
        words = [
            {**word("a", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("b", 0.7, 1.0), "speaker": "SPEAKER_00"},
        ]
        blocks = merge.build_blocks(words)
        self.assertEqual(len(blocks), 1)
        self.assertEqual(blocks[0]["text"], "a b")

    def test_sentence_end_joins_by_default(self):
        # `join_sentences: true` (дефолт): точка не режет монолог на абзацы.
        words = [
            {**word("конец.", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("Новое", 0.6, 1.0), "speaker": "SPEAKER_00"},
        ]
        self.assertEqual(len(merge.build_blocks(words)), 1)
        # Явный отказ — прежнее поведение.
        blocks = merge.build_blocks(words, merge.MergeOptions(join_sentences=False))
        self.assertEqual(len(blocks), 2)

    def test_forced_limit_splits(self):
        # `max_block_chars` по умолчанию 800: 500 + 500 символов > лимита.
        words = [
            {**word("x" * 500, 0.0, 1.0), "speaker": "SPEAKER_00"},
            {**word("y" * 500, 1.1, 2.0), "speaker": "SPEAKER_00"},
        ]
        self.assertEqual(len(merge.build_blocks(words)), 2)

    def test_punctuation_join(self):
        self.assertEqual(merge.join_words([{"text": "слово"}, {"text": ","}, {"text": "ещё"}]), "слово, ещё")

    def test_empty_turns_keep_text(self):
        words = [word("a", 0.0, 0.5), word("b", 0.6, 1.0)]
        blocks, stats = merge.merge_transcript(words, [], [])
        self.assertTrue(blocks)
        self.assertEqual(stats["unassigned_words"], 2)


class SmoothIslandsTest(unittest.TestCase):
    """Правило 6: короткий «островок» чужого спикера внутри фразы."""

    def _words(self, speakers):
        # Ровные слова по 0.4 с без пауз: важно только распределение спикеров.
        return [
            {**word("w%d" % i, i * 0.4, i * 0.4 + 0.35), "speaker": sp}
            for i, sp in enumerate(speakers)
        ]

    def test_single_word_island_is_reassigned(self):
        # «То» (SPEAKER_02) между словами SPEAKER_01 — как в расшифровке 24.09.
        words = self._words(["SPEAKER_01", "SPEAKER_01", "SPEAKER_02", "SPEAKER_01", "SPEAKER_01"])
        fixed = merge.smooth_islands(words)
        self.assertEqual(fixed, 1)
        self.assertEqual([w["speaker"] for w in words], ["SPEAKER_01"] * 5)
        self.assertEqual(words[2]["speaker_source"], "smoothed")

    def test_two_word_short_island_is_reassigned(self):
        words = self._words(
            ["SPEAKER_00", "SPEAKER_01", "SPEAKER_01", "SPEAKER_00", "SPEAKER_00"]
        )
        self.assertEqual(merge.smooth_islands(words), 2)
        self.assertEqual({w["speaker"] for w in words}, {"SPEAKER_00"})

    def test_long_island_is_kept(self):
        # Шесть слов другого спикера — это реальная реплика, не «островок».
        words = self._words(
            ["SPEAKER_00"] + ["SPEAKER_01"] * 6 + ["SPEAKER_00"]
        )
        self.assertEqual(merge.smooth_islands(words), 0)
        self.assertIn("SPEAKER_01", {w["speaker"] for w in words})

    def test_overlap_island_is_kept(self):
        # Перебивание помечено `overlap` — не трогаем.
        words = self._words(["SPEAKER_00", "SPEAKER_01", "SPEAKER_00"])
        words[1]["overlap"] = True
        self.assertEqual(merge.smooth_islands(words), 0)
        self.assertEqual(words[1]["speaker"], "SPEAKER_01")

    def test_complete_sentence_is_not_smoothed(self):
        # Однословный ответ «Да.» — полноценная реплика, а не «островок».
        words = self._words(["SPEAKER_00", "SPEAKER_01", "SPEAKER_00"])
        words[1]["text"] = "Да."
        self.assertEqual(merge.smooth_islands(words), 0)
        self.assertEqual(words[1]["speaker"], "SPEAKER_01")

    def test_sentence_is_not_split_between_speakers_after_smoothing(self):
        # Сквозной путь: слово-островок не должно порождать два блока.
        words = self._words(["SPEAKER_01", "SPEAKER_01", "SPEAKER_02", "SPEAKER_01", "SPEAKER_01"])
        self.assertEqual(merge.smooth_islands(words), 1)
        blocks = merge.build_blocks(words)
        self.assertEqual(len(blocks), 1, blocks)
        self.assertEqual({b["speaker"] for b in blocks}, {"SPEAKER_01"})
        self.assertEqual(blocks[0]["text"].startswith("w0 w1 w2 w3"), True)

    def test_merge_transcript_reports_smoothed_words(self):
        # `assign_speakers` берёт спикера из turns; «островок» — короткий turn
        # SPEAKER_02 внутри реплики SPEAKER_01 (как в записи 24.09).
        words = [
            word("И", 1.9, 1.98),
            word("То", 2.05, 2.15),
            word("есть", 2.2, 2.5),
            word("трогаем", 2.6, 3.0),
        ]
        turns = [
            {"start": 0.0, "end": 2.0, "speaker": "SPEAKER_01"},
            {"start": 2.0, "end": 2.18, "speaker": "SPEAKER_02"},
            {"start": 2.18, "end": 10.0, "speaker": "SPEAKER_01"},
        ]
        blocks, stats = merge.merge_transcript(words, turns, [])
        self.assertEqual(stats["smoothed_words"], 1)
        self.assertEqual(len(blocks), 1)
        self.assertEqual(blocks[0]["speaker"], "SPEAKER_01")


class MonologueTest(unittest.TestCase):
    """Правило 5: монолог одного спикера — один абзац, а не 18 реплик."""

    def _monologue(self, count=18, step=1.1, text="фраза."):
        # Как в расшифровке: фраза с точкой, пауза ~0.6–1.0 с, снова тот же спикер.
        words = []
        for i in range(count):
            start = i * step
            words.append({**word(text, start, start + 0.5), "speaker": "SPEAKER_02"})
        return words

    def test_monologue_collapses_into_one_block(self):
        words = self._monologue()
        blocks = merge.build_blocks(words)
        self.assertEqual(len(blocks), 1, [b["text"] for b in blocks])
        self.assertEqual(blocks[0]["start"], 0.0)
        self.assertGreater(blocks[0]["end"], 17.0)

    def test_max_block_sec_still_splits_long_monologue(self):
        words = self._monologue(count=60, step=1.0)
        blocks = merge.build_blocks(words, merge.MergeOptions(max_block_sec=20.0))
        self.assertGreater(len(blocks), 1)
        for block in blocks:
            self.assertLessEqual(block["end"] - block["start"], 21.0)

    def test_real_speaker_change_is_not_merged(self):
        words = [
            {**word("первый.", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("ответ.", 0.7, 1.2), "speaker": "SPEAKER_01"},
            {**word("снова.", 1.4, 1.9), "speaker": "SPEAKER_00"},
        ]
        blocks = merge.build_blocks(words)
        self.assertEqual([b["speaker"] for b in blocks], ["SPEAKER_00", "SPEAKER_01", "SPEAKER_00"])


if __name__ == "__main__":
    unittest.main()
