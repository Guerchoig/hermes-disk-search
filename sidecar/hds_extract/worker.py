"""Автономный sidecar-воркер извлечения/нормализации (B6, контракт §5 плана).

Транспорт: stdio + JSON-RPC 2.0, кадрирование NDJSON (один JSON-объект на строку).
Методы: hello | extract | normalize | clip_image | shutdown.

Грабли (W0/спайк 3), учтённые здесь:
* библиотеки и нативные процессы (ffmpeg) пишут в **fd 1** напрямую — настоящий
  stdout уводим на дубликат fd, а fd 1 перенаправляем в stderr (протокол чист);
* Python на Windows читает stdin в ANSI — принудительно UTF-8;
* `select()` по pipe на Windows не работает — читатель в отдельном потоке,
  главный цикл ждёт запрос с таймаутом простоя (`extract.idle_timeout`, по умолчанию 60 с);
* EOF на stdin → штатный выход (родитель владеет процессом, §5.1).

Запуск: python worker.py --root <путь проекта>
"""
import contextlib
import json
import os
import queue
import sys
import threading
import time

# --- изоляция протокола от вывода библиотек ---------------------------------
_proto_fd = os.dup(1)
os.dup2(2, 1)
try:
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:  # noqa: BLE001
    pass
sys.stdout = sys.stderr
# stdin: протокол читаем с ДУБЛИКАТА fd 0, а сам fd 0 уводим в nul — иначе
# подпроцессы (tesseract/ffmpeg/java) наследуют протокольный pipe и блокируются
# на чтении (грабля B6: extract pdf висел 120 с, пока не закроешь stdin).
_proto_in = os.fdopen(os.dup(0), "rb")
_devnull = os.open(os.devnull, os.O_RDONLY)
os.dup2(_devnull, 0)
os.close(_devnull)
_proto = os.fdopen(_proto_fd, "w", encoding="utf-8", newline="\n")

PROTOCOL = 1


def _reply(obj):
    _proto.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _proto.flush()


def _ok(mid, result):
    _reply({"jsonrpc": "2.0", "id": mid, "result": result})


def _err(mid, code, message, hint=None):
    error = {"code": code, "message": message}
    if hint:
        error["hint"] = hint
    _reply({"jsonrpc": "2.0", "id": mid, "error": error})


def _arg(name, default=None):
    if name in sys.argv:
        return sys.argv[sys.argv.index(name) + 1]
    return default


_HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = _arg("--root") or os.path.dirname(os.path.dirname(_HERE))
# Поставка: рядом с `hds_extract` лежит самодостаточная копия модулей извлечения
# под `sidecar/hds` (собирает installers/build_sidecar.ps1). Кладём каталог sidecar
# в path ПЕРЕД корнем проекта: в поставке `hds` резолвится из копии, в разработке
# (`sidecar/hds` отсутствует) — из корня проекта (источник истины).
_SIDE = os.path.dirname(_HERE)
if _SIDE not in sys.path:
    sys.path.insert(0, _SIDE)
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)

_cfg = None

# Имя системного каталога состояния (M2 §3.2): там server-поставка держит `config.yaml`.
STATE_DIR_NAME = "hermes-disk-search"


def _state_dir():
    """Системный каталог состояния: Windows — `%ProgramData%\\hermes-disk-search`,
    macOS — `/Library/Application Support/hermes-disk-search`; иначе None."""
    if os.name == "nt":
        pd = os.environ.get("ProgramData")
        return os.path.join(pd, STATE_DIR_NAME) if pd else None
    if sys.platform == "darwin":
        return os.path.join("/Library", "Application Support", STATE_DIR_NAME)
    return None


def _config_path():
    """Путь `config.yaml` — тем же порядком, что `hds_core::config::config_path`.

    `HDS_CONFIG` → `<ROOT>/config.yaml` (desktop-раскладка) → системный каталог
    состояния (server-раскладка: `app\\<версия>` только для чтения, конфиг
    лежит в `%ProgramData%\\hermes-disk-search`, M2 T2) → `<ROOT>/config.yaml`
    (для внятного сообщения об ошибке).

    РЕГРЕССИЯ (20.10.2026, релиз 0.20.5): раньше читался **только**
    `<ROOT>/config.yaml`, а у служб `HDS_CONFIG` указывал на ProgramData —
    воркер падал с `FileNotFoundError` на первом же `extract`, процесс умирал,
    и все следующие файлы получали «воркер: запись: Идёт закрытие канала»
    (43 537 ошибок в БД за один прогон).
    """
    env = (os.environ.get("HDS_CONFIG") or "").strip()
    if env:
        return env
    root_cfg = os.path.join(ROOT, "config.yaml")
    if os.path.exists(root_cfg):
        return root_cfg
    state = _state_dir()
    if state:
        sys_cfg = os.path.join(state, "config.yaml")
        if os.path.exists(sys_cfg):
            return sys_cfg
    return root_cfg


