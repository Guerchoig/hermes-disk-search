"""Минимальный воркер для теста протокола: hello/echo/boom/shutdown.

Запускается как процесс (stdio.init перехватывает fd 1 — в одном процессе с
тестом это сломало бы вывод).
"""
from __future__ import annotations

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_SIDECAR = os.path.dirname(_HERE)
if _SIDECAR not in sys.path:
    sys.path.insert(0, _SIDECAR)

from hds_sidecar import runtime, stdio  # noqa: E402


def handle(req: dict) -> bool:
    mid = req.get("id")
    method = req.get("method")
    if method == "hello":
        stdio.ok(mid, {"protocol": stdio.PROTOCOL, "pid": os.getpid()})
        return False
    if method == "echo":
        stdio.ok(mid, req.get("params") or {})
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
    return runtime.serve(handle, idle)


if __name__ == "__main__":
    sys.exit(main())