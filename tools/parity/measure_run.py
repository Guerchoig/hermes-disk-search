"""Замеры памяти W0 (§13.1): сценарии A–D на ИЗОЛИРОВАННОЙ БД.

Боевой индекс не затрагивается: конфиг out/measure.yaml указывает
db_path=out/measure.db и корень out/bench.

  A idle          — watcher запущен, очередь пуста (60 с сэмплирования);
  B index         — индексация 500 файлов;
  C transcribe    — индексация папки с медиа (загрузка whisper);
  D first_search  — первый поисковый запрос (гипотеза про pymorphy3/CLIP).

Метрики: WorkingSet + PrivateBytes по ролям + занятая VRAM.
Итог — out/measure_results.json.
"""
import json
import os
import shutil
import subprocess
import threading
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
BENCH = os.path.join(OUT, "bench")
MEDIA = os.path.join(OUT, "bench_media")
CFG = os.path.join(OUT, "measure.yaml")
SAMPLE = os.path.join(BASE, "sample_procs_light.ps1")
PY = os.path.join(ROOT, ".venv", "Scripts", "python.exe")
N_FILES = 500


def sh(cmd, timeout=3600, env=None):
    e = dict(os.environ)
    if env:
        e.update(env)
    r = subprocess.run(cmd, capture_output=True, timeout=timeout, env=e, cwd=ROOT)
    return ((r.stdout or b"").decode("utf-8", "replace"),
            (r.stderr or b"").decode("utf-8", "replace"), r.returncode)


def kill_tree(pid):
    """Убить дерево процессов (роли hds — пары launcher→worker, terminate() мало)."""
    subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)], capture_output=True)


def heartbeat_age():
    hb = os.path.join(ROOT, "index.heartbeat.json")
    if not os.path.exists(hb):
        return None
    return time.time() - os.path.getmtime(hb)


def wait_heartbeat_stale(limit_sec=60):
    """Ждём, пока чужой/свой heartbeat перестанет блокировать запуск (index_running < 30 с)."""
    t0 = time.time()
    while time.time() - t0 < limit_sec:
        age = heartbeat_age()
        if age is None or age > 35:
            return True
        time.sleep(5)
    return False


def sample_once():
    out, _, rc = sh(["powershell", "-NoProfile", "-File", SAMPLE], timeout=120)
    try:
        return json.loads(out.strip().splitlines()[-1])
    except Exception:  # noqa: BLE001
        return None


class Sampler(threading.Thread):
    """Сэмплирование процессов и VRAM. Интервал 20 с: Win32_PerfFormattedData
    тяжелый и при частых опросах сам тормозит замеряемую нагрузку (наблюдалось:
    частота индексации падала с 400 до 2 файлов/мин)."""

    def __init__(self, interval=20):
        super().__init__(daemon=True)
        self.interval = interval
        self.stop_event = threading.Event()
        self.samples = []

    def run(self):
        while not self.stop_event.is_set():
            s = sample_once()
            if s:
                self.samples.append(s)
            self.stop_event.wait(self.interval)

    def stop(self):
        self.stop_event.set()
        self.join(timeout=30)

    def summary(self):
        agg = {}
        for s in self.samples:
            for p in s.get("procs", []):
                a = agg.setdefault(p["role"], {"ws_peak": 0, "priv_peak": 0, "n": 0})
                a["ws_peak"] = max(a["ws_peak"], p["ws_mb"])
                a["priv_peak"] = max(a["priv_peak"], p["private_mb"])
                a["n"] += 1
        vram = [s["vram_used_mib"] for s in self.samples if s.get("vram_used_mib", -1) > 0]
        return {"roles": agg, "vram_peak_mib": max(vram) if vram else None,
                "vram_min_mib": min(vram) if vram else None,
                "samples": len(self.samples),
                "last": self.samples[-1] if self.samples else None}
