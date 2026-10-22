"""Флаги слов и `alignment_quality` (DoD §16 п. 4)."""
from __future__ import annotations

import unittest

from hds_transcribe import quality
from hds_transcribe.params import AlignQuality


class NormalizeWordsTest(unittest.TestCase):
    def test_interpolates_when_no_words(self):
        words = quality.normalize_words(
            {"start": 0.0, "end": 2.0, "text": "раз два три"}, AlignQuality()
        )
        self.assertEqual([w["text"] for w in words], ["раз", "два", "три"])
        self.assertTrue(all(w["alignment"] == "interpolated" for w in words))
        self.assertAlmostEqual(words[0]["start"], 0.0)
        self.assertAlmostEqual(words[-1]["end"], 2.0)

    def test_flags(self):
        segment = {
            "start": 0.0,
            "end": 5.0,
            "text": "ok ok",
            "words": [
                {"text": "ok", "start": 0.0, "end": 0.5},
                {"text": "ok", "start": 0.55, "end": 3.0},  # разрыв > 1.5 с после
                {"text": "ok", "start": 3.1, "end": 3.11},  # короче 0,02 с
                {"text": "", "start": 3.2, "end": 3.3},  # пустое → failed
                {"text": "ok", "start": 4.0, "end": 3.9},  # отрицательная длина
            ],
        }
        words = quality.normalize_words(segment, AlignQuality())
        self.assertEqual(words[0]["alignment"], "aligned")
        self.assertEqual(words[2]["alignment"], "low_confidence")
        self.assertEqual(words[3]["alignment"], "failed")
        self.assertEqual(words[4]["alignment"], "failed")

    def test_gap_marks_next_word_low_confidence(self):
        segment = {
            "start": 0.0,
            "end": 5.0,
            "text": "a b",
            "words": [
                {"text": "a", "start": 0.0, "end": 0.5},
                {"text": "b", "start": 2.5, "end": 3.0},  # пауза 2,0 > 1,5
            ],
        }
        words = quality.normalize_words(segment, AlignQuality())
        self.assertEqual(words[1]["alignment"], "low_confidence")

    def test_summary_counts(self):
        words = [
            {"text": "a", "start": 0.0, "end": 0.5, "alignment": "aligned"},
            {"text": "b", "start": 0.6, "end": 0.8, "alignment": "interpolated"},
        ]
        summary = quality.summarize(words, AlignQuality())
        self.assertEqual(summary["aligned"], 1)
        self.assertEqual(summary["interpolated"], 1)
        self.assertAlmostEqual(summary["aligned_ratio"], 0.5)
        self.assertFalse(summary["ok"])


class GapFlagCascadeTest(unittest.TestCase):
    """P2.1: один разрыв не должен «залипать» на весь остаток сегмента."""

    def test_only_word_after_gap_is_flagged(self):
        segment = {
            "start": 0.0,
            "end": 8.0,
            "text": "a b c d",
            "words": [
                {"text": "a", "start": 0.0, "end": 0.5},
                {"text": "b", "start": 2.5, "end": 2.9},  # разрыв 2,0 > 1,5
                {"text": "c", "start": 3.0, "end": 3.4},  # разрыв 0,1
                {"text": "d", "start": 3.5, "end": 3.9},
            ],
        }
        words = quality.normalize_words(segment, AlignQuality())
        flags = [w["alignment"] for w in words]
        # P1 давал ['aligned', 'low_confidence', 'low_confidence', 'low_confidence']
        self.assertEqual(flags, ["aligned", "low_confidence", "aligned", "aligned"])
        self.assertEqual(words[1]["alignment_reason"], "gap")

    def test_reasons_for_short_overlap_and_bad_bounds(self):
        segment = {
            "start": 0.0,
            "end": 3.0,
            "text": "a b c d",
            "words": [
                {"text": "a", "start": 0.0, "end": 1.0},
                {"text": "b", "start": 0.95, "end": 1.2},  # конфликт 0,05 < 0,12
                {"text": "c", "start": 0.5, "end": 0.9},  # конфликт 0,3
                {"text": "d", "start": 1.3, "end": 1.31},  # короче 0,02 с
            ],
        }
        words = quality.normalize_words(segment, AlignQuality())
        self.assertEqual(words[1]["alignment"], "aligned")
        self.assertEqual(words[2]["alignment"], "low_confidence")
        self.assertEqual(words[2]["alignment_reason"], "timing_overlap")
        self.assertEqual(words[3]["alignment"], "low_confidence")
        self.assertEqual(words[3]["alignment_reason"], "short_duration")

    def test_overlap_tolerance_boundaries(self):
        # 0,10 с — в пределах допуска (0,12), 0,15 с — уже конфликт таймкодов.
        with_tolerance = {
            "start": 0.0,
            "end": 3.0,
            "text": "a b",
            "words": [
                {"text": "a", "start": 0.0, "end": 1.0},
                {"text": "b", "start": 0.9, "end": 1.4},
            ],
        }
        self.assertEqual(quality.normalize_words(with_tolerance, AlignQuality())[1]["alignment"],
                         "aligned")
        conflict = {
            "start": 0.0,
            "end": 3.0,
            "text": "a b",
            "words": [
                {"text": "a", "start": 0.0, "end": 1.0},
                {"text": "b", "start": 0.85, "end": 1.4},
            ],
        }
        flagged = quality.normalize_words(conflict, AlignQuality())[1]
        self.assertEqual(flagged["alignment"], "low_confidence")
        self.assertEqual(flagged["alignment_reason"], "timing_overlap")


