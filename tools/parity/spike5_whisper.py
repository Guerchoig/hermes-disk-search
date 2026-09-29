"""Спайк 5 (W0 §4 п.7): транскрибация через движок openresearchtools/engine.

Проверяет главное для W3:
  * получаются ли сегменты с таймкодами (JSON/speech/subtitle на выходе);
  * обязателен ли ASCII-стейджинг путей (русский путь/имя файла против ASCII);
  * GPU-режим и поведение при нехватке VRAM (на машине занято ~11,2 из 12,3 ГБ);
  * время транскрибации.

Часть 1 (эта): «разведочный» прогон — печать всего, что печатает CLI, и списка
созданных файлов. Сравнение с faster-whisper — следующий шаг (spike5_compare.py).
"""
import os
import shutil
import subprocess
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
ENGINE = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "TranscribeOffline", "Engine")
CLI = os.path.join(ENGINE, "example-cli.exe")
TURBO = os.path.join(os.environ["APPDATA"], "OpenResearchTools", "models",
                    "openresearchtools__whisper-large-v3-turbo-GGML",
                    "whisper-large-v3-turbo-GGML.bin")
JFK = os.path.join(ROOT, "test_data", "jfk.wav")


def run(label, audio_path, extra_args, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    before = sorted(os.listdir(out_dir))
    args = [CLI, "bridge", "audio", "--audio-file", audio_path, "--mode", "speech",
            "--custom", "default", "--whisper-model", TURBO,
            "--output-dir", out_dir] + extra_args
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    t0 = time.time()
    print("=== %s ===" % label)
    print("args: %s" % " ".join(args[1:]))
    try:
        r = subprocess.run(args, cwd=ENGINE, capture_output=True, timeout=1800, env=env)
        rc, out, err = r.returncode, (r.stdout or b"").decode("utf-8", "replace"), \
            (r.stderr or b"").decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        rc, out, err = "TIMEOUT", "", ""
    dt = time.time() - t0
    print("rc=%s, %.1f с" % (rc, dt))
    print("--- stdout (первые 2500 симв.) ---")
    print(out[:2500])
    print("--- stderr (хвост 1500) ---")
    print(err[-1500:])
    after = sorted(os.listdir(out_dir))
    new = [f for f in after if f not in before]
    print("--- новые файлы в output_dir (%d): %s" % (len(new), new))
    for f in new[:5]:
        p = os.path.join(out_dir, f)
        try:
            head = open(p, encoding="utf-8", errors="replace").read()[:700]
            print("  [%s] %s" % (f, head.replace("\n", " | ")[:500]))
        except Exception as e:  # noqa: BLE001
            print("  [%s] не читается: %s" % (f, e))
    print()
    return {"rc": rc, "sec": round(dt, 1), "new_files": new,
            "stdout_tail": out[-1500:], "stderr_tail": err[-1500:]}


def main():
    os.makedirs(OUT, exist_ok=True)
    # контроль: чистый ASCII-путь
    ascii_dir = os.path.join(OUT, "whisper_ascii")
    ascii_wav = os.path.join(ascii_dir, "jfk.wav")
    os.makedirs(ascii_dir, exist_ok=True)
    shutil.copy2(JFK, ascii_wav)
    # боевой случай: русский каталог + русское имя файла
    ru_dir = os.path.join(OUT, "речь_тест")
    ru_wav = os.path.join(ru_dir, "дорога_домой.wav")
    os.makedirs(ru_dir, exist_ok=True)
    shutil.copy2(JFK, ru_wav)

    res = {"engine": ENGINE, "whisper_model_mb": round(os.path.getsize(TURBO) / 1048576, 1)}
    res["ascii"] = run("ASCII-путь (контроль)", ascii_wav, [],
                       os.path.join(OUT, "whisper_out_ascii"))
    res["non_ascii"] = run("Русский путь и имя файла", ru_wav, [],
                           os.path.join(OUT, "whisper_out_ru"))
    import json
    with open(os.path.join(OUT, "spike5_whisper.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike5_whisper.json")


if __name__ == "__main__":
    main()