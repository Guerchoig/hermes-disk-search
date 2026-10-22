"""`postprocess` (P3.2): замены по словам, edits.json, идемпотентность, лимиты."""
from __future__ import annotations

import json
import os
import tempfile
import unittest

from hds_transcribe import worker
from hds_transcribe.errors import JobError


def make_transcript(path: str, words) -> dict:
    """`transcript.json` в формате воркера (blocks + words + quality)."""
    blocks = [{
        "speaker": "SPEAKER_00",
        "start": 0.0,
        "end": 3.0,
        "text": " ".join(w["text"] for w in words),
        "words": words,
        "overlap": False,
    }]
    payload = {
        "job_id": "job-1",
        "media": "D:/media/meet.mp3",
        "blocks": blocks,
        "alignment_quality": {"words": len(words)},
        "stats": {"rtf": 0.02},
    }
    with open(path, "w", encoding="utf-8") as f:
        json.dump(payload, f, ensure_ascii=False, indent=2)
    return payload


def word(index, text):
    return {
        "word_id": index,
        "text": text,
        "start": float(index),
        "end": float(index) + 0.5,
        "alignment": "aligned",
        "speaker": "SPEAKER_00",
        "speakers": ["SPEAKER_00"],
        "overlap": False,
    }


WORDS = [word(0, "про"), word(1, "бухгалтерию"), word(2, "индинес,"),
         word(3, "и"), word(4, "отчёт")]

TERMS = [{
    "canonical": "1С",
    "variants": ["индинес"],
    "context": ["бухгалтерия"],
    "priority": "normal",
}]


class PostprocessTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.transcript = os.path.join(self.tmp.name, "meet.transcript.json")
        make_transcript(self.transcript, [dict(w) for w in WORDS])

    def run_postprocess(self, **params):
        request = {"job_id": "job-1", "transcript_path": self.transcript}
        request.update(params)
        return worker.postprocess(request)

    def test_empty_glossary_is_identical_to_p2(self):
        with open(self.transcript, encoding="utf-8") as f:
            before = json.load(f)
        result = self.run_postprocess()
        self.assertEqual(result["edits"], [])
        self.assertIsNone(result["edits_path"])
        self.assertEqual([b["text"] for b in result["blocks"]],
                         [b["text"] for b in before["blocks"]])
        self.assertEqual(result["text"], before["blocks"][0]["text"])
        self.assertFalse(os.path.exists(
            os.path.join(self.tmp.name, "meet.edits.json")))

    def test_glossary_replaces_words_and_writes_edits(self):
        result = self.run_postprocess(glossary={"terms": TERMS})
        self.assertEqual(result["blocks"][0]["words"][2]["text"], "1С,")
        self.assertEqual(result["blocks"][0]["text"], "про бухгалтерию 1С, и отчёт")
        self.assertEqual(len(result["edits"]), 1)
        edit = result["edits"][0]
        for key in ("word_id", "before", "after", "term", "reason", "needs_review"):
            self.assertIn(key, edit)
        self.assertEqual(edit["word_id"], 2)
        self.assertEqual(edit["before"], "индинес,")
        self.assertEqual(edit["after"], "1С")
        self.assertEqual(edit["reason"], "context")
        self.assertFalse(edit["needs_review"])
        self.assertTrue(os.path.isfile(result["edits_path"]))
        with open(result["edits_path"], encoding="utf-8") as f:
            saved = json.load(f)
        self.assertEqual(saved["stats"]["applied"], 1)
        self.assertEqual(saved["edits"], result["edits"])

    def test_transcript_json_keeps_asr_text_but_gets_postprocess_section(self):
        # Решение владельца: needs_review дублируется в transcript.json, но текст
        # ASR в файле не переписывается (замены живут в .md/edits.json).
        self.run_postprocess(glossary={"terms": TERMS})
        with open(self.transcript, encoding="utf-8") as f:
            saved = json.load(f)
        self.assertEqual(saved["blocks"][0]["words"][2]["text"], "индинес,")
        self.assertIn("postprocess", saved)
        self.assertEqual(saved["postprocess"]["stats"]["applied"], 1)
        self.assertEqual(saved["postprocess"]["needs_review"], [])

    def test_needs_review_is_duplicated_into_transcript_json(self):
        terms = [{"canonical": "1С", "variants": ["индинес"], "context": ["склад"]}]
        result = self.run_postprocess(glossary={"terms": terms})
        self.assertTrue(result["edits"][0]["needs_review"])
        with open(self.transcript, encoding="utf-8") as f:
            saved = json.load(f)
        word = saved["blocks"][0]["words"][2]
        self.assertEqual(word["text"], "индинес,")      # текст не тронут
        self.assertTrue(word.get("needs_review"))
        self.assertEqual(saved["postprocess"]["needs_review"][0]["word_id"], 2)
        # соседние слова не помечены
        self.assertFalse(saved["blocks"][0]["words"][1].get("needs_review", False))

    def test_removing_glossary_clears_review_markup(self):
        terms = [{"canonical": "1С", "variants": ["индинес"], "context": ["склад"]}]
        self.run_postprocess(glossary={"terms": terms})
        self.run_postprocess()  # глоссарий убрали
        with open(self.transcript, encoding="utf-8") as f:
            saved = json.load(f)
        self.assertNotIn("postprocess", saved)
        self.assertFalse(saved["blocks"][0]["words"][2].get("needs_review", False))

    def test_second_call_is_deterministic(self):
        # transcript.json не перезаписывается (источник — ASR), поэтому повторный
        # вызов даёт тот же текст и тот же edits.json; двойного применения нет.
        first = self.run_postprocess(glossary={"terms": TERMS})
        second = self.run_postprocess(glossary={"terms": TERMS})
        self.assertEqual(second["text"], first["text"])
        self.assertEqual(second["edits"], first["edits"])

    def test_timestamps_and_speakers_untouched(self):
        result = self.run_postprocess(glossary={"terms": TERMS})
        words = result["blocks"][0]["words"]
        self.assertEqual((words[2]["start"], words[2]["end"]), (2.0, 2.5))
        self.assertEqual(words[2]["speaker"], "SPEAKER_00")
        self.assertEqual(words[2]["alignment"], "aligned")

    def test_needs_review_without_context(self):
        # контекста «документооборот» рядом нет → текст не меняется, правка в review
        terms = [{"canonical": "1С", "variants": ["отчёт"], "context": ["документооборот"]}]
        result = self.run_postprocess(glossary={"terms": terms})
        self.assertEqual(result["blocks"][0]["words"][4]["text"], "отчёт")
        self.assertEqual(result["edits"][0]["reason"], "no_context")
        self.assertTrue(result["edits"][0]["needs_review"])

    def test_hotwords_none_reports_backend(self):
        result = self.run_postprocess(glossary={"terms": TERMS},
                                      hotwords={"backend": "none", "enabled": False})
        self.assertEqual(result["hotwords"]["backend"], "none")
        self.assertFalse(result["hotwords"]["applied_in_postprocess"])

    def test_hotword_limits_raise_bad_request(self):
        with self.assertRaises(JobError) as ctx:
            self.run_postprocess(
                hotwords={"enabled": True, "backend": "gigastt",
                          "phrases": ["x"] * 65},
            )
        self.assertEqual(ctx.exception.code, "bad_request")
        self.assertIn("too_many_hotwords", ctx.exception.message)

    def test_missing_transcript_is_bad_request(self):
        with self.assertRaises(JobError) as ctx:
            worker.postprocess({"transcript_path": self.transcript + ".absent"})
        self.assertEqual(ctx.exception.code, "bad_request")


if __name__ == "__main__":
    unittest.main()
