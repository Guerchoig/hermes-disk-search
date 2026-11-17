"""Sidecar-воркер анонимизации/деанонимизации документов (P1).

Транспорт: stdio + JSON-RPC 2.0, NDJSON, через общий пакет `hds_sidecar`
(PROTOCOL 1, изоляция stdout — как у воркера извлечения).

Методы: hello | warmup | inspect | anonymize | anonymize_values |
deanonymize | deanonymize_values | shutdown + progress-нотификации без `id`.

Параметры — ТОЛЬКО из аргументов командной строки (config.yaml воркер
не читает, §5.2 контракта sidecar):

    python worker.py --root <проект> --model knowledgator/gliner-pii-large-v1.0
        --threshold 0.3 --max-input-chars 1500 --chunk-overlap 200
        --timeout 300 --natasha 1 --models-dir <models>/anonymize
        --db data/anonymize.db --device cpu --gpu-mem-limit-mb 0
        [--hide-dates 0|1 --hide-numbers 0|1 --hide-software 0|1
         --hide-departments 0|1]

Флаги --hide-* включают скрытие опциональных категорий (даты; числа, не
являющиеся денежными суммами; программные продукты; подразделения).
По умолчанию все они ОТКРЫТЫ (0).

Offline-first: воркер не ходит в сеть — веса берутся из `--models-dir`
(`<models>/anonymize/hf-home`, `navec_*.tar`, `slovnet_ner_news_v1.tar`);
нет весов → структурированная ошибка `kind: model_missing` с hint про
установщик (`installers/fetch_anonymize_models.ps1`).
"""
from __future__ import annotations

import argparse
import asyncio
import os
import sys
import time
from pathlib import Path

_HERE = os.path.dirname(os.path.abspath(__file__))
_SIDE = os.path.dirname(_HERE)
if _SIDE not in sys.path:
    sys.path.insert(0, _SIDE)

from hds_sidecar import progress, runtime, stdio  # noqa: E402

PROTOCOL = stdio.PROTOCOL
ROOT = stdio.arg("--root") or os.path.dirname(_SIDE)

# Форматы P1: DOCX/XLSX/TXT/MD; P1b добавляет XML (MS Project) — слой
# парсера/сборки уже готов (file_parser.py, §1 реш. №7)
FORMATS = ["docx", "xlsx", "txt", "md", "xml"]

MODEL_MISSING_HINT = (
    "установите веса: powershell -File installers\\fetch_anonymize_models.ps1 "
    "-ModelsDir <models> (GLiNER PII ONNX + navec + slovnet)"
)


def _parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(prog="hds_anonymize.worker")
    p.add_argument("--root", default=None)
    p.add_argument("--model", default=None)
    p.add_argument("--threshold", type=float, default=None)
    p.add_argument("--max-input-chars", type=int, default=None)
    p.add_argument("--chunk-overlap", type=int, default=None)
    p.add_argument("--timeout", type=float, default=None)
    p.add_argument("--natasha", default="1")
    p.add_argument("--models-dir", default=None)
    p.add_argument("--db", default=None)
    p.add_argument(
        "--keep-terms",
        default=None,
        help="файл списка «не анонимизировать» (D19): YAML-список или строки",
    )
    p.add_argument(
        "--agent-images",
        default="strip",
        choices=["strip", "keep"],
        help="mode=agent: вырезать картинки тела в [IMAGE_n] (strip) или оставить",
    )
    p.add_argument("--device", default=None, choices=[None, "cpu", "cuda", "dml"])
    p.add_argument("--gpu-mem-limit-mb", type=int, default=None)
    p.add_argument("--idle-timeout", type=float, default=60.0)
    # Опциональные категории скрытия (по умолчанию все ОТКРЫТЫ)
    p.add_argument("--hide-dates", default="0")
    p.add_argument("--hide-numbers", default="0")
    p.add_argument("--hide-software", default="0")
    p.add_argument("--hide-departments", default="0")
    return p.parse_args()


def _flag(v: str) -> bool:
    """Строковый флаг 0|1|true|false|yes|no|on|off → bool."""
    return str(v).strip().lower() not in ("", "0", "false", "no", "off")


def _load_keep_terms(path: str | None) -> list[str]:
    """Список «не анонимизировать» из файла (YAML-список или строки).

    Формат `sidecar/hds_anonymize/keep_terms.example.yaml`: элементы списка
    (`- значение`), записи `re:<шаблон>` — регулярные выражения. Отдельной
    YAML-зависимости нет: разбор построчный и терпимый к комментариям.
    """
    if not path:
        return []
    source = Path(path)
    if not source.is_file():
        raise FileNotFoundError(f"нет файла keep_terms: {source}")
    terms: list[str] = []
    for line in source.read_text(encoding="utf-8-sig").splitlines():
        line = line.split("#", 1)[0].strip()
        if not line or (line.startswith("keep") and line.endswith(":")):
            continue
        if line.startswith("- "):
            line = line[2:].strip()
        if line[:1] in ("'", '"'):
            line = line.strip("'\"")
        if line:
            terms.append(line)
    return terms


