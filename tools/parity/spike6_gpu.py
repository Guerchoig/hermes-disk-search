"""Решающий тест для W2: использует ли движок GPU при явном n_gpu_layers?

Сравниваем два прогона `bridge chat` на одной модели и промпте:
  A) body-json без n_gpu_layers (как по умолчанию у example-cli) — ожидаем CPU;
  B) body-json с n_gpu_layers=99 — ожидаем GPU (рост VRAM, скорость выше в разы).

Плюс прогон embeddings с n_gpu_layers=99 (проверка, что паритет сохраняется и на GPU).
Метрики: VRAM пик, CPU-доля процесса, output_tps из summary, время загрузки.
"""
import json
import os
import re
import subprocess
import threading
import time
import ctypes

BASE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(BASE, "out")
ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline", "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")
LLAMA = os.path.join(os.environ["LOCALAPPDATA"], "llama-runtime", "models")
BGE = os.path.join(LLAMA, "embedding", "bge-m3-Q8_0.gguf")
CHAT = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__Qwen3.5-9B-GGUF", "Qwen3.5-9B-Q4_K_M.gguf")
PROMPT = "Ответь одним словом: столица России?"


def vram():
    r = subprocess.run(["nvidia-smi", "--query-gpu=memory.used",
                        "--format=csv,noheader,nounits"], capture_output=True, timeout=30)
    return float(r.stdout.decode().strip())


def proc_cpu(pid):
    class FT(ctypes.Structure):
        _fields_ = [("lo", ctypes.c_ulong), ("hi", ctypes.c_ulong)]
    h = ctypes.windll.kernel32.OpenProcess(0x1000, False, pid)
    if not h:
        return 0.0
    c, e, k, u = FT(), FT(), FT(), FT()
    cpu = 0.0
    if ctypes.windll.kernel32.GetProcessTimes(h, ctypes.byref(c), ctypes.byref(e),
                                              ctypes.byref(k), ctypes.byref(u)):
        cpu = ((k.hi << 32) | k.lo) / 1e7 + ((u.hi << 32) | u.lo) / 1e7
    ctypes.windll.kernel32.CloseHandle(h)
    return cpu


class S(threading.Thread):
    def __init__(self, pid):
        super().__init__(daemon=True)
        self.stop = threading.Event()
        self.pid = pid
        self.vram, self.cpu = [], []

    def run(self):
        while not self.stop.is_set():
            self.vram.append(vram())
            self.cpu.append(proc_cpu(self.pid))
            self.stop.wait(1)

    def done(self):
        self.stop.set()
        self.join(timeout=10)
        return {"vram_peak": max(self.vram) if self.vram else None,
                "cpu_peak": round(max(self.cpu), 1) if self.cpu else None}


def run(label, args, timeout=300):
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    base = vram()
    p = subprocess.Popen([CLI] + args, cwd=ENGINE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, env=env)
    s = S(p.pid)
    s.start()
    t0 = time.time()
    try:
        out_b, err_b = p.communicate(timeout=timeout)
        rc = p.returncode
    except subprocess.TimeoutExpired:
        p.kill()
        out_b, err_b = p.communicate()
        rc = "TIMEOUT"
    dt = time.time() - t0
    st = s.done()
    txt = (out_b or b"").decode("utf-8", "replace")
    err = (err_b or b"").decode("utf-8", "replace")
    summary = re.search(r"(chat_summary|embed_summary)[^\n]*", txt)
    gpu = re.search(r"offloaded (\d+)/(\d+) layers to GPU", err) or \
        re.search(r"CUDA0 model buffer size[^\n]*", err)
    time.sleep(3)
    return {"label": label, "rc": rc, "sec": round(dt, 1),
            "vram_base": base, "vram_peak": st["vram_peak"],
            "vram_peak_delta": round((st["vram_peak"] or base) - base, 0),
            "cpu_sec": st["cpu_peak"], "cpu_ratio": round((st["cpu_peak"] or 0) / max(dt, .1), 2),
            "summary": summary.group(0) if summary else "",
            "gpu_hint": gpu.group(0) if gpu else "",
            "stdout_tail": txt[-400:], "stderr_tail": err[-400:]}


def main():
    res = {"baseline": vram()}
    print("baseline VRAM used = %.0f МиБ" % res["baseline"])
    body_cpu = {"n_predict": 16, "n_ctx": 8192, "temperature": 0.2, "reasoning": "off"}
    body_gpu = dict(body_cpu, n_gpu_layers=99)
    res["chat_cpu"] = run("chat без n_gpu_layers", ["bridge", "chat", "--model", CHAT,
                                                    "--prompt", PROMPT, "--body-json",
                                                    json.dumps(body_cpu)])
    res["chat_gpu"] = run("chat с n_gpu_layers=99", ["bridge", "chat", "--model", CHAT,
                                                      "--prompt", PROMPT, "--body-json",
                                                      json.dumps(body_gpu)])
    for k in ("chat_cpu", "chat_gpu"):
        v = res[k]
        print("%-22s %.1f с | CPU-доля %.2f | VRAM+%.0f | %s | %s"
              % (v["label"], v["sec"], v["cpu_ratio"], v["vram_peak_delta"],
                 v["gpu_hint"][:40], v["summary"][:90]))
    res["embed_gpu"] = run("embed n_gpu_layers=99", ["bridge", "embed", "--model", BGE,
                                                     "--body-json", json.dumps(
                                                         {"input": ["тест GPU эмбеддинг"],
                                                          "n_gpu_layers": 99},
                                                         ensure_ascii=False)])
    print("embed GPU: %.1f с, CPU-доля %.2f, VRAM+%.0f"
          % (res["embed_gpu"]["sec"], res["embed_gpu"]["cpu_ratio"],
             res["embed_gpu"]["vram_peak_delta"]))
    with open(os.path.join(OUT, "spike6_gpu.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike6_gpu.json")


if __name__ == "__main__":
    main()