def _config(parity):
    """Конфиг извлечения; при parity=True — правила golden.py (изоляция)."""
    global _cfg
    if _cfg is None:
        from hds.config import load

        _cfg = load(_config_path())
    import copy

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
        return _ok(
            mid,
            {
                "protocol": PROTOCOL,
                "python": sys.version.split()[0],
                "pid": os.getpid(),
                "capabilities": _capabilities(),
            },
        )

    if method == "extract":
        from hds import extractors

        path = params.get("path", "")
        parity = bool((params.get("opts") or {}).get("parity"))
        cfg = _config(parity)
        t0 = time.time()
        try:
            # библиотеки печатают в stdout — уводим их в stderr
            with contextlib.redirect_stdout(sys.stderr):
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
            return _ok(
                mid,
                {
                    "kind": kind,
                    "segments": segs_out,
                    "warnings": [],
                    "elapsed_ms": round((time.time() - t0) * 1000, 1),
                },
            )
        except Exception as e:  # noqa: BLE001
            return _err(mid, -32001, "%s: %s" % (type(e).__name__, e))

    if method == "normalize":
        from hds import lemmatizer

        texts = params.get("texts") or []
        return _ok(mid, {"lemmas": [lemmatizer.normalize(t) for t in texts]})

    if method == "clip_image":
        # CLIP переезжает в Rust на ONNX (§7); воркер отвечает «не поддерживаю»
        return _err(mid, -32601, "clip_image не поддерживается воркером (CLIP — в Rust)")

    if method == "shutdown":
        return _ok(mid, {"bye": True})

    return _err(mid, -32601, "unknown method: %s" % method)


def main():
    idle_timeout = 60.0
    try:
        idle_timeout = float(_config(False).get("extract", {}).get("idle_timeout", 60))
    except Exception:  # noqa: BLE001
        pass
    # Явный аргумент от клиента (`hds-extract::WorkerConfig.idle_timeout`) важнее
    # конфига: batch-операции (`reindex-fts`/`index`) между запросами делают долгую
    # работу родителя (FTS-DELETE, эмбеддинги) и не должны терять воркер по простою.
    arg_idle = _arg("--idle-timeout")
    if arg_idle is not None:
        try:
            idle_timeout = float(arg_idle)
        except Exception:  # noqa: BLE001
            pass

    lines = queue.Queue()

    def reader():
        # читаем протокол с дубликата fd 0 (сам fd 0 уведён в nul, см. выше)
        while True:
            raw = _proto_in.readline()
            if not raw:
                break
            lines.put(raw.decode("utf-8", "replace"))
        lines.put(None)  # EOF

    threading.Thread(target=reader, daemon=True).start()

    while True:
        try:
            line = lines.get(timeout=idle_timeout if idle_timeout > 0 else None)
        except queue.Empty:
            break  # простой → штатный выход (0 процессов/0 МБ в ожидании)
        if line is None:
            break  # EOF на stdin
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:  # noqa: BLE001
            _err(None, -32700, "bad json: %s" % e)
            continue
        try:
            _handle(req)
        except Exception as e:  # noqa: BLE001
            # Одна неожиданная ошибка (нечитаемый конфиг, сбой библиотеки) не должна
            # убивать процесс: иначе клиент пишет в закрытый канал и **каждый**
            # следующий файл падает (регрессия 0.20.5: 43 537 файлов «закрытие
            # канала» после одного падения). Отвечаем структурной ошибкой.
            _err(req.get("id"), -32603, "internal error: %s: %s" % (type(e).__name__, e))
        if req.get("method") == "shutdown":
            break
    return 0


if __name__ == "__main__":
    sys.exit(main())