class SummaryTest(unittest.TestCase):
    def test_gap_counted_inside_phrase_only(self):
        # Два блока: пауза 21 с между ними — не разрыв внутри фразы.
        first = [
            {"text": "a", "start": 0.0, "end": 0.5, "alignment": "aligned"},
            {"text": "b", "start": 0.6, "end": 0.9, "alignment": "aligned"},
        ]
        second = [
            {"text": "c", "start": 21.0, "end": 21.4, "alignment": "aligned"},
            {"text": "d", "start": 21.5, "end": 21.9, "alignment": "aligned"},
        ]
        summary = quality.summarize(first + second, AlignQuality(), phrases=[first, second])
        self.assertAlmostEqual(summary["max_gap_sec"], 0.1, places=3)
        self.assertEqual(summary["gap_flags"], 0)
        self.assertEqual(summary["phrases"], 2)
        # Без phrases — прежнее поведение (совместимость): разрыв по всему файлу.
        naive = quality.summarize(first + second, AlignQuality())
        self.assertAlmostEqual(naive["max_gap_sec"], 20.1, places=3)

    def test_ratios_and_gate(self):
        words = [
            {"text": "a", "start": 0.0, "end": 0.5, "alignment": "aligned"},
            {"text": "b", "start": 0.6, "end": 0.8, "alignment": "aligned"},
            {"text": "c", "start": 0.9, "end": 1.0, "alignment": "aligned"},
            {"text": "d", "start": 1.1, "end": 1.2, "alignment": "low_confidence",
             "alignment_reason": "gap"},
            {"text": "", "start": None, "end": None, "alignment": "failed",
             "alignment_reason": "empty"},
        ]
        summary = quality.summarize(words, AlignQuality())
        self.assertAlmostEqual(summary["aligned_ratio"], 0.6)
        self.assertAlmostEqual(summary["low_confidence_ratio"], 0.2)
        self.assertAlmostEqual(summary["failed_ratio"], 0.2)
        self.assertAlmostEqual(summary["unreliable_ratio"], 0.4)
        self.assertEqual(summary["reasons"], {"empty": 1, "gap": 1})
        self.assertFalse(summary["ok"])

    def test_ok_when_ratios_within_thresholds(self):
        words = [
            {"text": "w%d" % i, "start": i * 0.5, "end": i * 0.5 + 0.4,
             "alignment": "aligned"}
            for i in range(49)
        ]
        words.append({"text": "x", "start": 30.0, "end": 30.1,
                      "alignment": "low_confidence", "alignment_reason": "short_duration"})
        summary = quality.summarize(words, AlignQuality())
        self.assertAlmostEqual(summary["unreliable_ratio"], 0.02)
        self.assertTrue(summary["ok"])


if __name__ == "__main__":
    unittest.main()
