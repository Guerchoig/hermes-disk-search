"""Спайк 3 (W0 §4 п.5): портативный sidecar-воркер через uv + антивирус.

Вариант A (python-build-standalone + site-packages, БЕЗ bootloader'а):
  1. uv python install 3.12 --install-dir out/sidecar_a/python
  2. venv поверх этого интерпретатора + целевые зависимости;
  3. замер: размер на диске, холодный старт (hello), RSS в простое и на извлечении,
     штатное завершение по закрытию stdin (родитель владеет процессом, §5.1);
  4. скан Windows Defender по ключевым файлам (MpCmdRun -Scan -ScanType 3).

Вариант B (PyInstaller one-folder) не собирается: PyInstaller в окружении нет, а по
плану (§10.5) вариант A предпочтителен — нет bootloader'а, главного триггера ложных
срабатываний.
"""
import ctypes
import json
import os
import subprocess
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
SIDE = os.path.join(OUT, "sidecar_a")
PYDIR = os.path.join(SIDE, "python")
VENV = os.path.join(SIDE, "venv")
WORKER = os.path.join(BASE, "spike3_worker.py")
TREE_PS = os.path.join(BASE, "proc_tree.ps1")
TEST = os.path.join(ROOT, "test_data")
DEPS = ["pymupdf", "python-docx", "openpyxl", "python-pptx", "pymorphy3",
        "pymorphy3-dicts-ru", "pillow", "pyyaml", "pytesseract"]
MPCMD = os.path.join(os.environ.get("ProgramFiles", r"C:\Program Files"),
                     "Windows Defender", "MpCmdRun.exe")


def run(cmd, timeout=1800, **kw):
    print("$ %s" % " ".join(str(c) for c in cmd))
    r = subprocess.run(cmd, capture_output=True, timeout=timeout, **kw)
    out = (r.stdout or b"").decode("utf-8", "replace")
    err = (r.stderr or b"").decode("utf-8", "replace")
    if r.returncode != 0:
        print("  rc=%s %s" % (r.returncode, (err or out)[-400:]))
    return r.returncode, out, err


def dir_size_mb(path):
    total = 0
    for dirpath, _dirs, files in os.walk(path):
        for f in files:
            try:
                total += os.path.getsize(os.path.join(dirpath, f))
            except OSError:
                pass
    return round(total / 1048576, 1)


def proc_mem_mb(pid):
    """Память дерева процессов (uv venv python.exe — трамплин, поэтому дерево)."""
    r = subprocess.run(["powershell", "-NoProfile", "-File", TREE_PS, "-RootPid", str(pid)],
                       capture_output=True, timeout=180)
    txt = (r.stdout or b"").decode("utf-8", "replace").strip().splitlines()
    for line in reversed(txt):
        try:
            d = json.loads(line)
            return {"ws_priv_mb": d.get("ws_priv_mb"), "commit_mb": d.get("commit_mb"),
                    "tree": d.get("processes")}
        except ValueError:
            continue
    return None


def prepare():
    os.makedirs(SIDE, exist_ok=True)
    if not os.path.isdir(PYDIR):
        run(["uv", "python", "install", "3.12", "--install-dir", PYDIR])
    py = None
    for dirpath, _dirs, files in os.walk(PYDIR):
        for f in files:
            if f.lower() == "python.exe":
                p = os.path.join(dirpath, f)
                # берём интерпретатор с кратчайшим путём (корень сборки, а не шаблон venv)
                if py is None or len(p) < len(py):
                    py = p
    if not py:
        raise SystemExit("не найден python.exe в %s" % PYDIR)
    venv_py = os.path.join(VENV, "Scripts", "python.exe")
    if not os.path.exists(venv_py):
        run(["uv", "venv", "--python", py, VENV])
        run(["uv", "pip", "install", "--python", venv_py] + DEPS)
    return venv_py