def make_config():
    db = os.path.join(OUT, "measure.db").replace("\\", "/")
    cfg = """# Изолированная конфигурация для замеров W0 (боевой индекс не затрагивается)
index:
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
  whisper_device: auto
  whisper_batch: 8
  clip: false
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
chat:
  base_url: "http://127.0.0.1:8010/v1"
  model: "qwen3.5-9b"
  thinking: "off"
llm_server:
  autostart: false
mcp_http:
  autostart: false
""" % (BENCH.replace("\\", "/"), db)
    with open(CFG, "w", encoding="utf-8") as f:
        f.write(cfg)
    return CFG


def make_bench():
    """500 файлов: текст + копии docx/xlsx/pptx/pdf из test_data."""
    if os.path.isdir(BENCH) and len(os.listdir(BENCH)) >= N_FILES:
        return
    os.makedirs(BENCH, exist_ok=True)
    src = os.path.join(ROOT, "test_data")
    copies = [f for f in os.listdir(src)
              if f.lower().endswith((".docx", ".xlsx", ".pptx", ".pdf", ".txt", ".csv"))]
    for i in range(1, N_FILES + 1):
        if copies and i % 5 == 0:
            c = copies[i % len(copies)]
            name = "bench_%04d%s" % (i, os.path.splitext(c)[1])
            shutil.copy2(os.path.join(src, c), os.path.join(BENCH, name))
        else:
            with open(os.path.join(BENCH, "bench_%04d.txt" % i), "w",
                      encoding="utf-8") as f:
                f.write("Файл замера %d. Проект ИРИС, 1С:Документооборот, согласование "
                        "договоров подряда.\n" % i)
                for k in range(40):
                    f.write("Строка %d: описание позиции реестра систем %d, система %d, "
                            "порог согласования %d рублей.\n" % (k, i, i % 20, i * 100))
    print("создано файлов в bench: %d" % len(os.listdir(BENCH)))


def make_media():
    """Короткий аудиофайл для сценария транскрипции."""
    os.makedirs(MEDIA, exist_ok=True)
    dst = os.path.join(MEDIA, "медиа_замер.wav")
    clip = os.path.join(OUT, "ru_clip", "clip_300_360.wav")
    if os.path.exists(clip) and not os.path.exists(dst):
        subprocess.run(["ffmpeg", "-y", "-t", "30", "-i", clip, dst],
                       capture_output=True, timeout=300)
    return dst if os.path.exists(dst) else None


def run_scenario(name, cmd, env=None, max_sec=3600):
    print("\n=== сценарий %s ===" % name)
    s = Sampler(interval=20)
    s.start()
    # Вывод пишем в ФАЙЛ, а не в PIPE: индексатор печатает много, и невычитываемый
    # PIPE переполняется (64 КБ) — процесс встаёт на записи (наблюдалось: 34 файла).
    log_path = os.path.join(OUT, "run_%s.log" % "".join(
        c if c.isalnum() else "_" for c in name)[:40])
    log = open(log_path, "w", encoding="utf-8", errors="replace")
    proc = subprocess.Popen(cmd, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                            env={**os.environ, **(env or {})})
    t0 = time.time()
    while proc.poll() is None and time.time() - t0 < max_sec:
        time.sleep(2)
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            proc.kill()
    kill_tree(proc.pid)                # дерево, а не только родитель
    log.close()
    with open(log_path, encoding="utf-8", errors="replace") as f:
        out = f.read()
    s.stop()
    res = s.summary()
    res["sec"] = round(time.time() - t0, 1)
    res["exit_code"] = proc.returncode
    res["log"] = os.path.relpath(log_path, ROOT)
    res["stdout_tail"] = out[-600:]
    roles = ", ".join("%s ws=%.0f/priv=%.0f" % (k, v["ws_peak"], v["priv_peak"])
                      for k, v in sorted(res["roles"].items()))
    print("  %.1f с, код=%s | %s | VRAM пик=%s"
          % (res["sec"], res["exit_code"], roles, res["vram_peak_mib"]))
    return res
