"""A1 — приёмочный стенд: переанонимизация S10 базой меток v2 (режим agent).

Берёт готовый стенд A0 (`tools/parity/out/agent_a0/s10/project`), прогоняет
его через **новый** воркер анонимизации одним долгоживущим процессом:

* `docx`/`xlsx`/`md`/`txt`/`xml` → RPC `anonymize` с `mode: "agent"` и
  `project_id` (метки проекта с падежными суффиксами, `[IMAGE_n]`, маркер);
* `pdf`/`pptx`/`mpp` → RPC `text_image` (D18): текст извлекает сам воркер
  (PyMuPDF через портативный CPython) или `hds_extract`, образ —
  `<имя>.<ext>.md` с метками проекта.

База меток и каталог образов — только в `tools/parity/out/agent_a1/`
(гитигнорится). Боевые `data/anonymize.db` и `index.db` не затрагиваются.

Запуск (интерпретатор воркера — venv-anonymize):
    sidecar\\venv-anonymize\\Scripts\\python.exe tools\\parity\\agent_a1_stand.py
        [--project DIR] [--out DIR] [--db FILE] [--device cuda|cpu]
        [--project-id P0001]
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
import time
import uuid

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
BASE = os.path.join(REPO, "tools", "parity", "out", "agent_a1")
S10 = os.path.join(REPO, "tools", "parity", "out", "agent_a0", "s10")
DEFAULT_PROJECT = os.path.join(S10, "project")
DEFAULT_MODELS = os.environ.get(
    "HDS_MODELS_DIR", r"C:\ProgramData\hermes-disk-search\models"
)

TEXT_FORMATS = {".docx", ".xlsx", ".md", ".txt", ".xml"}
IMAGE_FORMATS = {".pdf", ".pptx", ".mpp", ".mpt", ".mpx"}


class Worker:
    """JSON-RPC 2.0 по stdio (NDJSON), как в A0-стенде."""

    def __init__(self, python: str, db: str, device: str, log_path: str) -> None:
        self.proc = subprocess.Popen(
            [
                python,
                os.path.join(REPO, "sidecar", "hds_anonymize", "worker.py"),
                "--root", REPO,
                "--models-dir", os.path.join(DEFAULT_MODELS, "anonymize"),
                "--db", db,
                "--device", device,
                "--natasha", "1",
                "--idle-timeout", "1800",
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=open(log_path, "w", encoding="utf-8"),
            cwd=REPO,
            text=True,
            encoding="utf-8",
            bufsize=1,
        )
        self.responses: dict[str, dict] = {}
        self._lock = threading.Lock()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "id" in message:
                with self._lock:
                    self.responses[message["id"]] = message

    def call(self, method: str, params: dict | None = None, timeout: float = 1800.0) -> dict:
        mid = uuid.uuid4().hex
        payload = {"jsonrpc": "2.0", "id": mid, "method": method}
        if params:
            payload["params"] = params
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(payload, ensure_ascii=False) + "\n")
        self.proc.stdin.flush()
        started = time.time()
        while time.time() - started < timeout:
            with self._lock:
                if mid in self.responses:
                    return self.responses.pop(mid)
            time.sleep(0.02)
        raise TimeoutError(method)

    def close(self) -> None:
        try:
            self.call("shutdown", timeout=60)
        except Exception:  # noqa: BLE001 — процесс всё равно завершаем
            pass
        try:
            self.proc.terminate()
        except Exception:  # noqa: BLE001
            pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--project", default=DEFAULT_PROJECT)
    parser.add_argument("--out", default=os.path.join(BASE, "s10", "anon"))
    parser.add_argument("--db", default=os.path.join(BASE, "anonymize.db"))
    parser.add_argument("--device", default="cuda")
    parser.add_argument("--project-id", default="P0001")
    parser.add_argument(
        "--python",
        default=os.path.join(REPO, "sidecar", "venv-anonymize", "Scripts", "python.exe"),
    )
    args = parser.parse_args()
    os.makedirs(BASE, exist_ok=True)

    for suffix in ("", "-wal", "-shm"):
        try:
            os.remove(args.db + suffix)
        except FileNotFoundError:
            pass

    log_path = os.path.join(BASE, "anonymize_worker.log")
    worker = Worker(args.python, args.db, args.device, log_path)
    report: dict = {"mode": "agent", "project_id": args.project_id,
                    "files": [], "errors": []}
    started = time.time()
    try:
        hello = worker.call("hello")
        report["hello"] = hello.get("result", {})
        report["warmup"] = worker.call("warmup", timeout=1800).get("result", {})
        report["warmup_ms"] = round((time.time() - started) * 1000, 1)

        for root, _dirs, files in os.walk(args.project):
            for name in sorted(files):
                if name == "project.json":
                    continue
                # Технические артефакты прошлых прогонов (A0 писал текст PDF
                # рядом с исходником) — не образы проекта
                if name.endswith(".extracted.txt"):
                    continue
                src = os.path.join(root, name)
                rel = os.path.relpath(src, args.project)
                ext = os.path.splitext(name)[1].lower()
                dst_dir = os.path.join(args.out, os.path.dirname(rel))
                os.makedirs(dst_dir, exist_ok=True)
                t0 = time.time()

                if ext in IMAGE_FORMATS:
                    resp = worker.call("text_image", {
                        "in_path": src,
                        "out_dir": dst_dir,
                        "project_id": args.project_id,
                    })
                    result = resp.get("result") or {}
                    entry = {
                        "rel": rel,
                        "kind": "text_image",
                        "out": os.path.relpath(result["out_path"], BASE)
                        if result.get("out_path") else None,
                        "labels": len(result.get("labels") or []),
                        "warnings": len(result.get("warnings") or []),
                        "error": resp.get("error"),
                    }
                elif ext in TEXT_FORMATS:
                    resp = worker.call("anonymize", {
                        "in_path": src,
                        "out_path": os.path.join(dst_dir, name),
                        "project_id": args.project_id,
                        "mode": "agent",
                    })
                    result = resp.get("result") or {}
                    entry = {
                        "rel": rel,
                        "kind": "agent",
                        "out": os.path.relpath(result["out_path"], BASE)
                        if result.get("out_path") else None,
                        "tokens": len(result.get("tokens") or []),
                        "images": result.get("images"),
                        "warnings": len(result.get("warnings") or []),
                        "error": resp.get("error"),
                    }
                else:
                    report["files"].append({"rel": rel, "skipped": f"формат {ext}"})
                    continue

                entry["ms"] = round((time.time() - t0) * 1000, 1)
                report["files"].append(entry)
                if entry["error"]:
                    report["errors"].append(entry)
                print(f"{rel}: {entry['ms']} ms", flush=True)

        snapshot = worker.call("vault_snapshot", {"project_id": args.project_id})
        snap = snapshot.get("result") or {}
        report["snapshot"] = {
            "rev": snap.get("rev"),
            "labels": len(snap.get("labels") or []),
            "keep": len(snap.get("keep") or []),
        }
    finally:
        worker.close()

    with open(os.path.join(BASE, "report.json"), "w", encoding="utf-8") as handle:
        json.dump(report, handle, ensure_ascii=False, indent=2)
    print(json.dumps({
        "files": len(report["files"]),
        "errors": len(report["errors"]),
        "snapshot": report.get("snapshot"),
    }, ensure_ascii=False))
    return 0 if not report["errors"] else 1


if __name__ == "__main__":
    sys.exit(main())
