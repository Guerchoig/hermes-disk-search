"""Автономный sidecar-воркер извлечения/нормализации (B6, контракт §5 плана).

Транспорт: stdio + JSON-RPC 2.0, кадрирование NDJSON (один JSON-объект на строку).
Методы: hello | extract | normalize | clip_image | shutdown.

После P1 транспорт, конфиг и изоляция вывода живут в общем пакете
`sidecar/hds_sidecar` (PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §5.1); поведение
воркера не изменилось: те же методы, `hello.protocol=1`, тот же idle-timeout,
EOF, структурированные ошибки.

Запуск: python worker.py --root <путь проекта>
"""
import copy
import os
import sys
import time

_HERE = os.path.dirname(os.path.abspath(__file__))
# Поставка: рядом с `hds_extract` лежит самодостаточная копия модулей извлечения
# под `sidecar/hds` (собирает installers/build_sidecar.ps1). Кладём каталог sidecar
# в path ПЕРЕД корнем проекта: в поставке `hds` резолвится из копии, в разработке
# (`sidecar/hds` отсутствует) — из корня проекта (источник истины). Каталог
# `sidecar/` нужен и для общего пакета `hds_sidecar`.
_SIDE = os.path.dirname(_HERE)
if _SIDE not in sys.path:
    sys.path.insert(0, _SIDE)

from hds_sidecar import config as sc_config  # noqa: E402
from hds_sidecar import progress, runtime, stdio  # noqa: E402

PROTOCOL = stdio.PROTOCOL
# Корень проекта: аргумент клиента (`--root`) важнее вычисленного от файла.
ROOT = stdio.arg("--root") or os.path.dirname(_SIDE)
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)
_cfg = None


def _state_dir():
    """Системный каталог состояния (совместимость: см. `hds_sidecar.config`)."""
    return sc_config.state_dir()


def _config_path():
    """Путь `config.yaml` — тем же порядком, что `hds_core::config::config_path`."""
    return sc_config.config_path(ROOT)


def _config(parity):
    """Конфиг извлечения; при parity=True — правила golden.py (изоляция)."""
    global _cfg
    if _cfg is None:
        _cfg = sc_config.load_config(_config_path(), root=ROOT)
    cfg = copy.deepcopy(_cfg)
    idx = dict(cfg.get("index") or {})
    if parity:
        idx["roots"] = []
        idx["clip"] = False
        wdir = os.path.join(
            ROOT, "models", "whisper-" + str(idx.get("whisper_model", "small"))
        )
        idx["transcribe"] = os.path.exists(os.path.join(wdir, "model.bin"))
    cfg["index"] = idx
    return cfg


def _tesseract_available(cfg):
    """Дешёвая проверка OCR без запуска tesseract.

    Грабля B6: `pytesseract.get_tesseract_version()` запускает `tesseract.exe`,
    который наследует stdin-pipe воркера и **блокируется** — `hello` не отвечает.
    Поэтому проверяем наличие бинаря (`ocr_tesseract_cmd` или PATH).
    """
    from hds.config import dig

    cmd = (dig(cfg, "index.ocr_tesseract_cmd", "") or "").strip()
    if cmd and os.path.exists(cmd):
        return True
    import shutil

    return shutil.which("tesseract") is not None


def _capabilities():
    caps = ["text", "pdf", "docx", "xlsx", "pptx"]
    try:
        if _tesseract_available(_config(False)):
            caps.append("ocr")
    except Exception:  # noqa: BLE001
        pass
    try:
        from hds import lemmatizer

        if lemmatizer.available():
            caps.append("normalize")
    except Exception:  # noqa: BLE001
        pass
    try:
        import importlib.util

        # MS Project (.mpp) через mpxj/Java: репортим возможность, чтобы `hds check`
        # и UI видели деградацию (без Java extract_mpp отдаёт пометку, не падает).
        if importlib.util.find_spec("jpype") and importlib.util.find_spec("mpxj"):
            caps.append("mpp")
    except Exception:  # noqa: BLE001
        pass
    return caps


def _handle(req):
    mid = req.get("id")
    method = req.get("method")
    params = req.get("params") or {}

    if method == "hello":
        stdio.ok(
            mid,
            {
                "protocol": PROTOCOL,
                "python": sys.version.split()[0],
                "pid": os.getpid(),
                "capabilities": _capabilities(),
            },
        )
        return False

    if method == "extract":
        from hds import extractors

        path = params.get("path", "")
        parity = bool((params.get("opts") or {}).get("parity"))
        cfg = _config(parity)
        t0 = time.time()
        try:
            # библиотеки печатают в stdout — уводим их в stderr
            with progress.redirect_stdout():
                kind, segs = extractors.extract(path, cfg)
            segs_out = [
                {
                    "text": s.get("text"),
                    "page": s.get("page"),
                    "t_start": s.get("t_start"),
                    "t_end": s.get("t_end"),
                    "head": s.get("head"),
                }
                for s in segs
            ]
            stdio.ok(
                mid,
                {
                    "kind": kind,
                    "segments": segs_out,
                    "warnings": [],
                    "elapsed_ms": round((time.time() - t0) * 1000, 1),
                },
            )
        except Exception as e:  # noqa: BLE001
            stdio.err(mid, -32001, "%s: %s" % (type(e).__name__, e))
        return False

    if method == "normalize":
        from hds import lemmatizer

        texts = params.get("texts") or []
        stdio.ok(mid, {"lemmas": [lemmatizer.normalize(t) for t in texts]})
        return False

    if method == "clip_image":
        # CLIP переезжает в Rust на ONNX (§7); воркер отвечает «не поддерживаю»
        stdio.err(
            mid,
            runtime.CODE_UNKNOWN_METHOD,
            "clip_image не поддерживается воркером (CLIP — в Rust)",
        )
        return False

    if method == "shutdown":
        stdio.ok(mid, {"bye": True})
        return True

    stdio.err(mid, runtime.CODE_UNKNOWN_METHOD, "unknown method: %s" % method)
    return False


def main():
    # Изоляция протокола — ДО любого импорта тяжёлых библиотек.
    stdio.init()
    idle_timeout = 60.0
    try:
        idle_timeout = float(_config(False).get("extract", {}).get("idle_timeout", 60))
    except Exception:  # noqa: BLE001
        pass
    # Явный аргумент от клиента (`hds-extract::WorkerConfig.idle_timeout`) важнее
    # конфига: batch-операции (`reindex-fts`/`index`) между запросами делают долгую
    # работу родителя (FTS-DELETE, эмбеддинги) и не должны терять воркер по простою.
    arg_idle = stdio.arg("--idle-timeout")
    if arg_idle is not None:
        try:
            idle_timeout = float(arg_idle)
        except Exception:  # noqa: BLE001
            pass
    return runtime.serve(_handle, idle_timeout)


if __name__ == "__main__":
    sys.exit(main())

