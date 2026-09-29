"""Серия замеров текущего состояния (2 минуты) в трёх метриках + VRAM.

Запускается в том состоянии, в котором машина находится сейчас (индекс-сессия
на паузе), и сохраняет серию в out/measure_live.json. Дополнительно пробует
поднять отдельный watcher на изолированном корне (сценарий A) для проверки,
не блокирует ли его чужая индекс-сессия.
"""
import json
import os
import subprocess
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
SAMPLE = os.path.join(BASE, "sample_procs.ps1")
PY = os.path.join(ROOT, ".venv", "Scripts", "python.exe")
CFG = os.path.join(OUT, "measure.yaml")
BENCH = os.path.join(OUT, "bench")


def sample():
    r = subprocess.run(["powershell", "-NoProfile", "-File", SAMPLE],
                       capture_output=True, timeout=180)
    txt = (r.stdout or b"").decode("utf-8", "replace").strip().splitlines()
    for line in reversed(txt):
        try:
            return json.loads(line)
        except Exception:  # noqa: BLE001
            continue
    return None


def series(seconds, label):
    print("серия «%s»: %d с" % (label, seconds))
    out = []
    t0 = time.time()
    while time.time() - t0 < seconds:
        s = sample()
        if s:
            out.append(s)
            print("  %s VRAM=%.0f | %s" % (s["ts"], s["vram_used_mib"],
                  " ".join("%s ws=%.0f/wspriv=%.0f/priv=%.0f"
                           % (p["role"], p["ws_mb"], p["ws_priv_mb"], p["private_mb"])
                           for p in s["procs"])))
        time.sleep(20)
    return out


def main():
    os.makedirs(OUT, exist_ok=True)
    res = {"started": time.strftime("%Y-%m-%d %H:%M:%S")}
    res["A_now"] = series(120, "текущее состояние (индекс-сессия на паузе)")

    # проверка: пускает ли чужая индекс-сессия отдельный watcher
    env = {**os.environ, "HDS_CONFIG": CFG}
    p = subprocess.Popen([PY, "-m", "hds.cli", "watch"], cwd=ROOT,
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
    time.sleep(20)
    alive = p.poll() is None
    tail = ""
    if not alive:
        tail = (p.stdout.read() or b"").decode("utf-8", "replace")[-400:]
    res["watcher_started"] = {"alive_after_20s": alive, "output_tail": tail}
    print("\nwatcher на изолированном корне: жив=%s" % alive)
    if tail:
        print("  вывод: %s" % tail.replace("\n", " | ")[:300])
    if alive:
        res["A_watcher_idle"] = series(60, "watcher на изолированном корне, простой")
        p.terminate()
        try:
            p.wait(timeout=20)
        except subprocess.TimeoutExpired:
            p.kill()
    with open(os.path.join(OUT, "measure_live.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/measure_live.json")


if __name__ == "__main__":
    main()