def main():
    os.makedirs(OUT, exist_ok=True)
    make_bench()
    media = make_media()
    make_config()
    results = {"config": CFG, "bench_files": len(os.listdir(BENCH)),
               "media": media}
    env = {"HDS_CONFIG": CFG}
    # остаток от предыдущей сессии: index.pause ставит НОВЫЕ прогоны на паузу.
    # Для изолированных замеров убираем его на время и возвращаем в конце.
    pause = os.path.join(ROOT, "index.pause")
    pause_bak = pause + ".parity_measure_bak"
    if os.path.exists(pause):
        os.replace(pause, pause_bak)
        results["index_pause_moved"] = True
    try:
        run_scenarios(results, media, env)
    finally:
        if os.path.exists(pause_bak):
            os.replace(pause_bak, pause)
            results["index_pause_restored"] = True
    with open(os.path.join(OUT, "measure_results.json"), "w", encoding="utf-8") as f:
        json.dump(results, f, ensure_ascii=False, indent=1)
    print("\nсохранено: out/measure_results.json")


def run_scenarios(results, media, env):
    # B: индексация 500 файлов на пустой изолированной БД (первой — иначе мерить нечего)
    results["B_index_500"] = run_scenario(
        "B index 500 файлов",
        [PY, "-m", "hds.cli", "index", "--roots", BENCH, "--quiet"], env=env)

    # A: простой с работающим watcher'ом (после B indexed — watcher действительно простаивает)
    wlog = open(os.path.join(OUT, "run_watcher.log"), "w", encoding="utf-8",
                errors="replace")
    watcher = subprocess.Popen([PY, "-m", "hds.cli", "watch"], cwd=ROOT,
                               stdout=wlog, stderr=subprocess.STDOUT,
                               env={**os.environ, **env})
    time.sleep(30)                       # старт + reconcile (файлы уже в индексе)
    s = Sampler(interval=20)
    s.start()
    time.sleep(60)                       # простой 60 с (в плане 10 мин — зафиксировано)
    s.stop()
    results["A_idle_watcher"] = s.summary()
    print("\n=== сценарий A idle (watcher) ===")
    print("  роли: %s | VRAM пик=%s"
          % (json.dumps(results["A_idle_watcher"]["roles"], ensure_ascii=False),
             results["A_idle_watcher"]["vram_peak_mib"]))
    watcher.terminate()
    try:
        watcher.wait(timeout=20)
    except subprocess.TimeoutExpired:
        watcher.kill()
    kill_tree(watcher.pid)
    wlog.close()
    print("  heartbeat устарел: %s (возраст %.0f с)"
          % (wait_heartbeat_stale(), heartbeat_age() or -1))

    # C: транскрипция
    if media:
        results["C_transcribe"] = run_scenario(
            "C транскрипция 30 с аудио",
            [PY, "-m", "hds.cli", "index", "--roots", MEDIA, "--quiet", "--full"],
            env=env)

    # D: первый поисковый запрос (гипотеза pymorphy3/CLIP)
    out, err, rc = sh([PY, "-m", "hds.cli", "search", "документооборот согласование",
                       "--limit", "5"], timeout=1800, env=env)
    results["D_first_search"] = {"rc": rc, "out_tail": out[-500:], "err_tail": err[-300:],
                                 "sample_after": sample_once()}
    print("\n=== сценарий D первый поиск ===")
    print("  rc=%s; процессы: %s" % (rc, json.dumps(
        results["D_first_search"]["sample_after"], ensure_ascii=False)[:300]))

    with open(os.path.join(OUT, "measure_results.json"), "w", encoding="utf-8") as f:
        json.dump(results, f, ensure_ascii=False, indent=1)
    print("\nсохранено: out/measure_results.json")


if __name__ == "__main__":
    main()