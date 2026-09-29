"""Спайк 6(б): сверка memory_free движка с nvidia-smi в один момент времени.

Критерий §8.6.1: значения `llama_server_cluster_list_devices()` должны совпадать
с реальной свободной VRAM (±5 %). Скрипт делает замеры подряд (секунды) и печатает
расхождение, дублируя вывод в out/spike6_devices.json.
"""
import json
import os
import re
import subprocess
import time

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "out")
ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline",
                      "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")


def nvidia():
    try:
        r = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used,memory.total,memory.free",
             "--format=csv,noheader,nounits"],
            capture_output=True, timeout=30)
        used, total, free = [float(x) for x in r.stdout.decode().strip().split(",")]
        return {"used_mib": used, "total_mib": total, "free_mib": free}
    except Exception as e:  # noqa: BLE001
        return {"error": str(e)}


def engine_devices():
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    r = subprocess.run([CLI, "list-devices"], cwd=ENGINE, capture_output=True,
                       timeout=300, env=env)
    out = (r.stdout or b"").decode("utf-8", "replace")
    devs = []
    for ln in out.splitlines():
        m = re.match(r"device index=(\d+) backend=(\w+) name=(\S+) desc=(.*?) "
                     r"type=(\d+) free_mib=([\d.]+) total_mib=([\d.]+)", ln.strip())
        if m:
            devs.append({"index": int(m.group(1)), "backend": m.group(2),
                         "name": m.group(3), "desc": m.group(4).strip(),
                         "type": int(m.group(5)),
                         "free_mib": float(m.group(6)), "total_mib": float(m.group(7))})
    return {"rc": r.returncode, "raw": out.strip(), "devices": devs}


def main():
    os.makedirs(OUT, exist_ok=True)
    t0 = time.time()
    nv_before = nvidia()
    ed = engine_devices()
    nv_after = nvidia()
    dt = time.time() - t0
    print("t=%.2f c" % dt)
    print("nvidia ДО:    used=%.0f free=%.0f total=%.0f MiB"
          % (nv_before["used_mib"], nv_before["free_mib"], nv_before["total_mib"]))
    print("nvidia ПОСЛЕ: used=%.0f free=%.0f total=%.0f MiB"
          % (nv_after["used_mib"], nv_after["free_mib"], nv_after["total_mib"]))
    nv = nv_before
    print("движок:       %s" % json.dumps(ed["devices"], ensure_ascii=False))
    res = {"measured_span_sec": round(dt, 2), "nvidia_before": nv_before,
           "nvidia_after": nv_after, "engine": ed}
    cuda = next((d for d in ed["devices"] if d["backend"] == "CUDA"), None)
    if cuda and "free_mib" in nv:
        drift = nv_after["free_mib"] - nv_before["free_mib"]
        delta = cuda["free_mib"] - nv_before["free_mib"]
        pct = abs(delta) / max(nv["free_mib"], 1) * 100
        res["nvidia_free_drift_mib"] = drift
        res["engine_minus_nvidia_mib"] = round(delta, 1)
        res["total_delta_mib"] = round(cuda["total_mib"] - nv["total_mib"], 1)
        res["verdict"] = ("совпадает (±5 %)" if pct <= 5 else
                          "метрика free движка НЕ равна реальной свободной VRAM")
        print("дрейф nvidia между замерами: %+.0f МиБ (если ~0 — гонка по времени исключена)"
              % drift)
        print("движок минус nvidia (free): %+.0f МиБ (%.1f %% от свободной); total: %+.1f МиБ"
              % (delta, pct, res["total_delta_mib"]))
        print("ВЕРДИКТ: %s" % res["verdict"])
    with open(os.path.join(OUT, "spike6_devices.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()