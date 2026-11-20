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

# Состояние «базы меток» процесса (A5.5): метки проекта, псевдонимы, склейки.
# Процесс у каждого клиента свой — посев зеркала и UI друг другу не мешают.
LABELS = {
    "[PERSON_1]": {
        "type": "PERSON",
        "forms": [
            {"value": "Иванов Иван", "gram": "nomn", "origin": "seen"},
            {"value": "Иванову Ивану", "gram": "datv", "origin": "seen"},
        ],
    },
    "[PERSON_2]": {
        "type": "PERSON",
        "forms": [{"value": "Петров Пётр", "gram": "nomn", "origin": "seen"}],
    },
}
ALIASES = {}   # младшая метка → оставшаяся (после склейки)
MERGES = []    # [{id, ts, keep, dropped, forms_moved, before, items}]
STATE = {"rev": 3}


def anonymize_bytes(data):
    text = data.decode("utf-8", "surrogateescape")
    for value, label in FORMS:
        text = text.replace(value, label)
    if "Петров" in text:
        text = text.replace("Петров", "[PERSON_2]")
    # после склейки NER отдаёт старшую метку — как настоящий воркер
    for dropped, keep in ALIASES.items():
        text = text.replace(dropped, keep)
    return text.encode("utf-8", "surrogateescape")


def project_labels():
    labels = []
    for label, item in LABELS.items():
        aliases = sorted(a for a, keep in ALIASES.items() if keep == label)
        labels.append({
            "label": label,
            "type": item["type"],
            "forms": [dict(form) for form in item["forms"]],
            "aliases": aliases,
        })
    merges = [
        {"id": m["id"], "ts": m["ts"], "keep": m["keep"],
         "dropped": list(m["dropped"]), "forms_moved": m["forms_moved"]}
        for m in MERGES
    ]
    return {"rev": STATE["rev"], "labels": labels, "merges": merges, "keep": KEEP}


def merge_labels(keep, drop):
    if keep not in LABELS:
        return None
    state = LABELS[keep]
    before = [form["value"] for form in state["forms"]]
    moved = 0
    items = []
    for token in drop:
        if token not in LABELS or token == keep:
            continue
        item = LABELS[token]
        for form in item["forms"]:
            if form["value"] not in [f["value"] for f in state["forms"]]:
                state["forms"].append(dict(form))
                moved += 1
        items.append({"label": token, "type": item["type"],
                      "forms": [dict(form) for form in item["forms"]]})
        del LABELS[token]
        ALIASES[token] = keep
    if not items:
        return None
    STATE["rev"] += 1
    record = {
        "id": len(MERGES) + 1,
        "ts": 1700000000.0 + len(MERGES),
        "keep": keep,
        "dropped": [item["label"] for item in items],
        "forms_moved": moved,
        "before": before,
        "items": items,
    }
    MERGES.append(record)
    return {"keep": keep, "dropped": record["dropped"],
            "forms_moved": moved, "merge_id": record["id"]}


def unmerge_labels(merge_id):
    record = next((m for m in MERGES if m["id"] == int(merge_id or 0)), None)
    if record is None:
        return None
    keep = record["keep"]
    before = set(record["before"])
    restored = []
    removed = 0
    for item in record["items"]:
        token = item["label"]
        LABELS[token] = {"type": item["type"],
                         "forms": [dict(form) for form in item["forms"]]}
        ALIASES.pop(token, None)
        for form in item["forms"]:
            if form["value"] in before:
                continue
            current = LABELS[keep]["forms"]
            LABELS[keep]["forms"] = [
                f for f in current if f["value"] != form["value"]
            ]
            removed += 1
        restored.append(token)
    MERGES.remove(record)
    STATE["rev"] += 1
    return {"merge_id": record["id"], "keep": keep,
            "restored": restored, "forms_removed": removed}



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
                             "vault_project_labels", "merge_entities",
                             "unmerge_entities", "keep_terms",
                             "mode_agent", "image_map"],
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
            value = params.get("value")
            added = bool(value) and value not in KEEP
            if added:
                KEEP.append(value)
            stdio.ok(mid, {"value": value, "added": added})
        else:
            stdio.ok(mid, {"keep": KEEP})
        return False
    if method == "vault_project_labels":
        stdio.ok(mid, project_labels())
        return False
    if method == "merge_entities":
        result = merge_labels(str(params.get("keep") or ""),
                              [str(t) for t in (params.get("drop") or [])])
        if result is None:
            stdio.err(mid, -32001, "склейка не выполнена", kind="internal")
            return False
        stdio.ok(mid, result)
        return False
    if method == "unmerge_entities":
        result = unmerge_labels(params.get("merge_id"))
        if result is None:
            stdio.err(mid, -32001, "склейка не найдена", kind="internal")
            return False
        stdio.ok(mid, result)
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
