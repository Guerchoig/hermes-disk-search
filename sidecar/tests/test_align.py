"""Alignment P2.2: сопоставление слов, hybrid-правило, fallback без whisperx."""
from __future__ import annotations

import unittest

from hds_transcribe import align
from hds_transcribe.errors import JobError
from hds_transcribe.params import AlignParams


def word(text, start, end):
    return {"text": text, "start": start, "end": end}


def aligner_word(text, start, end, score=0.9):
    return {"word": text, "start": start, "end": end, "score": score}


class MapSegmentTest(unittest.TestCase):
    def test_equal_words_get_aligner_times(self):
        asr = [word("привет", 0.0, 0.4), word("мир", 0.5, 0.8)]
        ali = [aligner_word("привет", 0.1, 0.5), aligner_word("мир", 0.6, 0.9)]
        mapping = align.map_segment(asr, ali)
        self.assertEqual(sorted(mapping), [0, 1])
        self.assertAlmostEqual(mapping[0]["start"], 0.1)
        self.assertAlmostEqual(mapping[1]["end"], 0.9)

    def test_mismatched_word_is_not_mapped(self):
        # «б» нет у aligner'а → слово остаётся нативным (никакого наследования).
        asr = [word("а", 0.0, 0.3), word("б", 0.4, 0.7), word("в", 0.8, 1.1)]
        ali = [aligner_word("а", 0.0, 0.3), aligner_word("в", 0.8, 1.1)]
        mapping = align.map_segment(asr, ali)
        self.assertIn(0, mapping)
        self.assertNotIn(1, mapping)
        self.assertIn(2, mapping)

    def test_multi_token_word_uses_span_of_all_tokens(self):
        asr = [word("1С", 0.0, 0.5), word("отчёт", 0.6, 1.0)]
        ali = [aligner_word("один", 0.0, 0.2), aligner_word("эс", 0.25, 0.45),
               aligner_word("отчёт", 0.6, 1.0)]
        mapping = align.map_segment(asr, ali)
        self.assertAlmostEqual(mapping[0]["start"], 0.0)
        self.assertAlmostEqual(mapping[0]["end"], 0.45)

    def test_partially_matched_multi_token_word_stays_native(self):
        asr = [word("1С", 0.0, 0.5)]
        ali = [aligner_word("один", 0.0, 0.2)]
        self.assertEqual(align.map_segment(asr, ali), {})

    def test_no_aligner_words(self):
        self.assertEqual(align.map_segment([word("а", 0.0, 0.1)], []), {})


class ApplyTimesTest(unittest.TestCase):
    def segments(self):
        return [
            {
                "start": 0.0,
                "end": 2.0,
                "text": "привет мир",
                "words": [word("привет", 0.0, 0.4), word("мир", 0.5, 0.8)],
            }
        ]

    def test_whisperx_mode_replaces_all_mapped(self):
        ali = [[aligner_word("привет", 0.1, 0.5), aligner_word("мир", 0.6, 0.9)]]
        segments = self.segments()
        info = align._apply_times(segments, ali, AlignParams(mode="whisperx"), "whisperx")
        words = segments[0]["words"]
        self.assertAlmostEqual(words[0]["start"], 0.1)
        self.assertAlmostEqual(words[1]["end"], 0.9)
        self.assertEqual(words[0]["alignment_source"], "whisperx")
        self.assertEqual(info["whisperx_share"], 1.0)
        self.assertAlmostEqual(info["mean_shift_ms"], 100.0, places=1)

    def test_hybrid_keeps_native_when_shift_is_small(self):
        # Расхождение 50 мс < max_shift 0.35 и слово aligned → остаётся native.
        ali = [[aligner_word("привет", 0.05, 0.45), aligner_word("мир", 0.55, 0.85)]]
        segments = self.segments()
        info = align._apply_times(segments, ali, AlignParams(mode="hybrid"), "hybrid")
        words = segments[0]["words"]
        self.assertAlmostEqual(words[0]["start"], 0.0)
        self.assertNotIn("alignment_source", words[0])
        self.assertEqual(info["whisperx_share"], 0.0)

    def test_hybrid_takes_whisperx_for_unreliable_word(self):
        # Второе слово короче min_duration (0.02) → берём whisperx-таймкод.
        segments = [
            {
                "start": 0.0,
                "end": 2.0,
                "text": "привет мир",
                "words": [word("привет", 0.0, 0.4), word("мир", 0.41, 0.42)],
            }
        ]
        ali = [[aligner_word("привет", 0.0, 0.4), aligner_word("мир", 0.5, 0.9)]]
        info = align._apply_times(segments, ali, AlignParams(mode="hybrid"), "hybrid")
        words = segments[0]["words"]
        self.assertNotIn("alignment_source", words[0])
        self.assertAlmostEqual(words[1]["start"], 0.5)
        self.assertEqual(info["words_shifted"], 1)

    def test_degenerate_aligner_span_keeps_native(self):
        # whisperx иногда даёт span короче кадра (20 мс) — не переносим.
        segments = self.segments()
        ali = [[aligner_word("привет", 0.2, 0.21), aligner_word("мир", 0.6, 0.9)]]
        info = align._apply_times(segments, ali, AlignParams(mode="whisperx"), "whisperx")
        words = segments[0]["words"]
        self.assertAlmostEqual(words[0]["start"], 0.0)  # native остался
        self.assertAlmostEqual(words[1]["start"], 0.6)  # нормальный span взят
        self.assertEqual(info["words_shifted"], 1)

    def test_hybrid_takes_whisperx_on_large_shift(self):
        ali = [[aligner_word("привет", 0.6, 1.0), aligner_word("мир", 1.1, 1.4)]]
        segments = self.segments()
        align._apply_times(segments, ali, AlignParams(mode="hybrid"), "hybrid")
        self.assertAlmostEqual(segments[0]["words"][0]["start"], 0.6)


class ApplyFallbackTest(unittest.TestCase):
    def test_native_returns_info_without_whisperx(self):
        original = align.available
        align.available = lambda: False
        try:
            _, warnings, info = align.apply([], AlignParams(mode="native"))
        finally:
            align.available = original
        self.assertEqual(warnings, [])
        self.assertEqual(info["mode_used"], "native")

    def test_hybrid_without_whisperx_warns_and_falls_back(self):
        original = align.available
        align.available = lambda: False
        try:
            _, warnings, info = align.apply([], AlignParams(mode="hybrid"), audio=[0.0])
        finally:
            align.available = original
        self.assertEqual(info["mode_used"], "native")
        self.assertTrue(any("fallback на native" in w for w in warnings))

    def test_explicit_whisperx_without_package_raises(self):
        original = align.available
        align.available = lambda: False
        try:
            with self.assertRaises(JobError) as ctx:
                align.apply([], AlignParams(mode="whisperx"), audio=[0.0])
        finally:
            align.available = original
        self.assertEqual(ctx.exception.code, "align_failed")

    def test_unknown_mode_warns(self):
        _, warnings, info = align.apply([], AlignParams(mode="magic"))
        self.assertEqual(info["mode_used"], "native")
        self.assertTrue(any("неизвестен" in w for w in warnings))


if __name__ == "__main__":
    unittest.main()
