#!/usr/bin/env python3
"""Mock-воркер hds_anonymize (stdlib-only) — как mock_worker.py в hds-extract:
изолирует транспорт/жизненный цикл Rust-клиента от реального Python-окружения
(NER-моделей). Шлёт progress-нотификации без `id` (клиент обязан их пропускать)
и структурированные kind-ошибки."""
import json
import os
import re
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

# Состояние «базы меток» mock-воркера: rev растёт при выдаче новой метки, а
# снимок включает выданные значения — так проверяется обновление фильтра по
# `vault_rev` (TR-6.5).
STATE = {"rev": 7, "extra": []}


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
                             "deanonymize_values", "inspect", "warmup",
                             "anonymize_text", "text_image", "label_for_value",
                             "anonymize_path",
                             "read_marker", "deanonymize_path", "image_map",
                             "vault_snapshot", "vault_rev", "vault_drop_project",
                             "merge_entities", "keep_terms",
                             "office_read", "office_edit", "office_create",
                             "mode_agent"],
            "schema_version": 2,
            "morph": "pymorphy2: ok; natasha: ok",
            "keep_terms": 3,
        })
        return False

    if method == "warmup":
        stdio.ok(mid, {"backend": "mock onnx", "providers": ["MockExecutionProvider"],
                       "load_ms": 1.0})
        return False

    if method in ("anonymize", "anonymize_values"):
        _progress(os.path.basename(params.get("in_path", "")))
        agent = params.get("mode") == "agent"
        if method == "anonymize" and not agent:
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
        if agent and not values:
            # в режиме agent mock изображает NER: метка проекта + падежный суффикс
            values = ["Иванов Иван", "Иванову Ивану"]
            matched = list(values)
        tokens = []
        anon = text
        counters = {}  # per-type счётчики — как в vault (счётчик проекта)
        for v in values:
            if v not in text:
                continue
            etype = "PERSON" if v[0].isalpha() else "MISC"
            if agent:
                # одна сущность — одна метка проекта, меняется только суффикс
                n = counters.setdefault(etype, 1)
            else:
                counters[etype] = counters.get(etype, 0) + 1
                n = counters[etype]
            suffix = "|дат" if agent and v.endswith("у") else ""
            token = "[%s_%d%s]" % (etype, n, suffix)
            count = text.count(v)
            anon = anon.replace(v, token)
            tokens.append({"token": token, "type": etype, "count": count})
        if agent:
            # Реальный воркер пишет маркер в свойства (docx/xlsx) или первой
            # строкой (`*.md`): обратный путь узнаёт свой образ по нему.
            marker = "hds-agent/" + (params.get("project_id") or "")
            if params.get("artifact_id"):
                marker = marker + "/" + str(params["artifact_id"])
            anon = "<!-- %s -->\n%s" % (marker, anon)
        with open(out_path, "w", encoding="utf-8", newline="") as f:
            f.write(anon)
        stdio.ok(mid, {
            "out_path": out_path, "tokens": tokens,
            "matched": matched, "unmatched": unmatched,
            "warnings": [], "elapsed_ms": 1.0,
            "mode": "agent" if agent else "classic",
            "images": 1 if agent else 0,
            "project_id": params.get("project_id"),
        })
        return False

    if method == "anonymize_text":
        project = params.get("project_id") or ""
        text = params.get("text") or ""
        anon = text.replace("Иванов Иван", "[PERSON_1]")
        stdio.ok(mid, {
            "text": anon,
            "labels": [{"label": "[PERSON_1]", "type": "PERSON",
                        "count": text.count("Иванов Иван")}],
            "project_id": project,
            "warnings": [],
        })
        return False

    if method == "anonymize_path":
        project = params.get("project_id") or ""
        raw = str(params.get("path") or "")
        if params.get("suffix"):
            stdio.err(mid, -32602, "suffix=true недопустим в именах файлов",
                      kind="internal")
            return False
        if not raw:
            stdio.err(mid, -32602, "anonymize_path: пустой path", kind="internal")
            return False
        parts = [p for p in re.split(r"[\\/]+", raw) if p not in ("", ".")]
        out = []
        for index, part in enumerate(parts):
            last = index == len(parts) - 1
            stem, ext = os.path.splitext(part) if last else (part, "")
            anon = (stem.replace("Иванов Иван", "[PERSON_1]")
                        .replace("Петров Пётр", "[PERSON_2]"))
            anon = re.sub(r'[<>:"/\\|?*]', "", anon).strip() or "_"
            out.append(anon + ext)
        sep = "\\" if "\\" in raw else "/"
        stdio.ok(mid, {
            "path": sep.join(out),
            "components": out,
            "labels": [{"label": "[PERSON_1]", "type": "PERSON",
                        "count": 1}],
            "project_id": project,
        })
        return False

    if method == "deanonymize_path":
        project = params.get("project_id") or ""
        raw = str(params.get("path") or "")
        if not project:
            stdio.err(mid, -32602, "deanonymize_path: не задан project_id",
                      kind="internal")
            return False
        if not raw:
            stdio.err(mid, -32602, "deanonymize_path: пустой path", kind="internal")
            return False
        parts = [p for p in re.split(r"[\\/]+", raw) if p]
        out = []
        for part in parts:
            value = part
            for token, real in MOCK_MAPPINGS.items():
                value = value.replace(token, real)
            out.append(value)
        sep = "\\" if "\\" in raw else "/"
        stdio.ok(mid, {
            "path": sep.join(out),
            "components": out,
            "unresolved": [],
            "project_id": project,
        })
        return False

    if method == "read_marker":
        path = params.get("path") or params.get("in_path") or ""
        marker = None
        project = artifact = None
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as f:
                first = (f.readline() or "").strip()
        except OSError:
            first = ""
        if first.startswith("<!-- hds-agent/") and first.endswith("-->"):
            marker = first[len("<!-- "):-len(" -->")]
            parts = marker.split("/")
            project = parts[1] if len(parts) > 1 else None
            artifact = parts[2] if len(parts) > 2 else None
        stdio.ok(mid, {
            "marker": marker,
            "project_id": project,
            "artifact_id": artifact,
        })
        return False

    if method == "image_map":
        stdio.ok(mid, {"images": []})
        return False

    if method == "vault_snapshot":
        labels = [{
            "label": "[PERSON_1]", "type": "PERSON",
            "forms": [
                {"value": "Иванов Иван", "gram": "nomn", "origin": "seen"},
                {"value": "Иванову Ивану", "gram": "datv", "origin": "seen"},
            ],
        }]
        labels.extend(STATE["extra"])
        stdio.ok(mid, {
            "rev": STATE["rev"],
            "labels": labels,
            "others": ["Петров Пётр"],
            "keep": ["ТЗ", "1С:ERP"],
        })
        return False

    if method == "vault_rev":
        stdio.ok(mid, {"rev": STATE["rev"]})
        return False

    if method == "label_for_value":
        value = str(params.get("value") or "")
        etype = str(params.get("type") or "OTHER")
        label = "[PERSON_2]"
        if value and not any(
            form["value"] == value
            for item in STATE["extra"]
            for form in item.get("forms", [])
        ):
            STATE["extra"].append({
                "label": label,
                "type": etype,
                "forms": [{"value": value, "gram": "nomn", "origin": "seen"}],
            })
            STATE["rev"] += 1
        stdio.ok(mid, {
            "project_id": params.get("project_id"),
            "value_type": etype,
            "label": label,
            "kept": False,
            "confident": True,
        })
        return False

    if method == "merge_entities":
        stdio.ok(mid, {"keep": params.get("keep"),
                       "dropped": params.get("drop") or [],
                       "forms_moved": 2})
        return False

    if method == "vault_drop_project":
        stdio.ok(mid, {"project_id": params.get("project_id"),
                       "labels": 1, "counters": 1, "aliases": 0})
        return False

    if method == "keep_terms":
        if params.get("action") == "add":
            stdio.ok(mid, {"value": params.get("value"), "added": True})
        else:
            stdio.ok(mid, {"keep": ["ТЗ", "1С:ERP", "re:REQ-\\d+"]})
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
        # Служебный маркер образа наружу не отдаём (как реальный воркер).
        de = re.sub(r"^<!--\s*hds-agent/[^\n>]*-->\r?\n?", "", de)
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

    if method == "text_image":
        in_path = params.get("in_path", "")
        out_dir = params.get("out_dir", "")
        project = params.get("project_id", "")
        artifact = params.get("artifact_id")
        text = params.get("text")
        if text is None:
            text = "Иванов Иван — текст извлечения"
        if params.get("out_path"):
            out_path = params["out_path"]
        else:
            out_path = os.path.join(out_dir, os.path.basename(in_path) + ".md")
        marker = "<!-- hds-agent/%s/%s -->" % (project, artifact or "")
        marker = marker.replace("/ -->", " -->")
        with open(out_path, "w", encoding="utf-8", newline="") as f:
            f.write(marker + "\n# " + os.path.basename(in_path) + "\n\n"
                    + text.replace("Иванов Иван", "[PERSON_1]") + "\n")
        stdio.ok(mid, {
            "out_path": out_path,
            "labels": [{"label": "[PERSON_1]", "type": "PERSON", "count": 1}],
            "project_id": project,
            "artifact_id": artifact,
            "marker": marker,
            "warnings": ["образ только для чтения"],
        })
        return False

    if method in ("office_read", "office_edit", "office_create"):
        stdio.ok(mid, {
            "path": params.get("path", ""),
            "kind": "docx",
            "markdown": "[1] mock",
            "applied": [
                {"op": op.get("op")} for op in (params.get("ops") or [])
            ],
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
