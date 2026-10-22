"""Блоки диаризации (15–30 мин) и сшивка меток между блоками (§4.4)."""
from __future__ import annotations

import unittest

from hds_transcribe.diarize_pyannote import plan_blocks


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




if __name__ == "__main__":
    unittest.main()