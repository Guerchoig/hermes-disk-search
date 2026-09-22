"""Фаза A1: сколько токенов реально в наших чанках (токенизатор bge-m3).

Читает выборку чанков из рабочей БД (только чтение) и считает распределение
длин в токенах токенизатором BAAI/bge-m3 — сравнивает с контекстом модели,
загруженной в LM Studio (n_ctx).
"""
import os
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds.config import db_abs_path, dig, load  # noqa: E402

N_SAMPLE = 400


def main():
    cfg = load()
    db = db_abs_path(cfg)
    print("[A1] БД: %s" % db)
    print("[A1] chunk.size=%s chunk.overlap=%s (из config)"
          % (dig(cfg, "chunk.size", 1200), dig(cfg, "chunk.overlap", 200)))

    os.environ.setdefault("HF_HUB_OFFLINE", "0")
    from transformers import AutoTokenizer

    tok = AutoTokenizer.from_pretrained("BAAI/bge-m3")
    print("[A1] токенизатор bge-m3 загружен")

    import sqlite3
    conn = sqlite3.connect("file:%s?mode=ro" % db.replace("\\", "/"), uri=True)
    rows = conn.execute(
        "SELECT text FROM chunks ORDER BY RANDOM() LIMIT ?", (N_SAMPLE,)
    ).fetchall()
    conn.close()
    if not rows:
        print("[A1] чанков нет — БД пуста")
        return

    lens_chars = [len(r[0]) for r in rows]
    lens_tok = [len(tok.encode(r[0])) for r in rows]
    print("[A1] выборка: %d чанков" % len(rows))
    for name, arr, unit in (("символы", lens_chars, ""), ("токены", lens_tok, "")):
        arr_s = sorted(arr)
        print("[A1] %-8s min=%d  median=%d  p90=%d  p99=%d  max=%d"
              % (name, arr_s[0], statistics.median(arr_s),
                 arr_s[int(len(arr_s) * 0.9)], arr_s[int(len(arr_s) * 0.99)],
                 arr_s[-1]))

    worst = max(rows, key=lambda r: len(tok.encode(r[0])))
    print("[A1] самый длинный чанк: %d символов, %d токенов"
          % (len(worst[0]), len(tok.encode(worst[0]))))

    # чанк + вклеенный хвост overlap (реальный максимум текста, попадающего в модель)
    max_with_overlap = max(lens_tok) + len(tok.encode("я" * 200))
    print("[A1] верхняя оценка с overlap-хвостом: <= %d токенов" % max_with_overlap)

    for ctx in (512, 1024, 2048, 8192):
        over = sum(1 for t in lens_tok if t > ctx)
        print("[A1] чанков длиннее %5d токенов: %d из %d (%.1f%%)"
              % (ctx, over, len(lens_tok), 100.0 * over / len(lens_tok)))


if __name__ == "__main__":
    main()