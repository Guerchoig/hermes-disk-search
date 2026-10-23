"""P4.7: LLM-патчи без настоящей модели — фейковый OpenAI-совместимый сервер.

Проверяется гейт P4: патчи не меняют спикера/таймкоды/alignment, отказы не
портят текст, `llm.enabled=false` = P3/P2, недоступный хост и мусор — деградация
без падения, повторный `postprocess` идемпотентен.
"""
from __future__ import annotations

import json
import os
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from hds_transcribe import glossary as glossary_mod
from hds_transcribe import llm_patch, worker
from hds_transcribe.params import PostprocessParams

LLM_ON = {
    "enabled": True,
    "base_url": "http://127.0.0.1:1/v1",  # заменяется на порт фейка
    "model": "fake-chat",
    "temperature": 0,
    "max_tokens": 512,
    "timeout_sec": 5,
    "max_edits_per_1000": 10,
}


def llm_params(**overrides) -> PostprocessParams:
    llm = dict(LLM_ON)
    llm.update(overrides)
    return PostprocessParams.from_dict({"llm": llm})


def word(index, text, speaker="SPEAKER_00", alignment="aligned"):
    return {
        "word_id": index,
        "text": text,
        "start": float(index),
        "end": float(index) + 0.5,
        "alignment": alignment,
        "speaker": speaker,
        "speakers": [speaker],
        "overlap": False,
    }


def make_block(words, speaker="SPEAKER_00"):
    return {
        "speaker": speaker,
        "start": words[0]["start"],
        "end": words[-1]["end"],
        "text": " ".join(w["text"] for w in words),
        "words": words,
        "overlap": False,
    }


def standard_blocks():
    """Блок с подозрительным «ерпи» (low_confidence) и контекстом «систему»."""
    words = [
        word(0, "про"),
        word(1, "бухгалтерию"),
        word(2, "ерпи", alignment="low_confidence"),
        word(3, "систему"),
        word(4, "не"),
        word(5, "закрыли"),
        word(6, "25"),
    ]
    return [make_block(words)]


def glossary_erp():
    return glossary_mod.Glossary.from_dict({"terms": [{
        "canonical": "ERP",
        "variants": ["ерпи"],
        "context": ["систему"],
        "priority": "normal",
    }]})


def edit(word_ids, replacement, **extra):
    payload = {"word_ids": word_ids, "replacement": replacement, "reason": "asr"}
    payload.update(extra)
    return payload


def content(edits):
    return json.dumps({"edits": edits}, ensure_ascii=False)


class FakeLlm:
    """HTTP-сервер `/v1/chat/completions`: ответ, мусор, ошибка, задержка."""

    def __init__(self, payload=None, *, raw=None, status=200, delay=0.0):
        self.payload = payload
        self.raw = raw
        self.status = status
        self.delay = delay
        self.requests = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):  # noqa: N802 - имя задаёт http.server
                try:
                    self._respond()
                except OSError:
                    # Клиент закрыл соединение по таймауту — это и проверяет тест.
                    pass

            def _respond(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length).decode("utf-8", "replace")
                outer.requests.append(body)
                if outer.delay:
                    time.sleep(outer.delay)
                if outer.status != 200:
                    self.send_response(outer.status)
                    self.end_headers()
                    self.wfile.write(b'{"error": "boom"}')
                    return
                text = outer.raw
                if text is None:
                    # payload — уже готовый JSON-текст правок (строка) либо объект.
                    inner = outer.payload
                    if not isinstance(inner, str):
                        inner = json.dumps(inner, ensure_ascii=False)
                    text = json.dumps(
                        {"choices": [{"message": {"content": inner}}]},
                        ensure_ascii=False,
                    )
                data = text.encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):  # тишина в stderr тестов
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def url(self) -> str:
        host, port = self.server.server_address[:2]
        return "http://%s:%s/v1" % (host, port)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


