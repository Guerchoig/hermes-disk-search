"""Артефакты кэша: коллизии стемов не перезаписывают чужие transcript/diarization."""
from __future__ import annotations

import json
import os
import tempfile
import unittest

from hds_transcribe import cache
from hds_transcribe.params import TranscribeParams


def media(directory: str, name: str, body: bytes = b"RIFF....WAVE") -> str:
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, name)
    with open(path, "wb") as f:
        f.write(body)
    return path


class StemCollisionTest(unittest.TestCase):
    def test_same_stem_different_media_gets_suffix(self):
        with tempfile.TemporaryDirectory() as tmp:
            first = media(os.path.join(tmp, "a"), "meeting.mp3")
            second = media(os.path.join(tmp, "b"), "meeting.mp3", b"OTHER")
            state = os.path.join(tmp, "state")
            params = TranscribeParams(path=first, state_dir=state)
            base = cache.transcript_path(params, "meeting")
            os.makedirs(state, exist_ok=True)
            with open(base, "w", encoding="utf-8") as f:
                json.dump({"media": first}, f)
            other = cache.transcript_path(TranscribeParams(path=second, state_dir=state), "meeting")
            self.assertNotEqual(base, other)
            self.assertTrue(os.path.basename(other).startswith("meeting-"))

    def test_same_media_reuses_base(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = media(tmp, "meeting.mp3")
            state = os.path.join(tmp, "state")
            params = TranscribeParams(path=path, state_dir=state)
            base = cache.transcript_path(params, "meeting")
            os.makedirs(state, exist_ok=True)
            with open(base, "w", encoding="utf-8") as f:
                json.dump({"media": path}, f)
            self.assertEqual(cache.transcript_path(params, "meeting"), base)

    def test_diarization_key_collision_gets_suffix(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = media(tmp, "meeting.mp3")
            state = os.path.join(tmp, "state")
            params = TranscribeParams(path=path, state_dir=state)
            base = cache.diarization_path(params, "meeting", "a" * 64)
            os.makedirs(state, exist_ok=True)
            with open(base, "w", encoding="utf-8") as f:
                json.dump({"key": "a" * 64}, f)
            self.assertEqual(cache.diarization_path(params, "meeting", "a" * 64), base)
            other = cache.diarization_path(params, "meeting", "b" * 64)
            self.assertNotEqual(base, other)
            self.assertIn("-bbbbbbbb", os.path.basename(other))


if __name__ == "__main__":
    unittest.main()