def _setup_env_and_settings(args: argparse.Namespace) -> str:
    """Env offline-first + настройки движков из аргументов (до импортов)."""
    from hds_anonymize import settings

    models_dir = args.models_dir or str(settings.MODELS_FALLBACK_DIR)
    hf_home = os.path.join(models_dir, "hf-home")
    os.environ.setdefault("HF_HOME", hf_home)
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"

    settings.configure(
        model=args.model,
        threshold=args.threshold,
        max_input_chars=args.max_input_chars,
        chunk_overlap=args.chunk_overlap,
        timeout=args.timeout,
        natasha=str(args.natasha).strip().lower()
        not in ("0", "false", "no", "off"),
        models_dir=models_dir,
        device=args.device,
        gpu_mem_limit_mb=args.gpu_mem_limit_mb,
        hide_dates=_flag(args.hide_dates),
        hide_numbers=_flag(args.hide_numbers),
        hide_software=_flag(args.hide_software),
        hide_departments=_flag(args.hide_departments),
    )
    return models_dir


def _build_out_path(in_path: str, params: dict, mode: str):
    """Выходной путь: `out_path` из RPC, иначе out_dir + имя по схеме плана
    (§3): `<stem>.anonymized.<ext>` / `<stem>.deanon.<ext>`; у входа
    `a.anonymized.docx` суффикс `.anonymized` отбрасывается → `a.deanon.docx`.
    Path-safety: `..`-компоненты в out_dir/out_path запрещены (§3, kind
    out_of_dir; основная валидация — на стороне Rust-клиента)."""
    from pathlib import Path

    src = Path(in_path)
    if params.get("out_path"):
        out = Path(params["out_path"])
    else:
        out_dir = params.get("out_dir") or str(src.parent)
        stem = src.stem
        if stem.endswith(".anonymized"):
            stem = stem[: -len(".anonymized")]
        suffix = ".anonymized" if mode == "anonymize" else ".deanon"
        out = Path(out_dir) / f"{stem}{suffix}{src.suffix}"
    if ".." in out.parts:
        from hds_anonymize.handlers import OutOfDirError

        raise OutOfDirError(f"выходной путь вне разрешённого каталога: {out}")
    return out


def _precheck_weights(models_dir: str) -> None:
    """Дешёвая проверка весов ДО загрузки движков (offline-first, §2.1)."""
    from pathlib import Path

    from hds_anonymize import settings

    root = Path(models_dir)
    missing = []
    if settings.NER_ENGINE.get("natasha", True):
        for name in ("navec_news_v1_1B_250K_300d_100q.tar", "slovnet_ner_news_v1.tar"):
            if not (root / name).is_file():
                missing.append(name)
    hf_home = root / "hf-home"
    has_gliner = hf_home.is_dir() and any(hf_home.rglob("model.onnx"))
    if not has_gliner:
        missing.append("hf-home/**/model.onnx (GLiNER PII ONNX)")
    if missing:
        raise FileNotFoundError(
            "нет весов NER-моделей в --models-dir: " + ", ".join(missing)
        )


def _err(mid, exc: BaseException) -> None:
    """Структурированная ошибка {code, message, hint, kind} (§3 плана)."""
    from hds_anonymize.handlers import (
        FileBusyError,
        ModelMissingError,
        OutOfDirError,
        UnsupportedFormatError,
    )

    text = "%s: %s" % (type(exc).__name__, exc)
    hint = None
    kind = None
    if isinstance(exc, UnsupportedFormatError):
        kind = "unsupported"
    elif isinstance(exc, (FileBusyError, PermissionError)):
        kind = "file_busy"
        hint = "закройте файл в редакторе (Word/LibreOffice) и повторите"
    elif isinstance(exc, ImportError):
        kind = "model_missing"
        hint = MODEL_MISSING_HINT
    elif isinstance(exc, (ModelMissingError, FileNotFoundError)):
        # FileNotFoundError бывает РАЗНЫЙ: нет ВЕСОВ (→ model_missing) и
        # нет файла/каталога данных (→ обычная ошибка, kind не задаём).
        if "вес" in text or "модел" in text:
            kind = "model_missing"
            hint = MODEL_MISSING_HINT
    elif isinstance(exc, OutOfDirError):
        kind = "out_of_dir"
    elif isinstance(exc, LookupError):
        kind = "db_missing"
        hint = "БД токенов отсутствует/пуста; проверьте anonymize.db_path"
    stdio.err(mid, -32001, text, hint=hint, kind=kind)


