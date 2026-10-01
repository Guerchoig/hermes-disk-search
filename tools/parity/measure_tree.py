"""Замер памяти сценариев индексации/простоя на ДЕРЕВЕ процесса (W2, задача B-4).

Методика и метрики — как в W0 (§13.1, `measure_run.py` + `sample_procs_light.ps1`):
`WorkingSet64` (ws) и `PrivateMemorySize64` (commit) по дереву процесса
(`sample_tree.ps1`), изолированные конфиг и БД в `tools/parity/out/`.

Сценарии на одном bench (500 файлов): и для Python (`hds.cli index/watch`), и для
Rust (`hds.exe index/watch`) — честное сравнение на одной машине.

Перед запуском внешне нужно остановить боевой watcher (иначе он держит `watch.lock`
и heartbeat блокирует Python-индекс). `index.pause` скрипт убирает сам и возвращает.

Запуск: .venv\\Scripts\\python.exe tools\\parity\\measure_tree.py
"""
import json
import os
import subprocess
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
BENCH = os.path.join(OUT, "bench")
SAMPLE = os.path.join(BASE, "sample_tree.ps1")
PY = os.path.join(ROOT, ".venv", "Scripts", "python.exe")
HDS = os.path.join(ROOT, "target", "debug", "hds.exe")

CFG_TMPL = """index:
  roots:
    - '%s'
  exclude_dirs: []
  exclude_paths: []
  max_file_mb: 200
  max_media_mb: 2500
  ocr: true
  ocr_lang: "rus+eng"
  ocr_tesseract_cmd: 'C:\\Program Files\\Tesseract-OCR\\tesseract.exe'
  transcribe: true
  whisper_model: small
  max_chunks: 3000
db_path: '%s'
chunk:
  size: 800
  overlap: 120
embedding:
  base_url: "http://127.0.0.1:8011/v1"
  model: "text-embedding-bge-m3"
  batch_size: 64
  dim: 1024
llm_server:
  autostart: false
mcp_http:
  autostart: false
"""


def write_cfg(tag):
    db = os.path.join(OUT, "measure_%s.db" % tag).replace("\\", "/")
    cfg = os.path.join(OUT, "measure_%s.yaml" % tag)
    with open(cfg, "w", encoding="utf-8") as f:
        f.write(CFG_TMPL % (BENCH.replace("\\", "/"), db))
    return cfg, db


def sample(root_pid):
    r = subprocess.run(["powershell", "-NoProfile", "-File", SAMPLE, "-RootPid", str(root_pid)],
                       capture_output=True, timeout=120)
    try:
        return json.loads((r.stdout or b"").decode("utf-8", "replace").strip().splitlines()[-1])
    except Exception:  # noqa: BLE001
        return None


def kill_tree(pid):
    subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)], capture_output=True)


def run_index(tag, cmd, interval=2, max_sec=1800):
    cfg, db = write_cfg(tag)
    for suf in ("", "-wal", "-shm"):
        try:
            os.remove(db + suf)
        except OSError:
            pass
    env = {**os.environ, "HDS_CONFIG": cfg}
    log = open(os.path.join(OUT, "measure_%s_index.log" % tag), "w", encoding="utf-8", errors="replace")
    t0 = time.time()
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, env=env)
    ws = cm = 0.0
    n = 0
    while p.poll() is None and time.time() - t0 < max_sec:
        s = sample(p.pid)
        if s:
            ws = max(ws, s["ws_mb"])
            cm = max(cm, s["commit_mb"])
            n += 1
        time.sleep(interval)
    kill_tree(p.pid)
    log.close()
    return {"sec": round(time.time() - t0, 1), "rc": p.returncode,
            "ws_peak_mb": ws, "commit_peak_mb": cm, "samples": n}


def run_idle(tag, cmd, start_wait=25, hold=60, interval=2):
    cfg, _db = write_cfg(tag)
    env = {**os.environ, "HDS_CONFIG": cfg}
    log = open(os.path.join(OUT, "measure_%s_watch.log" % tag), "w", encoding="utf-8", errors="replace")
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, env=env)
    time.sleep(start_wait)  # старт + reconcile
    ws = cm = 0.0
    n = 0
    t0 = time.time()
    while time.time() - t0 < hold:
        s = sample(p.pid)
        if s:
            ws = max(ws, s["ws_mb"])
            cm = max(cm, s["commit_mb"])
            n += 1
        time.sleep(interval)
    kill_tree(p.pid)
    log.close()
    return {"ws_peak_mb": ws, "commit_peak_mb": cm, "samples": n, "hold_sec": hold}


def main():
    os.makedirs(OUT, exist_ok=True)
    pause = os.path.join(ROOT, "index.pause")
    bak = pause + ".b4bak"
    if os.path.exists(pause):
        os.replace(pause, bak)
    hb = os.path.join(ROOT, "index.heartbeat.json")
    try:
        os.remove(hb)
    except OSError:
        pass

    variants = {
        "py": ([PY, "-m", "hds.cli", "index", "--roots", BENCH, "--quiet"],
               [PY, "-m", "hds.cli", "watch"]),
        "rust": ([HDS, "index", "--roots", BENCH, "--quiet"],
                 [HDS, "watch"]),
    }
    res = {}
    try:
        for tag, (idx, watch) in variants.items():
            print("=== %s: index 500 ===" % tag)
            res["%s_index_500" % tag] = run_index(tag, idx)
            print("  ", res["%s_index_500" % tag])
            print("=== %s: idle watch ===" % tag)
            res["%s_idle_watch" % tag] = run_idle(tag, watch)
            print("  ", res["%s_idle_watch" % tag])
    finally:
        if os.path.exists(bak):
            os.replace(bak, pause)
    with open(os.path.join(OUT, "measure_tree_results.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("\nсохранено: out/measure_tree_results.json")
    print(json.dumps(res, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    sys.exit(main())
