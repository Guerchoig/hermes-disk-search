"""Спайк 5 (продолжение): паритет «движок vs faster-whisper» + VRAM.

Замеряет текст и время обоих путей на одном файле (jfk.wav с известным эталоном),
WER (пословный edit distance), VRAM с сэмплированием во время прогона движка
(укладывается ли turbo в бюджет 12 ГБ, есть ли пейджинг) и русскую речь.
"""
import difflib
import json
import os
import re
import subprocess
import sys
import threading
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
sys.path.insert(0, ROOT)

ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline", "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")
TURBO = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__whisper-large-v3-turbo-GGML",
                    "whisper-large-v3-turbo-GGML.bin")
JFK = os.path.join(ROOT, "test_data", "jfk.wav")
RU = os.path.join(ROOT, "test_data", "speech_2min.wav")
REF_JFK = ("and so my fellow americans ask not what your country can do for you "
           "ask what you can do for your country")


def vram_used():
    try:
        r = subprocess.run(["nvidia-smi", "--query-gpu=memory.used",
                            "--format=csv,noheader,nounits"], capture_output=True, timeout=20)
        return float(r.stdout.decode().strip())
    except Exception:  # noqa: BLE001
        return -1.0


class Sampler(threading.Thread):
    """Сэмплирование занятой VRAM каждые 2 с (пик и профиль)."""

    def __init__(self):
        super().__init__(daemon=True)
        self.stop = threading.Event()
        self.samples = []

    def run(self):
        while not self.stop.is_set():
            self.samples.append(vram_used())
            self.stop.wait(2)


def engine_transcribe(audio, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    s = Sampler()
    s.start()
    t0 = time.time()
    r = subprocess.run([CLI, "bridge", "audio", "--audio-file", audio, "--mode", "speech",
                        "--custom", "default", "--whisper-model", TURBO,
                        "--output-dir", out_dir], cwd=ENGINE, capture_output=True,
                       timeout=3600, env=env)
    dt = time.time() - t0
    s.stop.set()
    s.join(timeout=5)
    out = (r.stdout or b"").decode("utf-8", "replace")
    text = ""
    for f in os.listdir(out_dir):
        if f.lower().endswith(".md"):
            text = open(os.path.join(out_dir, f), encoding="utf-8",
                        errors="replace").read().strip()
    segs = re.findall(r"\[\d\d:\d\d:\d\d\.\d+ --> \d\d:\d\d:\d\d\.\d+\]", out)
    return {"sec": round(dt, 1), "rc": r.returncode, "text": text,
            "segments": len(segs), "vram_peak_mib": max(s.samples) if s.samples else None,
            "vram_min_mib": min(s.samples) if s.samples else None}


def faster_whisper_transcribe(audio):
    """Текущий путь проекта: hds.extractors.extract → extract_av (faster-whisper)."""
    from hds.config import load
    from hds import extractors

    cfg = load(os.path.join(ROOT, "config.yaml"))
    s = Sampler()
    s.start()
    t0 = time.time()
    kind, segs = extractors.extract(audio, cfg)
    dt = time.time() - t0
    s.stop.set()
    s.join(timeout=5)
    text = " ".join((sg.get("text") or "").strip() for sg in segs).strip()
    return {"sec": round(dt, 1), "kind": kind, "text": text, "segments": len(segs),
            "vram_peak_mib": max(s.samples) if s.samples else None}


def wer(ref, hyp):
    a = ref.lower().split()
    b = hyp.lower().split()
    sm = difflib.SequenceMatcher(None, a, b)
    same = sum(bl.size for bl in sm.get_matching_blocks())
    return round((len(a) - same) / max(len(a), 1), 4)
def main():
    os.makedirs(OUT, exist_ok=True)
    res = {"engine_whisper_model": os.path.basename(TURBO),
           "vram_used_before": vram_used()}
    print("VRAM до опытов: %.0f МиБ занято" % res["vram_used_before"])

    print("[1/4] движок: jfk.wav")
    res["engine_jfk"] = engine_transcribe(JFK, os.path.join(OUT, "whisper_cmp_eng_jfk"))
    print("   %.1f с, сегментов=%s, VRAM пик=%s (мин=%s)"
          % (res["engine_jfk"]["sec"], res["engine_jfk"]["segments"],
             res["engine_jfk"]["vram_peak_mib"], res["engine_jfk"]["vram_min_mib"]))
    print("   текст: %s" % res["engine_jfk"]["text"][:200])

    print("[2/4] faster-whisper (текущий путь): jfk.wav")
    res["faster_jfk"] = faster_whisper_transcribe(JFK)
    print("   %.1f с, сегментов=%s, VRAM пик=%s" % (res["faster_jfk"]["sec"],
          res["faster_jfk"]["segments"], res["faster_jfk"]["vram_peak_mib"]))
    print("   текст: %s" % res["faster_jfk"]["text"][:200])

    res["wer_engine_jfk"] = wer(REF_JFK, res["engine_jfk"]["text"])
    res["wer_faster_jfk"] = wer(REF_JFK, res["faster_jfk"]["text"])
    print("WER (эталон jfk): движок=%.4f, faster-whisper=%.4f"
          % (res["wer_engine_jfk"], res["wer_faster_jfk"]))

    print("[3/4] движок: русская речь (~2 мин)")
    res["engine_ru"] = engine_transcribe(RU, os.path.join(OUT, "whisper_cmp_eng_ru"))
    print("   %.1f с, сегментов=%s" % (res["engine_ru"]["sec"], res["engine_ru"]["segments"]))
    print("   текст: %s" % res["engine_ru"]["text"][:300])

    print("[4/4] faster-whisper: русская речь")
    res["faster_ru"] = faster_whisper_transcribe(RU)
    print("   %.1f с, сегментов=%s" % (res["faster_ru"]["sec"], res["faster_ru"]["segments"]))
    print("   текст: %s" % res["faster_ru"]["text"][:300])
    res["wer_engine_vs_faster_ru"] = wer(res["faster_ru"]["text"], res["engine_ru"]["text"])
    print("WER(движок против faster-whisper, рус): %.4f" % res["wer_engine_vs_faster_ru"])

    with open(os.path.join(OUT, "spike5_compare.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike5_compare.json")


if __name__ == "__main__":
    main()