"""Диагностика GPU-пути движка (спайк 6в для W2): почему инференс идёт на CPU.

1) chat с явными `--n-gpu-layers 99 --n-ctx 8192`;
2) whisper с `--whisper-no-fallback` (если GPU падает — увидим настоящую ошибку CUDA,
   а не тихий откат на CPU).

Пишет out/spike6_gpu_diag.json с ключевыми строками stderr/stdout.
"""
import json
import os
import re
import subprocess
import threading
import time

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "out")
ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline", "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")
CHAT = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__Qwen3.5-9B-GGUF", "Qwen3.5-9B-Q4_K_M.gguf")
TURBO = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__whisper-large-v3-turbo-GGML",
                    "whisper-large-v3-turbo-GGML.bin")
CLIP = os.path.join(OUT, "ru_clip", "clip_300_360.wav")
INTERESTING = re.compile(
    r"offloaded|CUDA|cuda|device|fallback|no kernel|out of memory|OOM|error|failed|"
    r"backend|n_gpu_layers|GPU", re.IGNORECASE)


def vram():
    r = subprocess.run(["nvidia-smi", "--query-gpu=memory.used",
                        "--format=csv,noheader,nounits"], capture_output=True, timeout=30)
    return float(r.stdout.decode().strip())


class S(threading.Thread):
    def __init__(self):
        super().__init__(daemon=True)
        self.stop = threading.Event()
        self.v = []

    def run(self):
        while not self.stop.is_set():
            self.v.append(vram())
            self.stop.wait(1)

    def done(self):
        self.stop.set()
        self.join(timeout=8)
        return max(self.v) if self.v else None


def run(label, args, timeout=240):
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    base = vram()
    s = S()
    s.start()
    p = subprocess.Popen([CLI] + args, cwd=ENGINE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, env=env)
    t0 = time.time()
    try:
        out_b, err_b = p.communicate(timeout=timeout)
        rc = p.returncode
    except subprocess.TimeoutExpired:
        p.kill()
        out_b, err_b = p.communicate()
        rc = "TIMEOUT"
    dt = time.time() - t0
    peak = s.done()
    txt = (out_b or b"").decode("utf-8", "replace")
    err = (err_b or b"").decode("utf-8", "replace")
    keys = [ln.strip() for ln in err.splitlines() if INTERESTING.search(ln)][:20]
    summary = re.search(r"\w+_summary[^\n]*", txt)
    time.sleep(3)
    return {"label": label, "rc": rc, "sec": round(dt, 1),
            "vram_base": base, "vram_peak": peak,
            "vram_delta": round((peak or base) - base, 0),
            "stderr_key_lines": keys,
            "summary": summary.group(0) if summary else "",
            "stdout_tail": txt[-500:]}


def main():
    res = {"baseline_vram": vram()}
    if not os.path.exists(CLIP):
        raise SystemExit("нет клипа %s" % CLIP)
    res["whisper_cpu_default"] = run("whisper по умолчанию", [
        "bridge", "audio", "--audio-file", CLIP, "--mode", "speech", "--custom", "default",
        "--whisper-model", TURBO, "--output-dir", os.path.join(OUT, "whisper_gpu_t")],
        timeout=300)
    res["whisper_gpu_device"] = run("whisper --whisper-gpu-device 0", [
        "bridge", "audio", "--audio-file", CLIP, "--mode", "speech", "--custom", "default",
        "--whisper-model", TURBO, "--whisper-gpu-device", "0",
        "--output-dir", os.path.join(OUT, "whisper_gpu_t")], timeout=300)
    for k in ("whisper_cpu_default", "whisper_gpu_device"):
        v = res[k]
        print("%-28s %6s с | VRAM+%-6s | rc=%s | %s"
              % (v["label"], v["sec"], v["vram_delta"], v["rc"], v["summary"][:70]))
    with open(os.path.join(OUT, "spike5_gpu.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike5_gpu.json")


if __name__ == "__main__":
    main()