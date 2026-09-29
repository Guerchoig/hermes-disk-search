"""Корпус и эталон лемматизации для паритет-теста (риск R2, §6 плана).

Формирует:
  * out/lemma_corpus.tsv — 100 000 токенов из боевого chunks_fts + эталонная лемма
    pymorphy3 (вариант A, текущее поведение `lemmatize_token`);
  * out/lemma_stats.json — время и стоимость нормализации 100k токенов, кэш-статистика.

Этот файл — эталон для будущего паритет-теста Rust-нормализации: строка-в-строку.
"""
import json
import os
import random
import sqlite3
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
OUT = os.path.join(BASE, "out")
sys.path.insert(0, ROOT)

DB = r"D:\hermes-disk-search-db\index.db"
N = 100_000


def sample_tokens(n):
    """Случайные токены из chunks_fts боевой БД (read-only)."""
    conn = sqlite3.connect("file:%s?mode=ro" % DB.replace("\\", "/"), uri=True)
    total = conn.execute("SELECT COUNT(*) FROM chunks_fts").fetchone()[0]
    out = []
    step = max(1, total // 2000)
    rows = conn.execute(
        "SELECT text FROM chunks_fts WHERE rowid %% %d = 0 LIMIT 3000" % step).fetchall()
    conn.close()
    for (text,) in rows:
        out.extend(text.split())
        if len(out) >= n * 2:
            break
    random.seed(42)                      # воспроизводимость выбора
    random.shuffle(out)
    return out[:n]


def main():
    os.makedirs(OUT, exist_ok=True)
    from hds import lemmatizer

    assert lemmatizer.available(), "pymorphy3 недоступен — эталон построить нельзя"
    toks = sample_tokens(N)
    print("токенов: %d (уникальных %d)" % (len(toks), len(set(toks))))

    t0 = time.time()
    lemmas = [lemmatizer.lemmatize_token(t) for t in toks]
    dt = time.time() - t0
    changed = sum(1 for t, l in zip(toks, lemmas) if t.lower() != l)

    path = os.path.join(OUT, "lemma_corpus.tsv")
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        for t, l in zip(toks, lemmas):
            f.write("%s\t%s\n" % (t, l))
    size = os.path.getsize(path)
    stats = {
        "tokens": len(toks), "unique": len(set(toks)),
        "sec_total": round(dt, 2), "us_per_token": round(dt * 1e6 / len(toks), 1),
        "tokens_per_sec": int(len(toks) / max(dt, 1e-9)),
        "changed_vs_form": changed,
        "cache_size": len(lemmatizer._cache),
        "cache_max": lemmatizer._CACHE_MAX,
        "corpus_tsv_mb": round(size / 1048576, 2),
        "corpus_path": os.path.relpath(path, ROOT),
    }
    # повторный прогон на прогретом кэше — оценка латентности тёплого пути
    t1 = time.time()
    for t in toks:
        lemmatizer.lemmatize_token(t)
    stats["sec_warm"] = round(time.time() - t1, 2)
    with open(os.path.join(OUT, "lemma_stats.json"), "w", encoding="utf-8") as f:
        json.dump(stats, f, ensure_ascii=False, indent=1)
    print(json.dumps(stats, ensure_ascii=False, indent=1))


if __name__ == "__main__":
    main()