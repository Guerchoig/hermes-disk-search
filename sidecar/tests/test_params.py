"""Разбор параметров задания: параметры приходят из Rust (§5.2)."""
from __future__ import annotations

import unittest

from hds_transcribe.params import (
    AlignParams,
    AlignQuality,
    AsrParams,
    DecodeParams,
    DiarizationParams,
    TranscribeParams,
)


class TranscribeParamsTest(unittest.TestCase):
    def test_nested_sections_are_parsed(self):
        params = TranscribeParams.from_dict({
            "job_id": "j1",
            "path": "D:/media/a.mp3",
            "language": "ru",
            "align": {"mode": "native"},
            "decode": {"channels": "AUTO", "ffmpeg": "C:/ffmpeg.exe"},
            "asr": {"variant": "e2e_rnnt", "backend": "torch", "batch_size": "8"},
            "diarization": {"enabled": "false", "block_minutes": 50},
            "fallback_cpu": "true",
        })
        self.assertEqual(params.job_id, "j1")
        self.assertEqual(params.decode.channels, "auto")
        self.assertEqual(params.decode.ffmpeg, "C:/ffmpeg.exe")
        self.assertEqual(params.asr.batch_size, 8)
        self.assertFalse(params.diarization.enabled)
        self.assertTrue(params.fallback_cpu)

    def test_defaults(self):
        params = TranscribeParams.from_dict({})
        self.assertEqual(params.asr.variant, "e2e_rnnt")
        self.assertEqual(params.asr.backend, "torch")
        self.assertEqual(params.align.mode, "native")
        self.assertEqual(params.diarization.model, "pyannote/speaker-diarization-community-1")
        self.assertTrue(params.diarization.cache)
        self.assertEqual(params.diarization.block_minutes, 20.0)


class DiarizationParamsTest(unittest.TestCase):
    def test_block_minutes_clamped_to_15_30(self):
        self.assertEqual(DiarizationParams.from_dict({"block_minutes": 5}).block_minutes, 15.0)
        self.assertEqual(DiarizationParams.from_dict({"block_minutes": 90}).block_minutes, 30.0)
        self.assertEqual(DiarizationParams.from_dict({"block_minutes": 22}).block_minutes, 22.0)

    def test_num_speakers_overrides_min_max(self):
        params = DiarizationParams.from_dict({
            "num_speakers": 3, "min_speakers": 1, "max_speakers": 20,
        })
        self.assertEqual(params.pipeline_kwargs(), {"num_speakers": 3})

    def test_min_max_used_when_num_unknown(self):
        params = DiarizationParams.from_dict({"min_speakers": 2, "max_speakers": 6})
        self.assertEqual(params.pipeline_kwargs(), {"min_speakers": 2, "max_speakers": 6})


class AlignParamsTest(unittest.TestCase):
    def test_quality_defaults(self):
        quality = AlignParams.from_dict({"mode": "native"}).quality
        self.assertEqual(quality.max_gap_sec, 1.5)
        self.assertEqual(quality.min_duration_sec, 0.02)
        self.assertEqual(quality.overlap_tolerance_sec, 0.12)
        self.assertEqual(quality.min_aligned_ratio, 0.80)
        self.assertEqual(quality.max_unreliable_ratio, 0.02)

    def test_quality_from_rust_with_strings(self):
        quality = AlignQuality.from_dict({
            "max_gap_sec": "2.5",
            "min_duration_sec": 0.04,
            "max_unreliable_ratio": "0.01",
            "unknown_key": 1,
        })
        self.assertEqual(quality.max_gap_sec, 2.5)
        self.assertEqual(quality.min_duration_sec, 0.04)
        self.assertEqual(quality.max_unreliable_ratio, 0.01)
        self.assertEqual(quality.min_aligned_ratio, 0.80)


class AsrDecodeParamsTest(unittest.TestCase):
    def test_variant_and_backend_normalized(self):
        asr = AsrParams.from_dict({"variant": "E2E_RNNT", "backend": "ONNX", "batch_size": 0})
        self.assertEqual(asr.variant, "e2e_rnnt")
        self.assertEqual(asr.backend, "onnx")
        self.assertEqual(asr.batch_size, 1)  # 0 недопустим

    def test_unknown_variant_falls_back(self):
        self.assertEqual(AsrParams.from_dict({"variant": "whisper"}).variant, "e2e_rnnt")

    def test_decode_channels_normalized(self):
        self.assertEqual(DecodeParams.from_dict({"channels": "SPLIT"}).channels, "split")
        self.assertEqual(DecodeParams.from_dict({"channels": "bogus"}).channels, "auto")

    def test_unknown_vad_falls_back_to_silero(self):
        self.assertEqual(AsrParams.from_dict({"vad": "webrtc"}).vad, "silero")
        self.assertEqual(AsrParams.from_dict({"vad": "OFF"}).vad, "off")

    def test_sample_rate_is_always_16000(self):
        # ASR/VAD/диаризация не поддерживают другую частоту: значение из
        # конфига не должно молча сдвигать таймкоды.
        self.assertEqual(DecodeParams.from_dict({"sample_rate": 44100}).sample_rate, 16000)
        self.assertEqual(DecodeParams.from_dict({}).sample_rate, 16000)


if __name__ == "__main__":
    unittest.main()