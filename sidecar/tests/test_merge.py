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


class BuildBlocksTest(unittest.TestCase):
    def test_speaker_change_splits(self):
        words = [
            {**word("a", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("b", 0.6, 1.0), "speaker": "SPEAKER_01"},
        ]
        blocks = merge.build_blocks(words)
        self.assertEqual([b["speaker"] for b in blocks], ["SPEAKER_00", "SPEAKER_01"])

    def test_pause_splits(self):
        words = [
            {**word("a", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("b", 1.5, 2.0), "speaker": "SPEAKER_00"},
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

    def test_sentence_end_splits(self):
        words = [
            {**word("конец.", 0.0, 0.5), "speaker": "SPEAKER_00"},
            {**word("Новое", 0.6, 1.0), "speaker": "SPEAKER_00"},
        ]
        self.assertEqual(len(merge.build_blocks(words)), 2)

    def test_forced_limit_splits(self):
        words = [
            {**word("x" * 300, 0.0, 1.0), "speaker": "SPEAKER_00"},
            {**word("y" * 300, 1.1, 2.0), "speaker": "SPEAKER_00"},
        ]
        self.assertEqual(len(merge.build_blocks(words)), 2)

    def test_punctuation_join(self):
        self.assertEqual(merge.join_words([{"text": "слово"}, {"text": ","}, {"text": "ещё"}]), "слово, ещё")

    def test_empty_turns_keep_text(self):
        words = [word("a", 0.0, 0.5), word("b", 0.6, 1.0)]
        blocks, stats = merge.merge_transcript(words, [], [])
        self.assertTrue(blocks)
        self.assertEqual(stats["unassigned_words"], 2)


if __name__ == "__main__":
    unittest.main()