class Worker:
    """Состояние воркера: хранилище, оркестратор, цикл asyncio."""

    def __init__(self, args: argparse.Namespace, models_dir: str) -> None:
        from hds_anonymize.handlers import AnonymizeHandlers
        from hds_anonymize.vault import Vault

        self.args = args
        self.models_dir = models_dir
        db_path = args.db or os.path.join(ROOT, "data", "anonymize.db")
        keep_terms = _load_keep_terms(getattr(args, "keep_terms", None))
        self.agent_images = str(
            getattr(args, "agent_images", "strip") or "strip"
        ).strip().lower()
        self.store = Vault(db_path, keep_terms=keep_terms)
        self.handlers = AnonymizeHandlers(self.store)
        self.loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.loop)

    # ── методы RPC ──────────────────────────────────────────────────────

    def _hello(self) -> dict:
        backend, providers = "unknown", []
        try:
            from hds_anonymize.gliner_engine import detect_onnx_providers

            providers = detect_onnx_providers()
            backend = "onnx" if providers else "unavailable"
        except Exception:  # noqa: BLE001 — hello не должен падать из-за ORT
            backend = "unavailable"
        from hds_anonymize import morph, settings

        return {
            "protocol": PROTOCOL,
            "python": sys.version.split()[0],
            "pid": os.getpid(),
            "backend": backend,
            "providers": providers,
            "model": settings.NER_ENGINE["model"],
            "formats": FORMATS,
            "capabilities": [
                "anonymize", "anonymize_values", "deanonymize",
                "deanonymize_values", "inspect", "warmup",
                # v2 (TR-1.6/1.7, TR-3.5, TR-7.4, D18)
                "anonymize_text", "text_image", "label_for_value",
                "vault_snapshot", "vault_rev", "vault_drop_project",
                "merge_entities", "keep_terms",
                "office_read", "office_edit", "office_create",
                "mode_agent",
            ],
            "schema_version": 2,
            "morph": morph.backend_info(),
            "keep_terms": self.store.stats()["keep_terms"],
        }

    def _warmup(self) -> dict:
        """Загрузить NER-движки; отсутствие весов/пакетов — исключение
        (в ответ уходит kind: model_missing)."""
        _precheck_weights(self.models_dir)
        t0 = time.time()

        async def _load():
            svc = self.handlers._ner_service()
            await svc.warmup()

        with progress.redirect_stdout():
            self.loop.run_until_complete(_load())
        ner = self.handlers._ner
        backend = getattr(ner._engine, "describe", lambda: "")()
        providers = list(getattr(ner._engine, "_providers", []) or [])
        return {"backend": backend, "providers": providers,
                "load_ms": round((time.time() - t0) * 1000, 1)}

    # ── файловые акты ───────────────────────────────────────────────────

    def _progress(self, file: str, phase: str, done: int, total: int) -> None:
        stdio.reply({
            "jsonrpc": "2.0",
            "method": "progress",
            "params": {"file": file, "phase": phase, "done": done, "total": total},
        })

    def _run_act(self, mid, method: str, params: dict) -> None:
        """Один RPC: файловые акты, текст, база меток, Office, образы."""
        t0 = time.time()
        project_id = (params.get("project_id") or "").strip()
        mode = str(params.get("mode") or "classic")
        in_path = params.get("in_path") or params.get("path") or ""

        def run(coro):
            return self.loop.run_until_complete(coro)

        with progress.redirect_stdout():
            if method in ("anonymize", "anonymize_values",
                          "deanonymize", "deanonymize_values", "inspect"):
                if not in_path:
                    raise ValueError("не задан in_path")
                if method in ("anonymize", "anonymize_values"):
                    values = params.get("values") or []
                    if method == "anonymize_values" and not values:
                        raise ValueError("anonymize_values: пустой список values")
                    out_path = _build_out_path(in_path, params, "anonymize")
                    res = run(self.handlers.anonymize(
                        in_path, str(out_path),
                        values=values or None,
                        progress=self._progress,
                        project_id=project_id or None,
                        mode=mode,
                        artifact_id=params.get("artifact_id"),
                        images_mode=str(params.get("images") or self.agent_images),
                    ))
                elif method in ("deanonymize", "deanonymize_values"):
                    out_path = _build_out_path(in_path, params, "deanonymize")
                    res = run(self.handlers.deanonymize(
                        in_path, str(out_path),
                        tokens=(params.get("tokens") or []) if method ==
                        "deanonymize_values" else None,
                        progress=self._progress,
                        project_id=project_id or None,
                    ))
                else:
                    res = run(self.handlers.inspect(in_path))
            elif method == "anonymize_text":
                text = params.get("text") or ""
                if not text:
                    raise ValueError("anonymize_text: пустой text")
                res = run(self.handlers.anonymize_text(
                    project_id, text, bool(params.get("ner", True))))
            elif method == "text_image":
                if not in_path:
                    raise ValueError("text_image: не задан in_path")
                res = run(self.handlers.text_image(
                    in_path,
                    out_path=params.get("out_path"),
                    out_dir=params.get("out_dir"),
                    project_id=project_id,
                    artifact_id=params.get("artifact_id"),
                    text=params.get("text"),
                    ner=bool(params.get("ner", True)),
                ))
            elif method == "label_for_value":
                if not params.get("value"):
                    raise ValueError("label_for_value: не задан value")
                res = self.handlers.label_for_value(
                    project_id,
                    str(params["value"]),
                    str(params.get("type") or params.get("value_type") or "MISC"),
                    bool(params.get("suffix", True)),
                )
            elif method == "vault_snapshot":
                if not project_id:
                    raise ValueError("vault_snapshot: не задан project_id")
                res = self.handlers.vault_snapshot(project_id)
            elif method == "vault_rev":
                res = self.handlers.vault_rev()
            elif method == "merge_entities":
                res = self.handlers.merge_entities(
                    project_id,
                    str(params.get("keep") or ""),
                    list(params.get("drop") or []),
                )
            elif method == "vault_drop_project":
                res = self.handlers.vault_drop_project(
                    project_id or str(params.get("project_id") or ""))
            elif method == "keep_terms":
                if (params.get("action") or "list") == "add":
                    res = self.handlers.keep_term_add(str(params.get("value") or ""))
                else:
                    res = self.handlers.keep_terms_list()
            elif method == "vault_stats":
                res = self.handlers.vault_stats()
            elif method == "office_read":
                res = self.handlers.office_read(in_path)
            elif method == "office_edit":
                res = self.handlers.office_edit(in_path, list(params.get("ops") or []))
            elif method == "office_create":
                res = self.handlers.office_create(
                    in_path,
                    str(params.get("kind") or ""),
                    dict(params.get("content") or {}),
                )
            else:  # pragma: no cover — dispatch проверяет заранее
                raise ValueError(method)
        res["elapsed_ms"] = round((time.time() - t0) * 1000, 1)
        stdio.ok(mid, res)

    def _handle(self, req: dict) -> bool:
        mid = req.get("id")
        method = req.get("method")
        params = req.get("params") or {}

        if method == "hello":
            stdio.ok(mid, self._hello())
            return False
        if method == "warmup":
            try:
                stdio.ok(mid, self._warmup())
            except Exception as e:  # noqa: BLE001 — warmup не роняет воркер
                _err(mid, e)
            return False
        if method in ("anonymize", "anonymize_values",
                      "deanonymize", "deanonymize_values", "inspect",
                      "anonymize_text", "text_image", "label_for_value",
                      "vault_snapshot", "vault_rev", "merge_entities",
                      "vault_drop_project", "keep_terms", "vault_stats",
                      "office_read", "office_edit", "office_create"):
            try:
                self._run_act(mid, method, params)
            except Exception as e:  # noqa: BLE001 — акт не должен ронять воркер
                _err(mid, e)
            return False
        if method == "shutdown":
            try:
                self.loop.run_until_complete(self._close_engines())
            finally:
                self.store.close()
            stdio.ok(mid, {"ok": True})
            return True
        stdio.err(mid, runtime.CODE_UNKNOWN_METHOD, "unknown method: %s" % method)
        return False

    async def _close_engines(self) -> None:
        ner = getattr(self.handlers, "_ner", None)
        if ner is not None:
            try:
                await ner.close()
            except Exception:  # noqa: BLE001
                pass


def main() -> int:
    # Изоляция протокола — ДО любых импортов, которые могут печатать в stdout.
    stdio.init()
    args = _parse_args()
    models_dir = _setup_env_and_settings(args)
    try:
        worker = Worker(args, models_dir)
    except Exception as e:  # noqa: BLE001 — БД недоступна: сообщаем и выходим
        _err(None, e)
        return 1
    return runtime.serve(worker._handle, idle_timeout=float(args.idle_timeout))


if __name__ == "__main__":
    sys.exit(main())
