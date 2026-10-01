"""Mock-воркер для теста клиента (stdlib-only, без hds).

Повторяет изоляцию протокола (fd 1 → stderr) и контракт §5 в минимуме.
"""
import json
import os
import sys

_fd = os.dup(1)
os.dup2(2, 1)
sys.stdout = sys.stderr
try:
    sys.stdin.reconfigure(encoding="utf-8")
except Exception:  # noqa: BLE001
    pass
_p = os.fdopen(_fd, "w", encoding="utf-8", newline="\n")


def _send(obj):
    _p.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _p.flush()


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        req = json.loads(line)
        mid = req.get("id")
        m = req.get("method")
        if m == "hello":
            _send({"jsonrpc": "2.0", "id": mid, "result": {
                "protocol": 1, "python": "mock", "pid": os.getpid(),
                "capabilities": ["text", "normalize"]}})
        elif m == "normalize":
            texts = (req.get("params") or {}).get("texts") or []
            _send({"jsonrpc": "2.0", "id": mid, "result": {
                "lemmas": ["M:" + t for t in texts]}})
        elif m == "extract":
            _send({"jsonrpc": "2.0", "id": mid, "result": {
                "kind": "text", "segments": [{"text": "mock", "page": None}],
                "warnings": [], "elapsed_ms": 0.1}})
        elif m == "shutdown":
            _send({"jsonrpc": "2.0", "id": mid, "result": {"bye": True}})
            break
        else:
            _send({"jsonrpc": "2.0", "id": mid, "error": {
                "code": -32601, "message": "unknown method: %s" % m}})
    return 0


if __name__ == "__main__":
    sys.exit(main())