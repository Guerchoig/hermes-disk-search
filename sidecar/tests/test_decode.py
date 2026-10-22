"""Каналы: корреляция L/R (auto → downmix/split), решение `auto_channel_decision`."""
from __future__ import annotations

import os
import shutil
import tempfile
import unittest
import wave

import numpy as np

from hds_transcribe.decode import (
    auto_channel_decision,
    plan_channels,
    probe,
    window_correlations,
)
from hds_transcribe.params import DecodeParams


def write_stereo(path: str, left, right, sample_rate: int = 16000) -> None:
    """Записать стерео PCM16 (L/R) — фикстура для ffprobe/ffmpeg."""
    frames = np.empty((len(left), 2), dtype=np.int16)
    frames[:, 0] = np.clip(np.asarray(left) * 32767.0, -32768, 32767).astype(np.int16)
    frames[:, 1] = np.clip(np.asarray(right) * 32767.0, -32768, 32767).astype(np.int16)
    with wave.open(path, "wb") as handle:
        handle.setnchannels(2)
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(frames.tobytes())


class CorrelationTest(unittest.TestCase):
    def test_identical_channels_are_dual_mono(self):
        rng = np.random.default_rng(0)
        left = rng.normal(0, 0.2, 16000 * 20).astype(np.float32)
        right = left.copy()
        corrs = window_correlations(left, right, 16000, 10.0)
        self.assertEqual(len(corrs), 2)
        self.assertTrue(all(c > 0.99 for c in corrs))

    def test_independent_channels_are_split(self):
        rng = np.random.default_rng(1)
        left = rng.normal(0, 0.2, 16000 * 20).astype(np.float32)
        right = rng.normal(0, 0.2, 16000 * 20).astype(np.float32)
        corrs = window_correlations(left, right, 16000, 10.0)
        self.assertTrue(all(abs(c) < 0.1 for c in corrs))

    def test_silence_windows_skipped(self):
        left = np.zeros(16000 * 10, dtype=np.float32)
        right = np.zeros(16000 * 10, dtype=np.float32)
        self.assertEqual(window_correlations(left, right, 16000, 10.0), [])


@unittest.skipUnless(
    shutil.which("ffmpeg") and shutil.which("ffprobe"), "нужны ffmpeg/ffprobe"
)
class AutoChannelDecisionTest(unittest.TestCase):
    """P2.4: `decode.channels: auto` на синтетике (код-путь, не приёмка)."""

    def _write(self, tmp: str, left, right) -> str:
        path = os.path.join(tmp, "stereo.wav")
        write_stereo(path, left, right)
        return path

    def test_dual_mono_goes_downmix(self):
        rng = np.random.default_rng(0)
        left = rng.normal(0, 0.2, 16000 * 12).astype(np.float32)
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, left, left)
            params = DecodeParams()
            info = probe(path, params)
            decision = auto_channel_decision(path, info, params)
            self.assertTrue(decision["dual_mono"])
            self.assertEqual(plan_channels(path, info, params).mode, "downmix")

    def test_independent_channels_go_split(self):
        rng = np.random.default_rng(1)
        left = rng.normal(0, 0.2, 16000 * 12).astype(np.float32)
        right = rng.normal(0, 0.2, 16000 * 12).astype(np.float32)
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, left, right)
            params = DecodeParams()
            info = probe(path, params)
            decision = auto_channel_decision(path, info, params)
            self.assertFalse(decision["dual_mono"])
            plan = plan_channels(path, info, params)
            self.assertEqual(plan.mode, "split")
            self.assertEqual(plan.channels, [0, 1])

    def test_forced_downmix_ignores_correlation(self):
        rng = np.random.default_rng(2)
        left = rng.normal(0, 0.2, 16000 * 12).astype(np.float32)
        right = rng.normal(0, 0.2, 16000 * 12).astype(np.float32)
        with tempfile.TemporaryDirectory() as tmp:
            path = self._write(tmp, left, right)
            params = DecodeParams(channels="downmix")
            info = probe(path, params)
            self.assertEqual(plan_channels(path, info, params).mode, "downmix")


if __name__ == "__main__":
    unittest.main()