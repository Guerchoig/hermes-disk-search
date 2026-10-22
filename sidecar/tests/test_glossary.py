"""Глоссарий (P3.1/P3.2): нормализация, контекст, приоритеты, forbid, замены."""
from __future__ import annotations

import json
import os
import tempfile
import unittest

from hds_transcribe import glossary
from hds_transcribe.errors import JobError


def word(index, text, start=None, end=None, speaker="SPEAKER_00"):
    return {
        "word_id": index,
        "text": text,
        "start": start if start is not None else float(index),
        "end": end if end is not None else float(index) + 0.5,
        "alignment": "aligned",
        "speaker": speaker,
    }


def block(words):
    return {"speaker": "SPEAKER_00", "start": 0.0, "end": 10.0,
            "text": " ".join(w["text"] for w in words), "words": words}


def term(**kwargs):
    return kwargs


class NormalizeTest(unittest.TestCase):
    def test_case_and_yo(self):
        self.assertEqual(glossary.normalize("ЁЛКА"), glossary.normalize("Елка"))
        self.assertEqual(glossary.normalize("ёлка"), glossary.normalize("Елка"))

    def test_latin_cyrillic_1c(self):
        self.assertEqual(glossary.normalize("1C"), "1c")
        self.assertEqual(glossary.normalize("1С"), "1c")
        self.assertEqual(glossary.normalize("1c,"), glossary.normalize("1С"))

    def test_latin_and_cyrillic_are_not_conflated_beyond_homoglyphs(self):
        # «ЕРП» и «ERP» — разные написания (П ≠ P): связывает их только variants,
        # гомоглифы покрывают лишь визуально совпадающие буквы (1C/1С).
        self.assertNotEqual(glossary.normalize("ERP"), glossary.normalize("ЕРП"))

    def test_word_boundaries_keep_inner_hyphen(self):
        value = glossary.normalize("«ERP-система»")
        self.assertEqual(value, glossary.normalize("ERP-система"))
        self.assertTrue(value.startswith("erp-"))
        self.assertEqual(glossary.normalize("(индинес)"), glossary.normalize("индинес"))

    def test_tokens_for_multiword_variant(self):
        self.assertEqual(
            glossary.tokens("один  эс"),
            [glossary.normalize("один"), glossary.normalize("эс")],
        )


class LoadTest(unittest.TestCase):
    def test_unknown_keys_are_warnings_not_crash(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [{"canonical": "1С", "variants": ["индес"], "magic": 1}],
             "forbid": [], "extra_section": True}
        )
        self.assertEqual(len(gl.terms), 1)
        self.assertTrue(any("magic" in w for w in gl.warnings))
        self.assertTrue(any("extra_section" in w for w in gl.warnings))

    def test_missing_canonical_is_skipped(self):
        gl = glossary.Glossary.from_dict({"terms": [{"variants": ["x"]}]})
        self.assertEqual(gl.terms, [])
        self.assertTrue(gl.warnings)

    def test_yaml_file_loaded_utf8_without_bom(self):
        body = (
            'terms:\n'
            '  - canonical: "1С"\n'
            '    variants: ["один эс", "1c"]\n'
            '    context: ["бухгалтерия"]\n'
            '    priority: high\n'
            'forbid: []\n'
        )
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "glossary.yaml")
            with open(path, "w", encoding="utf-8") as f:
                f.write(body)
            gl = glossary.Glossary.load(path)
        self.assertEqual(len(gl.terms), 1)
        self.assertEqual(gl.terms[0].priority, "high")

    def test_missing_path_is_empty_glossary(self):
        gl = glossary.Glossary.load("")
        self.assertEqual(gl.terms, [])
        self.assertEqual(gl.warnings, [])

    def test_required_missing_file_raises_bad_request(self):
        with self.assertRaises(JobError) as ctx:
            glossary.Glossary.load("no-such-glossary.yaml", required=True)
        self.assertEqual(ctx.exception.code, "bad_request")