class LlmPatchTest(unittest.TestCase):
    def run_layer(self, payload, blocks=None, glossary=None, **llm):
        with FakeLlm(payload) as fake:
            params = llm_params(base_url=fake.url, **llm)
            result = llm_patch.run(blocks or standard_blocks(), params,
                                   glossary if glossary is not None else glossary_erp())
        return result, fake.requests

    # --------------------------------------------------------- принятие
    def test_valid_glossary_patch_is_applied(self):
        blocks = standard_blocks()
        with FakeLlm(content([edit([2], "ERP")])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary_erp())
        self.assertEqual(len(fake.requests), 1)
        self.assertEqual(result["stats"]["applied"], 1)
        self.assertEqual(result["stats"]["rejected"], 0)
        self.assertEqual(result["summary"]["model"], "fake-chat")
        rule = result["edits"][0]
        self.assertEqual(rule["source"], "llm")
        self.assertEqual(rule["status"], "applied")
        self.assertEqual(rule["word_ids"], [2])
        self.assertGreaterEqual(rule["latency_ms"], 0.0)
        self.assertEqual(blocks[0]["words"][2]["text"], "ERP")
        self.assertEqual(blocks[0]["text"], "про бухгалтерию ERP систему не закрыли 25")
        # структура не изменилась
        self.assertEqual((blocks[0]["words"][2]["start"],
                          blocks[0]["words"][2]["end"]), (2.0, 2.5))
        self.assertEqual(blocks[0]["words"][2]["alignment"], "low_confidence")

    def test_allow_global_patch_applies_without_glossary_context(self):
        blocks = standard_blocks()
        with FakeLlm(content([edit([2], "ЕРП", allow_global=True)])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url),
                                   glossary_mod.Glossary())
        self.assertEqual(result["stats"]["applied"], 1)
        self.assertEqual(blocks[0]["words"][2]["text"], "ЕРП")

    def test_needs_review_patch_does_not_change_text(self):
        blocks = standard_blocks()
        with FakeLlm(content([edit([2], "ERP", needs_review=True)])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary_erp())
        self.assertEqual(result["stats"]["needs_review"], 1)
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(blocks[0]["words"][2]["text"], "ерпи")
        self.assertEqual(result["edits"][0]["status"], "needs_review")

    # --------------------------------------------------------- отказы
    def test_speaker_change_is_rejected(self):
        result, _ = self.run_layer(content([edit([2], "ERP", speaker="SPEAKER_01")]))
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(result["edits"][0]["reason"], "metadata_change")

    def test_timestamp_change_is_rejected(self):
        result, _ = self.run_layer(content([edit([2], "ERP", start=99.0)]))
        self.assertEqual(result["edits"][0]["reason"], "metadata_change")

    def test_numbers_are_not_changed(self):
        result, _ = self.run_layer(content([edit([6], "26", allow_global=True)]))
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(result["edits"][0]["reason"], "numbers_changed")

    def test_negation_is_not_lost(self):
        result, _ = self.run_layer(content([edit([4, 5], "закрыли", allow_global=True)]))
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(result["edits"][0]["reason"], "negation_lost")

    def test_too_long_replacement_is_rejected(self):
        result, _ = self.run_layer(content([edit([2], "ERP" * 40, allow_global=True)]))
        self.assertEqual(result["edits"][0]["reason"], "too_long")


    def test_edit_limit_is_enforced(self):
        blocks = standard_blocks()
        # allowance = ceil(10 * 7 / 1000) = 1 → вторая правка отклоняется.
        with FakeLlm(content([
            edit([2], "ERP", allow_global=True),
            edit([1], "бухгалтерия", allow_global=True),
        ])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url),
                                   glossary_mod.Glossary())
        self.assertEqual(result["stats"]["applied"], 1)
        self.assertEqual(result["stats"]["rejected"], 1)
        self.assertIn("edit_limit", [e["reason"] for e in result["edits"]])

    def test_unknown_and_cross_block_ids_are_rejected(self):
        blocks = standard_blocks() + [make_block(
            [word(7, "второй", speaker="SPEAKER_01")], speaker="SPEAKER_01")]
        with FakeLlm(content([
            edit([999], "x", allow_global=True),
            edit([2, 7], "x", allow_global=True),
        ])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url),
                                   glossary_mod.Glossary())
        reasons = [e["reason"] for e in result["edits"]]
        self.assertIn("unknown_word_ids", reasons)
        self.assertIn("cross_block", reasons)
        self.assertEqual(result["stats"]["applied"], 0)

    def test_non_contiguous_ids_are_rejected(self):
        result, _ = self.run_layer(content([edit([1, 3], "x", allow_global=True)]))
        self.assertEqual(result["edits"][0]["reason"], "non_contiguous_word_ids")

    def test_overlap_with_glossary_span_is_rejected(self):
        blocks = standard_blocks()
        with FakeLlm(content([edit([2], "ERP", allow_global=True)])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url),
                                   glossary_erp(), applied_spans={0: [(2, 3)]})
        self.assertEqual(result["edits"][0]["reason"], "overlaps_applied")
        self.assertEqual(blocks[0]["words"][2]["text"], "ерпи")
        self.assertEqual(result["stats"]["applied"], 0)

    def test_no_context_patch_is_rejected(self):
        # Постороннее слово + канон в замене: фрагмент не похож на вариант
        # (0,0–0,4), контекста нет → отказ (решение C не ослабляет гейт).
        glossary = glossary_mod.Glossary.from_dict({"terms": [{
            "canonical": "ERP", "variants": ["ерпи"], "context": ["склад"],
        }]})
        result, _ = self.run_layer(content([edit([0], "ERP")]), glossary=glossary)
        self.assertEqual(result["edits"][0]["reason"], "no_glossary_context")

    def test_exact_variant_canonical_replacement_allowed_without_context(self):
        # Решение C: точный вариант глоссария (сходство 1,0) + канон в замене —
        # контекст не обязателен; глоссарийный слой такой случай отправлял в review.
        result, _ = self.run_layer(content([edit([2], "ERP")]))
        self.assertEqual(result["stats"]["applied"], 1, result["edits"])
        self.assertEqual(result["edits"][0]["term"], "ERP")

    def test_no_change_is_rejected(self):
        result, _ = self.run_layer(content([edit([2], "ерпи", allow_global=True)]))
        self.assertEqual(result["edits"][0]["reason"], "no_change")

    def test_canonical_inside_replacement_uses_glossary_context(self):
        # «RP-система» → «ERP-система»: замена не равна канону «ERP», но содержит
        # его; контекст «система» рядом — патч применяется без allow_global.
        words = [word(0, "про"), word(1, "RP-система,", alignment="low_confidence"),
                 word(2, "систему")]
        blocks = [make_block(words)]
        glossary = glossary_mod.Glossary.from_dict({"terms": [{
            "canonical": "ERP", "variants": ["ерпи"], "context": ["систему"],
        }]})
        with FakeLlm(content([edit([1], "ERP-система")])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary)
        self.assertEqual(result["stats"]["applied"], 1, result["edits"])
        self.assertEqual(blocks[0]["words"][1]["text"], "ERP-система,")

    def test_canonical_form_with_similar_variant_needs_no_context(self):
        # Решение C (23.10.2026): канон в замене + фрагмент похож на вариант
        # глоссария (RP-система ↔ rp система = 0,90) → контекст не обязателен.
        words = [word(0, "как"), word(1, "написано"),
                 word(2, "RP-система,", alignment="low_confidence")]
        blocks = [make_block(words)]
        glossary = glossary_mod.Glossary.from_dict({"terms": [{
            "canonical": "ERP", "variants": ["ерпи", "rp система"],
            "context": ["систему"],
        }]})
        with FakeLlm(content([edit([2], "ERP-система")])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary)
        self.assertEqual(result["stats"]["applied"], 1, result["edits"])
        self.assertEqual(blocks[0]["words"][2]["text"], "ERP-система,")
        self.assertEqual(blocks[0]["text"], "как написано ERP-система,")

    def test_canonical_form_with_dissimilar_source_is_rejected(self):
        # Тот же канон, но фрагмент не похож ни на один вариант (0,29) →
        # вставка канона в постороннее слово блокируется.
        words = [word(0, "Хорошо,"), word(1, "слайдик", alignment="low_confidence"),
                 word(2, "надо")]
        blocks = [make_block(words)]
        glossary = glossary_mod.Glossary.from_dict({"terms": [{
            "canonical": "ERP", "variants": ["ерпи", "rp система"],
            "context": ["систему"],
        }]})
        with FakeLlm(content([edit([1], "слайд ERP")])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary)
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(result["edits"][0]["reason"], "no_glossary_context")
        self.assertEqual(blocks[0]["words"][1]["text"], "слайдик")

    def test_punctuation_only_patch_is_no_change(self):
        words = [word(0, "в"), word(1, "1С,", alignment="low_confidence"), word(2, "отчёт")]
        blocks = [make_block(words)]
        with FakeLlm(content([edit([1], "1С", allow_global=True)])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url),
                                   glossary_mod.Glossary())
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(result["edits"][0]["reason"], "no_change")
        self.assertEqual(blocks[0]["text"], "в 1С, отчёт")


    # ------------------------------------------------- деградация и разбор
    def test_unreachable_host_degrades_without_failure(self):
        blocks = standard_blocks()
        result = llm_patch.run(blocks, llm_params(base_url="http://127.0.0.1:9/v1"),
                               glossary_erp())
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertTrue(any("LLM:" in w and "деградация" in w
                            for w in result["warnings"]), result["warnings"])
        self.assertEqual(blocks[0]["words"][2]["text"], "ерпи")
        self.assertTrue(result["summary"]["errors"])

    def test_timeout_degrades_without_failure(self):
        blocks = standard_blocks()
        with FakeLlm(content([edit([2], "ERP")]), delay=1.5) as fake:
            params = llm_params(base_url=fake.url, timeout_sec=0.2)
            started = time.perf_counter()
            result = llm_patch.run(blocks, params, glossary_erp())
        self.assertLess(time.perf_counter() - started, 1.4)
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(blocks[0]["words"][2]["text"], "ерпи")
        self.assertTrue(result["warnings"])

    def test_http_500_degrades(self):
        blocks = standard_blocks()
        with FakeLlm(status=500) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary_erp())
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertTrue(any("HTTP 500" in e for e in result["summary"]["errors"]))

    def test_garbage_response_is_rejected_without_changes(self):
        blocks = standard_blocks()
        raw = json.dumps({"choices": [{"message": {"content": "не json"}}]})
        with FakeLlm(raw=raw) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary_erp())
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(blocks[0]["words"][2]["text"], "ерпи")
        self.assertTrue(any("не удалось разобрать" in w for w in result["warnings"]))

    def test_markdown_fences_and_extra_fields_are_tolerated(self):
        raw = "```json\n" + json.dumps(
            {"edits": [edit([2], "ERP", extra="ignored")], "note": "ok"},
            ensure_ascii=False) + "\n```"
        blocks = standard_blocks()
        payload = json.dumps({"choices": [{"message": {"content": raw}}]})
        with FakeLlm(raw=payload) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url), glossary_erp())
        self.assertEqual(result["stats"]["applied"], 1)
        self.assertEqual(blocks[0]["words"][2]["text"], "ERP")

    def test_malformed_edit_is_journaled_as_rejected(self):
        result, _ = self.run_layer(content([{"word_ids": "2", "replacement": "ERP"},
                                            "мусор"]))
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertEqual(result["stats"]["rejected"], 2)

    def test_llm_off_makes_no_requests_and_no_edits(self):
        blocks = standard_blocks()
        with FakeLlm(content([edit([2], "ERP")])) as fake:
            params = llm_params(base_url=fake.url, enabled=False)
            result = llm_patch.run(blocks, params, glossary_erp())
        self.assertEqual(fake.requests, [])
        self.assertEqual(result["edits"], [])
        self.assertFalse(result["summary"]["enabled"])
        self.assertEqual(blocks[0]["words"][2]["text"], "ерпи")

    def test_no_candidates_makes_no_request(self):
        words = [word(0, "обычный"), word(1, "текст"), word(2, "без"), word(3, "флагов")]
        blocks = [make_block(words)]
        with FakeLlm(content([edit([0], "x")])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url),
                                   glossary_mod.Glossary())
        self.assertEqual(fake.requests, [])
        self.assertEqual(result["summary"]["windows"], 0)

    def test_zero_edit_limit_disables_patches(self):
        blocks = standard_blocks()
        with FakeLlm(content([edit([2], "ERP", allow_global=True)])) as fake:
            result = llm_patch.run(blocks, llm_params(base_url=fake.url,
                                                      max_edits_per_1000=0),
                                   glossary_erp())
        self.assertEqual(fake.requests, [])
        self.assertEqual(result["stats"]["applied"], 0)
        self.assertIn("edit_limit_zero", result["summary"]["errors"])

    def test_repeated_run_is_idempotent(self):
        with FakeLlm(content([edit([2], "ERP")])) as fake:
            params = llm_params(base_url=fake.url)
            first_blocks = standard_blocks()
            first = llm_patch.run(first_blocks, params, glossary_erp())
            second_blocks = standard_blocks()
            second = llm_patch.run(second_blocks, params, glossary_erp())

        def stable(edits):
            # latency_ms измеряется заново — идемпотентность про текст и решения.
            return [{k: v for k, v in rule.items() if k != "latency_ms"}
                    for rule in edits]

        self.assertEqual(first_blocks[0]["text"], second_blocks[0]["text"])
        self.assertEqual(stable(first["edits"]), stable(second["edits"]))



