#!/usr/bin/env python3
"""Mock-воркер hds_anonymize (stdlib-only) — как mock_worker.py в hds-extract:
изолирует транспорт/жизненный цикл Rust-клиента от реального Python-окружения
(NER-моделей). Шлёт progress-нотификации без `id` (клиент обязан их пропускать)
и структурированные kind-ошибки."""
import json
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(_HERE)))
_SIDECAR = os.path.join(_ROOT, "sidecar")
if _SIDECAR not in sys.path:
    sys.path.insert(0, _SIDECAR)

from hds_sidecar import runtime, stdio  # noqa: E402

# Каноничные маппинги mock-хранилища (деанонимизация в тестах)
MOCK_MAPPINGS = {
    "[PERSON_1]": "Иванов Иван",
    "[PERSON_2]": "Петров Пётр",
    "[MISC_1]": "+7 912 345-67-89",
    "[MISC_2]": "51 903,14",
}
TOKEN_TYPES = {tok: tok.strip("[]").rsplit("_", 1)[0] for tok in MOCK_MAPPINGS}


def _progress(name: str) -> None:
    for i, phase in enumerate(("parse", "ner", "replace", "assemble"), 1):
        stdio.reply({
            "jsonrpc": "2.0",
            "method": "progress",
            "params": {"file": name, "phase": phase, "done": i, "total": 4},
        })


def _handle(req):
    mid = req.get("id")
    method = req.get("method")
    params = req.get("params") or {}

    if method == "hello":
        stdio.ok(mid, {
            "protocol": 1,
            "python": "mock",
            "pid": os.getpid(),
            "backend": "mock",
            "providers": ["MockExecutionProvider"],
            "model": "mock/gliner-pii",
            "formats": ["docx", "xlsx", "txt", "md", "xml"],
            "capabilities": ["anonymize", "anonymize_values", "deanonymize",
                             "deanonymize_values", "inspect", "warmup"],
        })
        return False

    if method == "warmup":
        stdio.ok(mid, {"backend": "mock onnx", "providers": ["MockExecutionProvider"],
                       "load_ms": 1.0})
        return False

    if method in ("anonymize", "anonymize_values"):
        _progress(os.path.basename(params.get("in_path", "")))
        if method == "anonymize":
            # полный NER без весов в mock-мире невозможен — как в реальном воркере
            stdio.err(mid, -32001, "NER-модель не загрузилась",
                      hint="установите веса", kind="model_missing")
            return False
        in_path = params.get("in_path", "")
        out_path = params.get("out_path", "")
        values = params.get("values") or []
        with open(in_path, "r", encoding="utf-8", newline="") as f:
            text = f.read()
        matched = [v for v in values if v in text]
        unmatched = [v for v in values if v not in text]
        tokens = []
        anon = text
        counters = {}  # per-type счётчики — как в mapping_store (решение §2.1)
        for v in values:
            if v not in text:
                continue
            etype = "PERSON" if v[0].isalpha() else "MISC"
            counters[etype] = counters.get(etype, 0) + 1
            token = "[%s_%d]" % (etype, counters[etype])
            count = text.count(v)
            anon = anon.replace(v, token)
            tokens.append({"token": token, "type": etype, "count": count})
        with open(out_path, "w", encoding="utf-8", newline="") as f:
            f.write(anon)
        stdio.ok(mid, {
            "out_path": out_path, "tokens": tokens,
            "matched": matched, "unmatched": unmatched,
            "warnings": [], "elapsed_ms": 1.0,
        })
        return False

    if method in ("deanonymize", "deanonymize_values"):
        _progress(os.path.basename(params.get("in_path", "")))
        in_path = params.get("in_path", "")
        out_path = params.get("out_path", "")
        if method == "deanonymize_values" and not params.get("tokens"):
            stdio.err(mid, -32001, "пустой список tokens", kind="internal")
            return False
        allowed = (
            MOCK_MAPPINGS if method == "deanonymize"
            else {t: MOCK_MAPPINGS[t] for t in params.get("tokens", []) if t in MOCK_MAPPINGS}
        )
        with open(in_path, "r", encoding="utf-8", newline="") as f:
            text = f.read()
        restored = []
        de = text
        for token, value in allowed.items():
            if token in text:
                de = de.replace(token, value)
                restored.append(token)
        unmatched = [
            t for t in (params.get("tokens") or []) if t not in allowed
        ] if method == "deanonymize_values" else []
        with open(out_path, "w", encoding="utf-8", newline="") as f:
            f.write(de)
        stdio.ok(mid, {
            "out_path": out_path, "restored": restored, "unmatched": unmatched,
            "warnings": [], "elapsed_ms": 1.0,
        })
        return False

    if method == "inspect":
        in_path = params.get("in_path", "")
        with open(in_path, "r", encoding="utf-8", newline="") as f:
            text = f.read()
        counts = {}
        for token in MOCK_MAPPINGS:
            n = text.count(token)
            if n:
                counts[token] = n
        stdio.ok(mid, {
            "supported": True,
            "marker_version": None,
            "placeholders": [
                {"token": t, "type": TOKEN_TYPES[t], "count": c}
                for t, c in counts.items()
            ],
            "resolved": [{"token": t, "in_db": True} for t in counts],
        })
        return False

    if method == "shutdown":
        stdio.ok(mid, {"ok": True})
        return True

    stdio.err(mid, runtime.CODE_UNKNOWN_METHOD, "unknown method: %s" % method)
    return False


def main():
    stdio.init()
    return runtime.serve(_handle, idle_timeout=10.0)


if __name__ == "__main__":
    sys.exit(main())