class ApplyTest(unittest.TestCase):
    def test_context_enables_replacement(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="1С", variants=["индинес"],
                            context=["бухгалтерия"], priority="normal")]}
        )
        blocks = [block([word(0, "в"), word(1, "бухгалтерии"), word(2, "индинес,"),
                         word(3, "настроен")])]
        edits, stats = gl.apply(blocks)
        self.assertEqual(blocks[0]["words"][2]["text"], "1С,")
        self.assertEqual(blocks[0]["text"], "в бухгалтерии 1С, настроен")
        self.assertEqual(edits[0]["reason"], "context")
        self.assertFalse(edits[0]["needs_review"])
        self.assertEqual(stats["applied"], 1)

    def test_no_context_needs_review_and_text_kept(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="1С", variants=["индинес"],
                            context=["бухгалтерия"], priority="normal")]}
        )
        blocks = [block([word(0, "про"), word(1, "индинес,"), word(2, "поговорим")])]
        before = json.dumps(blocks, ensure_ascii=False)
        edits, stats = gl.apply(blocks)
        self.assertEqual(json.dumps(blocks, ensure_ascii=False), before)
        self.assertEqual(edits[0]["reason"], "no_context")
        self.assertTrue(edits[0]["needs_review"])
        self.assertEqual(stats["applied"], 0)

    def test_priority_high_replaces_without_context(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="МЧД", variants=["эм че дэ"], priority="high")]}
        )
        blocks = [block([word(0, "нужна"), word(1, "эм"), word(2, "че"), word(3, "дэ")])]
        edits, _ = gl.apply(blocks)
        self.assertEqual(blocks[0]["text"], "нужна МЧД")
        self.assertEqual(edits[0]["reason"], "priority")
        self.assertEqual(blocks[0]["words"][2]["text"], "")
        self.assertEqual(blocks[0]["words"][3]["text"], "")

    def test_allow_global_replaces(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="ЗУП", variants=["зуп"], allow_global=True)]}
        )
        blocks = [block([word(0, "в"), word(1, "зуп"), word(2, "зарплата")])]
        edits, _ = gl.apply(blocks)
        self.assertEqual(blocks[0]["words"][1]["text"], "ЗУП")
        self.assertEqual(edits[0]["reason"], "allow_global")

    def test_forbid_variant_is_ignored(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="1С", variants=["один эс"], allow_global=True)],
             "forbid": ["один эс"]}
        )
        blocks = [block([word(0, "один"), word(1, "эс")])]
        before = blocks[0]["text"]
        edits, _ = gl.apply(blocks)
        self.assertEqual(blocks[0]["text"], before)
        self.assertEqual(edits, [])
        self.assertTrue(any("forbid" in w for w in gl.warnings))

    def test_case_sensitive(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="ERP", variants=["ерп"], case_sensitive=True,
                            allow_global=True)]}
        )
        blocks = [block([word(0, "ЕРП"), word(1, "ерп")])]
        gl.apply(blocks)
        # Регистр варианта значим: «ЕРП» не совпало с «ерп» и осталось как есть,
        # «ерп» совпало и получило канон «ERP».
        self.assertEqual(blocks[0]["words"][0]["text"], "ЕРП")
        self.assertEqual(blocks[0]["words"][1]["text"], "ERP")

    def test_latin_variant_matches_cyrillic_canonical(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="1С", variants=["1c"], allow_global=True)]}
        )
        blocks = [block([word(0, "в"), word(1, "1C."), word(2, "отчёте")])]
        gl.apply(blocks)
        self.assertEqual(blocks[0]["words"][1]["text"], "1С.")
        self.assertEqual(blocks[0]["text"], "в 1С. отчёте")

    def test_competing_terms_of_different_length_need_review(self):
        gl = glossary.Glossary.from_dict({"terms": [
            term(canonical="ЗУП", variants=["зуп"], priority="high"),
            term(canonical="ЗУП Зарплата", variants=["зуп зарплата"], priority="high"),
        ]})
        blocks = [block([word(0, "зуп"), word(1, "зарплата")])]
        before = blocks[0]["text"]
        edits, stats = gl.apply(blocks)
        self.assertEqual(blocks[0]["text"], before)
        self.assertEqual(stats["applied"], 0)
        self.assertEqual(len(edits), 2)
        self.assertTrue(all(e["needs_review"] for e in edits))
        self.assertTrue(all(e["reason"] == "ambiguous" for e in edits))

    def test_idempotent_second_pass(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="1С", variants=["индинес"], allow_global=True)]}
        )
        blocks = [block([word(0, "про"), word(1, "индинес")])]
        first_edits, _ = gl.apply(blocks)
        text_after_first = blocks[0]["text"]
        second_edits, stats = gl.apply(blocks)
        self.assertEqual(len(first_edits), 1)
        self.assertEqual(second_edits, [])
        self.assertEqual(stats["applied"], 0)
        self.assertEqual(blocks[0]["text"], text_after_first)

    def test_timestamps_speakers_and_ids_untouched(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="1С", variants=["индес"], priority="high")]}
        )
        words = [word(7, "индес", 1.25, 1.75, speaker="SPEAKER_02")]
        blocks = [block(words)]
        original = {k: words[0][k] for k in ("word_id", "start", "end", "speaker", "alignment")}
        gl.apply(blocks)
        self.assertEqual({k: words[0][k] for k in original}, original)

    def test_ambiguous_terms_need_review(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [
                term(canonical="Альфа", variants=["омега"], priority="high"),
                term(canonical="Бета", variants=["омега"], priority="high"),
            ]}
        )
        blocks = [block([word(0, "омега")])]
        edits, stats = gl.apply(blocks)
        self.assertEqual(blocks[0]["text"], "омега")
        self.assertEqual(stats["applied"], 0)
        self.assertEqual(edits[0]["reason"], "ambiguous")
        self.assertTrue(edits[0]["needs_review"])

    def test_empty_glossary_keeps_blocks(self):
        blocks = [block([word(0, "про"), word(1, "индинес")])]
        before = json.dumps(blocks, ensure_ascii=False)
        edits, stats = glossary.Glossary().apply(blocks)
        self.assertEqual(json.dumps(blocks, ensure_ascii=False), before)
        self.assertEqual(edits, [])
        self.assertEqual(stats["edits"], 0)

    def test_context_window_limit(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="1С", variants=["индес"], context=["бухгалтерия"])]}
        )
        far = [word(i, "слово") for i in range(glossary.CONTEXT_WINDOW_WORDS + 2)]
        words = [word(0, "бухгалтерия")] + far + [word(99, "индес")]
        blocks = [block(words)]
        gl.apply(blocks)
        self.assertEqual(blocks[0]["words"][-1]["text"], "индес")


