"""Блоки диаризации (15–30 мин) и сшивка меток между блоками (§4.4)."""
from __future__ import annotations

import unittest

from hds_transcribe.diarize_pyannote import map_labels, plan_blocks


class PlanBlocksTest(unittest.TestCase):
    def test_short_file_is_single_block(self):
        self.assertEqual(plan_blocks(600.0, 1200.0, 60.0), [(0.0, 600.0)])

    def test_77_min_split_into_four_blocks(self):
        blocks = plan_blocks(77 * 60.0, 20 * 60.0, 60.0)
        self.assertEqual(len(blocks), 4)
        self.assertEqual(blocks[0][0], 0.0)
        self.assertAlmostEqual(blocks[-1][1], 77 * 60.0, places=3)
        # перекрытие 60 с и никаких дыр
        for (_, end), (start, _) in zip(blocks, blocks[1:]):
            self.assertAlmostEqual(end - start, 60.0, places=3)
        for start, end in blocks:
            self.assertLessEqual(end - start, 20 * 60.0 + 1e-6)

    def test_empty_audio(self):
        self.assertEqual(plan_blocks(0.0, 1200.0, 60.0), [])


class MapLabelsTest(unittest.TestCase):
    def test_label_mapped_by_overlap_in_region(self):
        canonical = [
            {"start": 0.0, "end": 100.0, "speaker": "SPEAKER_00"},
            {"start": 100.0, "end": 200.0, "speaker": "SPEAKER_01"},
        ]
        local = [  # локальные метки блока начинаются с 0
            {"start": 60.0, "end": 110.0, "speaker": "SPEAKER_00"},
            {"start": 110.0, "end": 180.0, "speaker": "SPEAKER_01"},
        ]
        mapping = map_labels(local, canonical, 60.0, 120.0)
        self.assertEqual(mapping["SPEAKER_00"], "SPEAKER_00")
        self.assertEqual(mapping["SPEAKER_01"], "SPEAKER_01")

    def test_speaker_outside_region_not_mapped(self):
        canonical = [{"start": 0.0, "end": 100.0, "speaker": "SPEAKER_00"}]
        local = [{"start": 150.0, "end": 180.0, "speaker": "SPEAKER_07"}]
        self.assertEqual(map_labels(local, canonical, 60.0, 120.0), {})


if __name__ == "__main__":
    unittest.main()