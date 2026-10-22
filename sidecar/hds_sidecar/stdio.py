"""Транспорт sidecar-воркера: stdin/stdout, NDJSON, JSON-RPC 2.0.

Контракт — `sidecar/README.md` и PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §5.2.
Версия протокола одна для извлечения и транскрибации (`PROTOCOL = 1`): новые
методы и поля аддитивны, `Worker::handshake` сверяет ровно `1`.

Грабли (W0/спайк 3), учтённые здесь:
* библиотеки и нативные процессы (ffmpeg, tesseract, java) пишут в **fd 1**
  напрямую — настоящий stdout уводим в дубликат, а fd 1 перенаправляем в stderr,
  чтобы протокол остался чистым;
* Python на Windows читает stdin в ANSI — читаем байты и декодируем UTF-8;
* протокол читаем с ДУБЛИКАТА fd 0, а сам fd 0 уводим в nul — иначе подпроцессы
  наследуют протокольный pipe и блокируются на чтении (грабля B6: extract pdf
  висел 120 с, пока не закроешь stdin).
"""
from __future__ import annotations

import json
import os
import sys

#: Версия протокола (`hello.protocol`); родитель сверяет её.
PROTOCOL = 1

_proto_out = None  # type: ignore[var-annotated]
_proto_in = None  # type: ignore[var-annotated]
_initialized = False


def init() -> None:
    """Изолировать протокол от вывода библиотек. Идемпотентно.

    Вызывать **до** любого импорта тяжёлых библиотек: они могут печатать в
    stdout уже при импорте.
    """
    global _proto_out, _proto_in, _initialized
    if _initialized:
        return
    proto_fd = os.dup(1)
    os.dup2(2, 1)
    try:
        sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    except Exception:  # noqa: BLE001 - старый интерпретатор/перехваченный stderr
        pass
    sys.stdout = sys.stderr
    _proto_in = os.fdopen(os.dup(0), "rb")
    devnull = os.open(os.devnull, os.O_RDONLY)
    try:
        os.dup2(devnull, 0)
    finally:
        os.close(devnull)
    _proto_out = os.fdopen(proto_fd, "w", encoding="utf-8", newline="\n")
    _initialized = True


def readline() -> bytes:
    """Сырая строка запроса (без перевода строки); `b""` — EOF."""
    if _proto_in is None:
        return b""
    return _proto_in.readline()


def reply(obj) -> None:
    """Отправить один JSON-объект в протокольный канал (NDJSON)."""
    if _proto_out is None:
        return
    _proto_out.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _proto_out.flush()


def ok(mid, result) -> None:
    """Успешный ответ JSON-RPC 2.0."""
    reply({"jsonrpc": "2.0", "id": mid, "result": result})


def err(mid, code, message, hint=None, kind=None) -> None:
    """Ошибка JSON-RPC 2.0: `{code, message, hint, kind}`.

    `code` — числовой код JSON-RPC (спецификация), `kind` — машинный код задания
    (`model_missing`, `cuda_oom`, …), по которому Rust выбирает текст/повтор.
    """
    error = {"code": code, "message": message}
    if hint:
        error["hint"] = hint
    if kind:
        error["kind"] = kind
    reply({"jsonrpc": "2.0", "id": mid, "error": error})


def arg(name, default=None):
    """Значение аргумента командной строки `--name <value>` или `default`."""
    if name in sys.argv:
        try:
            return sys.argv[sys.argv.index(name) + 1]
        except IndexError:
            return default
    return default