class HotwordsTest(unittest.TestCase):
    def test_phrases_from_pronunciations_and_canonical(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [
                term(canonical="1С", pronunciations=["один эс"]),
                term(canonical="ERP", pronunciations=["ерп", "один эс"]),
            ]}
        )
        self.assertEqual(gl.hotword_phrases(), ["1С", "один эс", "ERP", "ерп"])

    def test_phrases_limit(self):
        gl = glossary.Glossary.from_dict(
            {"terms": [term(canonical="Т%d" % i) for i in range(80)]}
        )
        self.assertEqual(len(gl.hotword_phrases()), glossary.MAX_HOTWORD_PHRASES)

    def test_validate_too_many_phrases(self):
        with self.assertRaises(JobError) as ctx:
            glossary.validate_hotwords(["x"] * 65)
        self.assertEqual(ctx.exception.code, "bad_request")
        self.assertIn("too_many_hotwords", ctx.exception.message)

    def test_validate_phrase_too_long(self):
        with self.assertRaises(JobError) as ctx:
            glossary.validate_hotwords(["я" * 65])
        self.assertIn("hotword_phrase_too_long", ctx.exception.message)

    def test_write_hotwords_file_has_no_bom(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = glossary.write_hotwords_file(os.path.join(tmp, "hw.txt"), ["1С", "ERP"])
            with open(path, "rb") as f:
                raw = f.read()
        self.assertFalse(raw.startswith(b"\xef\xbb\xbf"))
        self.assertEqual(raw.decode("utf-8"), "1С\nERP\n")


if __name__ == "__main__":
    unittest.main()
