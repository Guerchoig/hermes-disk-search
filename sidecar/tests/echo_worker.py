"""Минимальный воркер для теста протокола: hello/echo/boom/shutdown.

Запускается как процесс (stdio.init перехватывает fd 1 — в одном процессе с
тестом это сломало бы вывод).
"""
from __future__ import annotations

import os
import sys
import threading

_HERE = os.path.dirname(os.path.abspath(__file__))
_SIDECAR = os.path.dirname(_HERE)
if _SIDECAR not in sys.path:
    sys.path.insert(0, _SIDECAR)

from hds_sidecar import runtime, stdio  # noqa: E402

#: Флаг отмены (тест `cancel` во время долгого запроса).
CANCELLED = threading.Event()


def control(req: dict) -> None:
    """Управляющий запрос в потоке-читателе."""
    if req.get("method") == "cancel":
        CANCELLED.set()


def handle(req: dict) -> bool:
    mid = req.get("id")
    method = req.get("method")
    if method == "hello":
        stdio.ok(mid, {"protocol": stdio.PROTOCOL, "pid": os.getpid()})
        return False
    if method == "echo":
        stdio.ok(mid, req.get("params") or {})
        return False
    if method == "sleep":
        seconds = float((req.get("params") or {}).get("seconds") or 10.0)
        CANCELLED.wait(seconds)
        stdio.ok(mid, {"cancelled": CANCELLED.is_set()})
        CANCELLED.clear()
        return False
    if method == "cancel":
        stdio.ok(mid, {"cancelled": True})
        return False
    if method == "boom":
        raise RuntimeError("искусственный сбой")
    if method == "shutdown":
        stdio.ok(mid, {"bye": True})
        return True
    stdio.err(mid, runtime.CODE_UNKNOWN_METHOD, "unknown method: %s" % method)
    return False


def main() -> int:
    stdio.init()
    idle = float(stdio.arg("--idle-timeout") or 5)
    return runtime.serve(handle, idle, control=control)


if __name__ == "__main__":
    sys.exit(main())