"""Mock-воркер анонимизации для тестов вкладки «Агент» (A5.2…A5.4).

NDJSON через `hds_sidecar`; NER-модели и venv не нужны. Формы — те же, что в
фикстурах стенда: «Иванов Иван» ↔ `[PERSON_1]`, «1С:ERP» — `keep_terms`.
`__SIDECAR__` заменяется тестом на путь к `sidecar/` репозитория.
"""

import os
import sys

sys.path.insert(0, r"__SIDECAR__")
from hds_sidecar import runtime, stdio

FORMS = [("Иванову Ивану", "[PERSON_1|дат]"), ("Иванов Иван", "[PERSON_1]")]
KEEP = ["1С:ERP", "ТЗ"]

TEXT_EXT = (".md", ".txt", ".xml", ".json")


def anonymize_bytes(data):
    text = data.decode("utf-8", "surrogateescape")
    for value, label in FORMS:
        text = text.replace(value, label)
    if "Петров" in text:
        text = text.replace("Петров", "[PERSON_2]")
    return text.encode("utf-8", "surrogateescape")


def handle(req):
    mid = req.get("id")
    method = req.get("method")
    params = req.get("params") or {}
    if method == "hello":
        stdio.ok(mid, {
            "protocol": 1, "python": "mock", "pid": os.getpid(), "backend": "mock",
            "providers": ["MockExecutionProvider"], "model": "mock/gliner-pii",
            "formats": ["docx", "xlsx", "txt", "md", "xml"],
            "capabilities": ["anonymize", "anonymize_text", "anonymize_path",
                             "vault_snapshot", "vault_rev", "label_for_value",
                             "keep_terms", "mode_agent", "image_map"],
            "schema_version": 2, "morph": "mock", "keep_terms": len(KEEP),
        })
        return False
    if method == "vault_snapshot":
        stdio.ok(mid, {
            "rev": 3,
            "labels": [{
                "label": "[PERSON_1]", "type": "PERSON",
                "forms": [
                    {"value": "Иванов Иван", "gram": "nomn", "origin": "ner"},
                    {"value": "Иванову Ивану", "gram": "datv", "origin": "ner"},
                ],
            }],
            "others": ["Петров Пётр"],
            "keep": KEEP,
        })
        return False
    if method == "vault_rev":
        stdio.ok(mid, {"rev": 3})
        return False
    if method == "anonymize_text":
        text = params.get("text") or ""
        labels = []
        for value, label in FORMS:
            if value in text:
                text = text.replace(value, label)
                labels.append({"label": label, "type": "PERSON", "count": 1})
        if "Петров" in text:
            text = text.replace("Петров", "[PERSON_2]")
            labels.append({"label": "[PERSON_2]", "type": "PERSON", "count": 1})
        stdio.ok(mid, {"text": text, "labels": labels,
                       "project_id": params.get("project_id"), "warnings": []})
        return False
    if method == "anonymize_path":
        path = params.get("path") or ""
        for value, label in FORMS:
            path = path.replace(value, label)
        stdio.ok(mid, {"path": path})
        return False
    if method == "image_map":
        stdio.ok(mid, {"images": []})
        return False
    if method == "label_for_value":
        stdio.ok(mid, {"project_id": params.get("project_id"),
                       "value_type": params.get("type") or "OTHER",
                       "label": "[PERSON_2]", "kept": False, "confident": True})
        return False
    if method == "keep_terms":
        if params.get("action") == "add":
            stdio.ok(mid, {"value": params.get("value"), "added": True})
        else:
            stdio.ok(mid, {"keep": KEEP})
        return False
    if method == "anonymize":
        in_path = params.get("in_path")
        out_path = params.get("out_path")
        with open(in_path, "rb") as source:
            data = source.read()
        # docx/xlsx — бинарные: текст не меняем, важно, что образ записан.
        if not str(in_path).lower().endswith(TEXT_EXT):
            data = b"<!-- hds-agent/mock -->\n" + data
        else:
            data = anonymize_bytes(data)
        with open(out_path, "wb") as target:
            target.write(data)
        stdio.ok(mid, {"out_path": out_path, "labels": [], "warnings": [],
                       "project_id": params.get("project_id")})
        return False
    if method == "deanonymize_agent":
        in_path = params.get("in_path")
        out_path = params.get("out_path")
        with open(in_path, "rb") as source:
            data = source.read()
        text = data.decode("utf-8", "surrogateescape")
        for value, label in FORMS:
            text = text.replace(label, value)
        with open(out_path, "wb") as target:
            target.write(text.encode("utf-8", "surrogateescape"))
        stdio.ok(mid, {"out_path": out_path, "labels": [], "warnings": [],
                       "project_id": params.get("project_id")})
        return False
    if method == "shutdown":
        stdio.ok(mid, {"ok": True})
        return True
    stdio.err(mid, -32601, "unknown method: %s" % method)
    return False


stdio.init()
sys.exit(runtime.serve(handle, idle_timeout=30.0))
