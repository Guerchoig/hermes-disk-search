"""Пробный sidecar-воркер извлечения (спайк 3, W0 §4 п.5).

Реализует контракт §5 плана в минимуме: NDJSON JSON-RPC по stdio
  {"id":1,"method":"hello"}                      → capabilities
  {"id":2,"method":"extract","params":{"path":…}} → {kind, segments, ms}
  EOF на stdin → штатное завершение (родитель владеет процессом).

Используется для замера: холодный старт, RSS, поведение при простое, реакция AV.
"""
import io
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, ROOT)

_caps = []


def main():
    root = ROOT
    if "--root" in sys.argv:
        root = sys.argv[sys.argv.index("--root") + 1]
    sys.path.insert(0, root)

    from hds.config import load
    from hds import extractors, lemmatizer

    cfg = load(os.path.join(root, "config.yaml"))
    caps = ["text", "pdf", "docx", "xlsx", "pptx"]
    if extractors._tesseract_ready(cfg):
        caps.append("ocr")
    if lemmatizer.available():
        caps.append("normalize")

    out = sys.stdout
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:  # noqa: BLE001
            out.write(json.dumps({"id": None, "error": "bad json: %s" % e}) + "\n")
            out.flush()
            continue
        mid = req.get("id")
        method = req.get("method")
        if method == "hello":
            resp = {"id": mid, "result": {"protocol": 1, "python": sys.version.split()[0],
                                          "capabilities": caps,
                                          "pid": os.getpid()}}
        elif method == "extract":
            p = (req.get("params") or {}).get("path", "")
            t0 = time.time()
            try:
                # ВАЖНО: библиотеки извлечения печатают в stdout — уводим их в stderr,
                # иначе NDJSON-поток протокола ломается (выявлено спайком 3)
                import contextlib
                buf = io.StringIO()
                with contextlib.redirect_stdout(sys.stderr):
                    kind, segs = extractors.extract(p, cfg)
                resp = {"id": mid, "result": {
                    "kind": kind,
                    "segments": [{"text": s.get("text"), "page": s.get("page"),
                                  "t_start": s.get("t_start"), "t_end": s.get("t_end"),
                                  "head": s.get("head")} for s in segs],
                    "ms": round((time.time() - t0) * 1000, 1)}}
            except Exception as e:  # noqa: BLE001
                resp = {"id": mid, "error": "%s: %s" % (type(e).__name__, e)}
        elif method == "normalize":
            text = (req.get("params") or {}).get("text", "")
            resp = {"id": mid, "result": {"text": lemmatizer.normalize(text)}}
        else:
            resp = {"id": mid, "error": "unknown method: %s" % method}
        out.write(json.dumps(resp, ensure_ascii=False) + "\n")
        out.flush()
    # stdin закрыт → штатный выход (в контракте §5 это основной путь остановки)
    return 0


if __name__ == "__main__":
    sys.exit(main())