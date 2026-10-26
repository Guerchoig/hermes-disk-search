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
    p.add_argument("--device", default=None, choices=[None, "cpu", "cuda", "dml"])
    p.add_argument("--gpu-mem-limit-mb", type=int, default=None)
    p.add_argument("--idle-timeout", type=float, default=60.0)
    return p.parse_args()


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
    elif isinstance(exc, (ModelMissingError, FileNotFoundError, ImportError)):
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
        from hds_anonymize.mapping_store import MappingStore

        self.args = args
        self.models_dir = models_dir
        db_path = args.db or os.path.join(ROOT, "data", "anonymize.db")
        self.store = MappingStore(db_path)
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
        from hds_anonymize import settings

        return {
            "protocol": PROTOCOL,
            "python": sys.version.split()[0],
            "pid": os.getpid(),
            "backend": backend,
            "providers": providers,
            "model": settings.NER_ENGINE["model"],
            "formats": FORMATS,
            "capabilities": ["anonymize", "anonymize_values", "deanonymize",
                             "deanonymize_values", "inspect", "warmup"],
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
        in_path = params.get("in_path") or params.get("path") or ""
        if not in_path:
            raise ValueError("не задан in_path")
        out_path = _build_out_path(in_path, params, "anonymize")
        t0 = time.time()
        with progress.redirect_stdout():
            if method == "anonymize":
                res = self.loop.run_until_complete(self.handlers.anonymize(
                    in_path, str(out_path), progress=self._progress))
            elif method == "anonymize_values":
                values = params.get("values") or []
                if not values:
                    raise ValueError("anonymize_values: пустой список values")
                res = self.loop.run_until_complete(self.handlers.anonymize(
                    in_path, str(out_path), values=values, progress=self._progress))
            elif method == "deanonymize":
                out = _build_out_path(in_path, params, "deanonymize")
                res = self.loop.run_until_complete(self.handlers.deanonymize(
                    in_path, str(out), progress=self._progress))
            elif method == "deanonymize_values":
                out = _build_out_path(in_path, params, "deanonymize")
                res = self.loop.run_until_complete(self.handlers.deanonymize(
                    in_path, str(out), tokens=params.get("tokens") or [],
                    progress=self._progress))
            elif method == "inspect":
                res = self.loop.run_until_complete(self.handlers.inspect(in_path))
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
                      "deanonymize", "deanonymize_values", "inspect"):
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