class PostprocessLlmTest(unittest.TestCase):
    """P4.3/P4.6: `worker.postprocess` с LLM-слоем и журналом `edits.json`."""

    def setUp(self):
        import tempfile  # локальный импорт: модуль тестов не тянет лишнего

        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.transcript = os.path.join(self.tmp.name, "meet.transcript.json")
        self.edits = os.path.join(self.tmp.name, "meet.edits.json")
        payload = {
            "job_id": "job-llm",
            "media": "D:/media/meet.mp3",
            "blocks": standard_blocks(),
            "alignment_quality": {"words": 7},
            "stats": {"rtf": 0.02},
        }
        with open(self.transcript, "w", encoding="utf-8") as f:
            json.dump(payload, f, ensure_ascii=False, indent=2)

    def run_postprocess(self, url, **llm):
        request = {"job_id": "job-llm", "transcript_path": self.transcript,
                   "llm": dict(LLM_ON, base_url=url, **llm)}
        return worker.postprocess(request)

    def read(self, path):
        with open(path, encoding="utf-8") as f:
            return json.load(f)

    def test_llm_patch_is_applied_and_journaled(self):
        with FakeLlm(content([edit([2], "ERP", allow_global=True)])) as fake:
            result = self.run_postprocess(fake.url)
        self.assertIn("ERP", result["text"])
        self.assertEqual(result["llm"]["applied"], 1)
        words = result["blocks"][0]["words"]
        # Гейт: спикер/таймкоды/alignment не тронуты.
        self.assertEqual((words[2]["start"], words[2]["end"]), (2.0, 2.5))
        self.assertEqual(words[2]["speaker"], "SPEAKER_00")
        self.assertEqual(words[2]["alignment"], "low_confidence")
        saved = self.read(self.edits)
        self.assertEqual(saved["edits"][0]["source"], "llm")
        self.assertEqual(saved["edits"][0]["status"], "applied")
        self.assertEqual(saved["edits"][0]["model"], "fake-chat")
        self.assertIn("latency_ms", saved["edits"][0])
        self.assertEqual(saved["stats"]["sources"], {"glossary": 0, "llm": 1})
        transcript = self.read(self.transcript)
        # Текст ASR в transcript.json не переписывается.
        self.assertEqual(transcript["blocks"][0]["words"][2]["text"], "ерпи")
        self.assertEqual(transcript["postprocess"]["llm"]["applied"], 1)
        self.assertEqual(transcript["postprocess"]["llm"]["host"], fake.url)

    def test_llm_off_with_empty_glossary_is_p2(self):
        with open(self.transcript, encoding="utf-8") as f:
            before = json.load(f)
        with FakeLlm(content([edit([2], "ERP", allow_global=True)])) as fake:
            result = self.run_postprocess(fake.url, enabled=False)
        self.assertEqual(fake.requests, [])
        self.assertFalse(os.path.exists(self.edits))
        self.assertEqual(result["text"], "про бухгалтерию ерпи систему не закрыли 25")
        saved = self.read(self.transcript)
        self.assertNotIn("postprocess", saved)
        self.assertEqual(saved["blocks"], before["blocks"])

    def test_llm_degradation_keeps_glossary_result(self):
        result = self.run_postprocess("http://127.0.0.1:9/v1")
        self.assertEqual(result["llm"]["applied"], 0)
        self.assertTrue(result["llm"]["errors"])
        self.assertTrue(any("деградация" in w for w in result["warnings"]),
                        result["warnings"])
        saved = self.read(self.transcript)
        self.assertIn("postprocess", saved)
        self.assertEqual(saved["blocks"][0]["words"][2]["text"], "ерпи")

    def test_llm_review_is_duplicated_into_transcript(self):
        with FakeLlm(content([edit([2], "ERP", needs_review=True)])) as fake:
            result = self.run_postprocess(fake.url)
        self.assertIn("ерпи", result["text"])
        saved = self.read(self.transcript)
        self.assertTrue(saved["blocks"][0]["words"][2].get("needs_review"))
        review = saved["postprocess"]["needs_review"]
        self.assertEqual(review[0]["source"], "llm")
        self.assertEqual(review[0]["word_ids"], [2])

    def test_glossary_layer_blocks_overlapping_llm_patch(self):
        request = {
            "job_id": "job-llm",
            "transcript_path": self.transcript,
            "glossary": {"terms": [{
                "canonical": "ERP", "variants": ["ерпи"], "context": ["систему"],
            }]},
        }
        with FakeLlm(content([edit([2], "ЕЭРПИ", allow_global=True)])) as fake:
            request["llm"] = dict(LLM_ON, base_url=fake.url)
            result = worker.postprocess(request)
        self.assertEqual(result["blocks"][0]["words"][2]["text"], "ERP")
        sources = {rule["source"]: rule for rule in result["edits"]}
        self.assertEqual(sources["glossary"]["status"], "applied")
        self.assertEqual(sources["llm"]["status"], "rejected")
        self.assertEqual(sources["llm"]["reason"], "overlaps_applied")

    def test_repeated_postprocess_is_idempotent(self):
        with FakeLlm(content([edit([2], "ERP", allow_global=True)])) as fake:
            first = self.run_postprocess(fake.url)
            second = self.run_postprocess(fake.url)
        self.assertEqual(first["text"], second["text"])
        self.assertEqual(first["llm"]["applied"], second["llm"]["applied"])
        saved = self.read(self.transcript)
        self.assertEqual(saved["postprocess"]["llm"]["applied"], 1)

    def test_no_llm_and_no_glossary_clears_previous_markup(self):
        with FakeLlm(content([edit([2], "ERP", allow_global=True)])) as fake:
            self.run_postprocess(fake.url)
        self.run_postprocess("http://127.0.0.1:9/v1", enabled=False)
        saved = self.read(self.transcript)
        self.assertNotIn("postprocess", saved)
        self.assertFalse(saved["blocks"][0]["words"][2].get("needs_review", False))


if __name__ == "__main__":
    unittest.main()


if __name__ == "__main__":
    unittest.main()

