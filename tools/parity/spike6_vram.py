"""Спайк 6(в) и спайк 5 (GPU): VRAM при загрузке/выгрузке моделей движком.

В окне свободной VRAM (chat/embedding остановлены) измеряем:
  1) baseline VRAM;
  2) `bridge embed` (bge-m3, ~1,2 ГБ) → пик и освобождение после выхода процесса;
  3) `bridge chat` с reasoning="off" (Q4_K_M, ~5,5 ГБ) → пик, освобождение, наличие
     блоков размышлений в ответе (паритет с enable_thinking=false);
  4) `bridge audio` (whisper turbo) на 60-с русском клипе → время и пик VRAM.

Результат — out/spike6_vram.json.
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
LLAMA = os.path.join(os.environ["LOCALAPPDATA"], "llama-runtime", "models")
BGE = os.path.join(LLAMA, "embedding", "bge-m3-Q8_0.gguf")
CHAT = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__Qwen3.5-9B-GGUF", "Qwen3.5-9B-Q4_K_M.gguf")
TURBO = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__whisper-large-v3-turbo-GGML",
                    "whisper-large-v3-turbo-GGML.bin")
CLIP = os.path.join(OUT, "ru_clip", "clip_300_360.wav")


def vram():
    r = subprocess.run(["nvidia-smi", "--query-gpu=memory.used,memory.free",
                        "--format=csv,noheader,nounits"], capture_output=True, timeout=30)
    used, free = [float(x) for x in r.stdout.decode().strip().split(",")]
    return {"used": used, "free": free}


def proc_stats(pid):
    """CPU-время (с) и RSS процесса — чтобы отличить GPU-инференс от CPU."""
    import ctypes

    class FT(ctypes.Structure):
        _fields_ = [("lo", ctypes.c_ulong), ("hi", ctypes.c_ulong)]

    class PMC(ctypes.Structure):
        _fields_ = [("cb", ctypes.c_ulong), ("PageFaultCount", ctypes.c_ulong),
                    ("PeakWorkingSetSize", ctypes.c_size_t),
                    ("WorkingSetSize", ctypes.c_size_t),
                    ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                    ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                    ("PagefileUsage", ctypes.c_size_t),
                    ("PeakPagefileUsage", ctypes.c_size_t)]
    PROCESS_QUERY_LIMITED = 0x1000
    h = ctypes.windll.kernel32.OpenProcess(PROCESS_QUERY_LIMITED, False, pid)
    if not h:
        return None
    c, e, k, u = FT(), FT(), FT(), FT()
    cpu = 0.0
    if ctypes.windll.kernel32.GetProcessTimes(h, ctypes.byref(c), ctypes.byref(e),
                                              ctypes.byref(k), ctypes.byref(u)):
        cpu = ((k.hi << 32) | k.lo + 0) / 1e7 + ((u.hi << 32) | u.lo) / 1e7
    pmc = PMC()
    pmc.cb = ctypes.sizeof(PMC)
    rss = 0.0
    if ctypes.windll.psapi.GetProcessMemoryInfo(h, ctypes.byref(pmc), pmc.cb):
        rss = pmc.WorkingSetSize / 1048576
    ctypes.windll.kernel32.CloseHandle(h)
    return {"cpu_sec": round(cpu, 1), "rss_mb": round(rss, 1)}


class Sampler(threading.Thread):
    """Сэмплирование VRAM (0,5 с) + CPU/RSS процесса движка (1 с)."""

    def __init__(self, pid=None):
        super().__init__(daemon=True)
        self.stop = threading.Event()
        self.pid = pid
        self.vram = []
        self.cpu = []
        self.rss = []
        self._n = 0

    def run(self):
        while not self.stop.is_set():
            self.vram.append(vram()["used"])
            if self.pid and self._n % 2 == 0:
                st = proc_stats(self.pid)
                if st:
                    self.cpu.append(st["cpu_sec"])
                    self.rss.append(st["rss_mb"])
            self._n += 1
            self.stop.wait(0.5)

    def summary(self):
        return {"vram_samples": len(self.vram),
                "vram_peak": max(self.vram) if self.vram else None,
                "cpu_sec_peak": max(self.cpu) if self.cpu else None,
                "rss_mb_peak": max(self.rss) if self.rss else None}


def run_engine(label, args, timeout=3600):
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    before = vram()
    proc = subprocess.Popen([CLI] + args, cwd=ENGINE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, env=env)
    s = Sampler(pid=proc.pid)
    s.start()
    t0 = time.time()
    try:
        out_b, err_b = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        out_b, err_b = b"", b"TIMEOUT"
    dt = time.time() - t0
    s.stop.set()
    s.join(timeout=10)
    time.sleep(3)                       # дать драйверу вернуть память
    after = vram()
    summ = s.summary()
    return {"label": label, "rc": proc.returncode, "sec": round(dt, 1),
            "vram_before_used": before["used"], "vram_after_used": after["used"],
            "peak_delta_mib": round((summ["vram_peak"] or before["used"]) - before["used"], 0),
            "released_mib": round(after["used"] - before["used"], 0),
            "cpu_sec": summ["cpu_sec_peak"], "rss_mb_peak": summ["rss_mb_peak"],
            "cpu_ratio": round((summ["cpu_sec_peak"] or 0) / max(dt, 0.1), 2),
            "stderr_tail": (err_b or b"").decode("utf-8", "replace")[-400:],
            "stdout_tail": (out_b or b"").decode("utf-8", "replace")[-900:]}
def json_from(text):
    start = text.find("{")
    while start != -1:
        try:
            obj, _ = json.JSONDecoder().raw_decode(text[start:])
            return obj
        except ValueError:
            start = text.find("{", start + 1)
    return None


def main():
    os.makedirs(OUT, exist_ok=True)
    res = {"baseline": vram(), "generated": time.strftime("%Y-%m-%d %H:%M:%S")}
    print("baseline: %s" % res["baseline"])

    res["embed"] = run_engine("embed bge-m3", [
        "bridge", "embed", "--model", BGE, "--body-json",
        json.dumps({"input": ["проверка VRAM %d" % i for i in range(8)]},
                   ensure_ascii=False)])
    print("embed:  пик +%.0f МиБ, после выхода used=%.0f (нетто %+.0f МиБ)"
          % (res["embed"]["peak_delta_mib"], res["embed"]["vram_after_used"],
             res["embed"]["released_mib"]))

    prompt = "Ответь одним словом: столица России?"
    res["chat"] = run_engine("chat reasoning=off", [
        "bridge", "chat", "--model", CHAT, "--prompt", prompt, "--body-json",
        json.dumps({"n_predict": 48, "temperature": 0.2, "reasoning": "off"},
                   ensure_ascii=False)])
    obj = json_from(res["chat"]["stdout_tail"] or "") or {}
    text = ""
    if isinstance(obj, dict):
        text = (obj.get("choices") or [{}])[0].get("text") or obj.get("content") or ""
        text = text or str(obj.get("response") or "")
    res["chat"]["answer"] = text[:300]
    res["chat"]["has_thinking_tags"] = bool(re.search(
        r"<think|Thinking Process|思考|分析", res["chat"]["stdout_tail"] or ""))
    print("chat:   пик +%.0f МиБ, нетто %+.0f МиБ, ответ=%r, теги размышлений=%s"
          % (res["chat"]["peak_delta_mib"], res["chat"]["released_mib"], text[:80],
             res["chat"]["has_thinking_tags"]))

    if os.path.exists(CLIP):
        res["whisper"] = run_engine("whisper turbo (GPU, свободная VRAM)", [
            "bridge", "audio", "--audio-file", CLIP, "--mode", "speech",
            "--custom", "default", "--whisper-model", TURBO,
            "--output-dir", os.path.join(OUT, "whisper_free_vram")])
        segs = re.findall(r"\[\d\d:\d\d:\d\d\.\d+ --> \d\d:\d\d:\d\d\.\d+\]",
                          res["whisper"]["stdout_tail"] or "")
        st = re.search(r'"stats":\{[^}]*\}', res["whisper"]["stdout_tail"] or "")
        res["whisper"]["segments"] = len(segs)
        res["whisper"]["stats"] = st.group(0) if st else ""
        res["whisper"]["text"] = ((re.search(r"\]\s+(.+)", res["whisper"]["stdout_tail"] or "")
                                   or [None, ""])[1] or "")[:200]
        print("whisper: %.1f с на 60 с аудио, пик +%.0f МиБ, сегментов %d, %s"
              % (res["whisper"]["sec"], res["whisper"]["peak_delta_mib"],
                 res["whisper"]["segments"], res["whisper"]["stats"]))

    with open(os.path.join(OUT, "spike6_vram.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike6_vram.json")


if __name__ == "__main__":
    main()