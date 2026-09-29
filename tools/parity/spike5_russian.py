"""Спайк 5 (русский паритет): движок против текущего транскрипта из БД (W0 §4 п.7).

Эталон = текст, который уже лежит в боевой index.db (faster-whisper small).
Вход = вырезанный сегмент того же файла. Критерий W3: WER <= 5 %.
"""
import difflib
import json
import os
import shutil
import sqlite3
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
DB = r"D:\hermes-disk-search-db\index.db"
SRC = r"D:\НИНА\Documents\uvSC_Projects\Project007\Sound_161124190550_001_000.wav"
T0, T1 = 300, 360          # вырезаем 60 с (в индексе есть транскрипт этого участка)
FFMPEG = shutil.which("ffmpeg")


def ref_text():
    """Текст из боевой БД за интервал [T0, T1] — эталон «как сейчас»."""
    uri = "file:%s?mode=ro" % DB.replace("\\", "/")
    conn = sqlite3.connect(uri, uri=True)
    conn.row_factory = sqlite3.Row
    rows = conn.execute(
        "SELECT c.t_start, c.t_end, c.text FROM chunks c JOIN files f ON f.id=c.file_id "
        "WHERE f.path = ? AND c.t_start < ? AND c.t_end > ? ORDER BY c.t_start",
        (SRC, T1, T0)).fetchall()
    conn.close()
    return " ".join(r["text"].replace("\n", " ").strip() for r in rows), len(rows)


def cut_ascii():
    """ASCII-стейджинг: движок портит имена выходных файлов на не-ASCII пути."""
    d = os.path.join(OUT, "ru_clip")
    os.makedirs(d, exist_ok=True)
    dst = os.path.join(d, "clip_300_360.wav")
    subprocess.run([FFMPEG, "-y", "-ss", str(T0), "-t", str(T1 - T0), "-i", SRC,
                    "-ar", "16000", "-ac", "1", dst], capture_output=True, timeout=600)
    return dst


def engine_transcribe(audio, out_dir):
    os.makedirs(out_dir, exist_ok=True)
    env = dict(os.environ)
    env["PATH"] = ENGINE + os.pathsep + env["PATH"]
    t0 = time.time()
    r = subprocess.run([CLI, "bridge", "audio", "--audio-file", audio, "--mode", "speech",
                        "--custom", "default", "--whisper-model", TURBO,
                        "--output-dir", out_dir], cwd=ENGINE, capture_output=True,
                       timeout=3600, env=env)
    dt = time.time() - t0
    out = (r.stdout or b"").decode("utf-8", "replace")
    text = ""
    for f in os.listdir(out_dir):
        if f.lower().endswith(".md"):
            text = open(os.path.join(out_dir, f), encoding="utf-8",
                        errors="replace").read().strip()
    return {"sec": round(dt, 1), "rc": r.returncode, "text": text, "raw_tail": out[-800:]}


def wer(ref, hyp):
    a, b = ref.lower().split(), hyp.lower().split()
    same = sum(bl.size for bl in difflib.SequenceMatcher(None, a, b).get_matching_blocks())
    return round((len(a) - same) / max(len(a), 1), 4)


def main():
    os.makedirs(OUT, exist_ok=True)
    res = {"source": SRC, "interval_sec": [T0, T1], "engine_model": os.path.basename(TURBO)}
    ref, n_chunks = ref_text()
    res["reference_chunks"] = n_chunks
    res["reference_words"] = len(ref.split())
    print("эталон из БД: %d чанков, %d слов" % (n_chunks, res["reference_words"]))
    print("эталон (начало): %s" % ref[:300])
    if n_chunks == 0:
        print("ВНИМАНИЕ: в интервале нет чанков — сравнение невозможно")
    audio = cut_ascii()
    res["engine"] = engine_transcribe(audio, os.path.join(OUT, "ru_clip_out"))
    print("движок: %.1f с, rc=%s" % (res["engine"]["sec"], res["engine"]["rc"]))
    print("движок (начало): %s" % res["engine"]["text"][:300])
    res["wer_engine_vs_db"] = wer(ref, res["engine"]["text"])
    res["reference_text"] = ref
    print("WER (движок против текущего индекса): %.4f -> %s"
          % (res["wer_engine_vs_db"],
             "КРИТЕРИЙ W3 ВЫПОЛНЕН (<=5%)" if res["wer_engine_vs_db"] <= 0.05
             else "выше порога 5% (движок другой модели: turbo vs small)"))
    with open(os.path.join(OUT, "spike5_russian.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike5_russian.json")


if __name__ == "__main__":
    main()