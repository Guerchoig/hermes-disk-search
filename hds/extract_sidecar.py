"""Тонкий Python-мост для Rust-конвейера (B4; прототип контракта B6).

Зачем: извлечение сегментов (`hds.extractors`) и лемматизация FTS
(`hds.lemmatizer`, pymorphy3) остаются в Python (MIGRATION_PLAN_RUST.md §2.5),
а Rust-конвейеру `process_file` они нужны для паритета golden.

Протокол — JSON-lines: одна строка JSON на stdin, одна строка JSON на stdout.
Операции (PLAN_W2_LLM_HOST.md §5): hello | extract | normalize | shutdown.

Грабли (W0/спайк 3), учтённые здесь:
* библиотеки и нативные процессы (ffmpeg) пишут в **fd 1** напрямую — поэтому
  настоящий stdout уводим на дубликат fd, а fd 1 перенаправляем в stderr;
* Python на Windows читает stdin в ANSI-кодировке — принудительно UTF-8
  (иначе кириллические имена файлов превращаются в мозаику).

Запуск (из корня репозитория):
  .venv/Scripts/python.exe -m hds.extract_sidecar
"""
import json
import os
import sys

# Настоящий stdout — только протокол: дублируем fd 1, затем fd 1 -> fd 2,
# чтобы вывод библиотек/нативных процессов (ffmpeg) не портил протокол.
_proto_fd = os.dup(1)
os.dup2(2, 1)
try:
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
except Exception:  # noqa: BLE001
    pass
sys.stdout = sys.stderr
try:
    sys.stdin.reconfigure(encoding="utf-8")
except Exception:  # noqa: BLE001
    pass
_proto = os.fdopen(_proto_fd, "w", encoding="utf-8", newline="\n")

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)


def _config(parity):
    """Конфиг для извлечения; при parity=True — правила golden.py (изоляция)."""
    from hds.config import load

    cfg = load()
    idx = dict(cfg.get("index") or {})
    if parity:
        idx["roots"] = []
        idx["clip"] = False
        # транскрипция — только если модель уже скачана (без сетевых загрузок)
        wdir = os.path.join(
            _ROOT, "models", "whisper-" + str(idx.get("whisper_model", "small")))
        idx["transcribe"] = os.path.exists(os.path.join(wdir, "model.bin"))
    cfg["index"] = idx
    return cfg


def _handle(req):
    op = req.get("op")
    if op == "hello":
        from hds import lemmatizer

        return {"ok": True, "version": 1, "pymorphy": lemmatizer.available()}
    if op == "extract":
        from hds import extractors

        cfg = _config(bool(req.get("parity")))
        kind, segs = extractors.extract(req.get("path"), cfg)
        return {"ok": True, "kind": kind, "segments": segs}
    if op == "normalize":
        from hds import lemmatizer

        texts = req.get("texts") or []
        return {"ok": True, "fts": [lemmatizer.normalize(t) for t in texts]}
    if op == "shutdown":
        return {"ok": True, "bye": True}
    return {"ok": False, "error": "unknown op: %s" % op}


def _reply(obj):
    _proto.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _proto.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:  # noqa: BLE001
            _reply({"ok": False, "error": "bad json: %s" % e})
            continue
        try:
            resp = _handle(req)
        except Exception as e:  # noqa: BLE001
            resp = {"ok": False, "error": str(e)[:500]}
        _reply(resp)
        if req.get("op") == "shutdown":
            break


if __name__ == "__main__":
    main()