def probe_worker(venv_py):
    """Холодный старт, hello, извлечение PDF/DOCX, RSS, завершение по EOF."""
    res = {}
    t0 = time.time()
    p = subprocess.Popen([venv_py, WORKER, "--root", ROOT], cwd=ROOT,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE)

    def call(obj, timeout=120):
        p.stdin.write((json.dumps(obj, ensure_ascii=False) + "\n").encode("utf-8"))
        p.stdin.flush()
        skipped = []
        for _ in range(50):                  # библиотеки могут печатать свои строки в stdout
            line = p.stdout.readline()
            if not line:
                err = (p.stderr.read() or b"").decode("utf-8", "replace")
                raise RuntimeError("воркер не ответил (упал?): %s\n%s"
                                   % (" | ".join(skipped), err[-600:]))
            text = line.decode("utf-8", "replace").strip()
            try:
                return json.loads(text)
            except ValueError:
                skipped.append(text[:120])
                continue
        raise RuntimeError("воркер не дал JSON за 50 строк")

    try:
        hello = call({"id": 1, "method": "hello"})
        res["cold_start_sec"] = round(time.time() - t0, 2)
        res["hello"] = hello.get("result", hello)
        res["rss_after_hello"] = proc_mem_mb(p.pid)

        targets = [("pdf", "отчет_цс.pdf"), ("docx", "записка_ртк.docx"),
                   ("xlsx", "реестр_систем.xlsx"), ("pptx", "презентация_цс.pptx"),
                   ("text", "проект_ирис.txt")]
        res["extracts"] = []
        for kind, name in targets:
            path = os.path.join(TEST, name)
            if not os.path.exists(path):
                continue
            t1 = time.time()
            r = call({"id": 2, "method": "extract", "params": {"path": path}})
            d = r.get("result", {})
            if r.get("error"):
                res["extracts"].append({"file": name, "error": r["error"]})
                continue
            res["extracts"].append({"file": name, "kind": d.get("kind"),
                                    "segments": len(d.get("segments") or []),
                                    "worker_ms": d.get("ms"),
                                    "wall_sec": round(time.time() - t1, 2),
                                    "text_head": ((d.get("segments") or [{}])[0]
                                                  .get("text") or "")[:80]})
        res["rss_after_extracts"] = proc_mem_mb(p.pid)
        t2 = time.time()
        p.stdin.close()
        try:
            p.wait(timeout=15)
            res["exit_on_stdin_close"] = {"ok": True, "sec": round(time.time() - t2, 2),
                                          "code": p.returncode}
        except subprocess.TimeoutExpired:
            p.kill()
            res["exit_on_stdin_close"] = {"ok": False, "note": "не завершился за 15 с"}
    finally:
        if p.poll() is None:
            p.kill()
        err = (p.stderr.read() or b"").decode("utf-8", "replace")
        res["stderr_tail"] = err[-400:]
    return res


def scan_defender(paths):
    """Скан Windows Defender по ключевым файлам (MpCmdRun -Scan -ScanType 3)."""
    out = {"mpcmd": MPCMD, "exists": os.path.exists(MPCMD), "scans": []}
    if not out["exists"]:
        return out
    for p in paths:
        if not os.path.exists(p):
            continue
        t0 = time.time()
        rc, o, e = run([MPCMD, "-Scan", "-ScanType", "3", "-File", p], timeout=3600)
        out["scans"].append({"path": os.path.relpath(p, ROOT), "rc": rc,
                             "sec": round(time.time() - t0, 1),
                             "output_tail": (o + e).strip()[-300:]})
    return out


def main():
    os.makedirs(OUT, exist_ok=True)
    res = {"variant": "A (python-build-standalone + site-packages, без bootloader)"}
    venv_py = prepare()
    res["venv_python"] = venv_py
    res["sizes_mb"] = {"python_dir": dir_size_mb(PYDIR), "venv": dir_size_mb(VENV),
                       "total": dir_size_mb(SIDE)}
    print("размеры: %s" % res["sizes_mb"])
    res["probe"] = probe_worker(venv_py)
    print("холодный старт: %s с; hello: %s" % (res["probe"].get("cold_start_sec"),
                                               res["probe"].get("hello")))
    for e in res["probe"].get("extracts", []):
        if e.get("error"):
            print("  %-24s ОШИБКА: %s" % (e["file"], str(e["error"])[:160]))
        else:
            print("  %-24s kind=%-5s segs=%-3s worker=%s мс" % (e["file"], e["kind"],
                                                                e["segments"], e["worker_ms"]))
    print("RSS: после hello=%s, после извлечений=%s, выход по EOF=%s"
          % (res["probe"].get("rss_after_hello"), res["probe"].get("rss_after_extracts"),
             res["probe"].get("exit_on_stdin_close")))
    res["defender"] = scan_defender([venv_py, WORKER,
                                     os.path.join(VENV, "Lib", "site-packages", "pymorphy3")])
    print("Defender: %s" % json.dumps(res["defender"]["scans"], ensure_ascii=False)[:600])
    with open(os.path.join(OUT, "spike3_sidecar.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike3_sidecar.json")


if __name__ == "__main__":
    main()