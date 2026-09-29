"""Решающий тест спайка 5: три текста на ОДНОМ клипе (движок / faster-whisper / БД).

Клип = [300..360] с из боевого файла. Сравниваем:
  * движок (large-v3-turbo),
  * текущий путь (faster-whisper small),
  * текст из индекса (эталон «как сейчас»).
Если faster-whisper и текст из индекса совпадают, а движок — нет, значит движок
транскрибирует иначе (или таймкоды БД съехали) — и это надо знать до W3.
"""
import difflib
import json
import os
import sqlite3
import sys

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
sys.path.insert(0, ROOT)

DB = r"D:\hermes-disk-search-db\index.db"
SRC = r"D:\НИНА\Documents\uvSC_Projects\Project007\Sound_161124190550_001_000.wav"
CLIP = os.path.join(OUT, "ru_clip", "clip_300_360.wav")


def wer(ref, hyp):
    a, b = ref.lower().split(), hyp.lower().split()
    same = sum(bl.size for bl in difflib.SequenceMatcher(None, a, b).get_matching_blocks())
    return round((len(a) - same) / max(len(a), 1), 4)


def db_window(t0, t1):
    conn = sqlite3.connect("file:%s?mode=ro" % DB.replace("\\", "/"), uri=True)
    rows = conn.execute(
        "SELECT c.t_start, c.t_end, c.text FROM chunks c JOIN files f ON f.id=c.file_id "
        "WHERE f.path=? AND c.t_end > ? AND c.t_start < ? ORDER BY c.t_start",
        (SRC, t0, t1)).fetchall()
    conn.close()
    return " ".join(r[2].replace("\n", " ").strip() for r in rows)


def main():
    res = {"clip": CLIP, "window_sec": [300, 360]}
    ref_db = db_window(300, 360)
    res["db_text"] = ref_db
    print("БД (окно 300-360): %s...\n" % ref_db[:220])

    from hds.config import load
    from hds import extractors
    cfg = load(os.path.join(ROOT, "config.yaml"))
    kind, segs = extractors.extract(CLIP, cfg)
    fw = " ".join((s.get("text") or "").replace("\n", " ").strip() for s in segs)
    res["faster_whisper_text"] = fw
    print("faster-whisper small: %s...\n" % fw[:220])

    eng = ""
    p = os.path.join(OUT, "spike5_russian.json")
    if os.path.exists(p):
        eng = json.load(open(p, encoding="utf-8")).get("engine", {}).get("text", "")
    res["engine_text"] = eng
    print("движок turbo: %s...\n" % eng[:220])

    res["wer_db_vs_faster"] = wer(ref_db, fw)
    res["wer_db_vs_engine"] = wer(ref_db, eng)
    res["wer_faster_vs_engine"] = wer(fw, eng)
    print("WER  БД      vs faster-whisper: %.4f" % res["wer_db_vs_faster"])
    print("WER  БД      vs движок:         %.4f" % res["wer_db_vs_engine"])
    print("WER  faster  vs движок:         %.4f" % res["wer_faster_vs_engine"])
    verdict = ("движок эквивалентен текущему пути (расхождение с БД — из-за таймкодов/сегментации)"
               if res["wer_faster_vs_engine"] < 0.15 else
               "движок транскрибирует ИНАЧЕ — требует разбора в W3 (VAD/сегментация/модель)")
    res["verdict"] = verdict
    print("ВЕРДИКТ: %s" % verdict)
    with open(os.path.join(OUT, "spike5_three_way.json"), "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1)
    print("сохранено: out/spike5_three_way.json")


if __name__ == "__main__":
    main()