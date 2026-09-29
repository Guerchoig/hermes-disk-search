"""Golden-прогон Python-версии по контрольным запросам заказчика на боевой БД
(W0 §4 «Что делать дальше» п.1, §12.3 плана MIGRATION_PLAN_RUST.md).

Запросы заданы заказчиком:
  1. «Найди на этом компе материалы о бабочках»
  2. «Найди на этом компе Технические задания»

БД открывается СТРОГО read-only (боевой индекс watcher'а не трогаем).
Топ-20 по каждому запросу пишется в tools/parity/golden/real_db_queries.json —
эталон для сравнения с Rust-реализацией в W1/W3.

Запуск:  .venv/Scripts/python.exe tools/parity/golden_queries.py
"""
import json
import os
import sqlite3
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
sys.path.insert(0, ROOT)

import sqlite_vec                                    # noqa: E402
from hds import search as searchmod                  # noqa: E402
from hds.config import dig, load                     # noqa: E402
from hds.embedder import make_embedder               # noqa: E402

QUERIES = [
    "Найди на этом компе материалы о бабочках",
    "Найди на этом компе Технические задания",
]
# фильтруем служебную часть запроса к агенту: поиску идут сами ключевые слова
SEARCH_Q = {QUERIES[0]: "материалы о бабочках", QUERIES[1]: "Технические задания"}


def main():
    from hds.config import db_abs_path

    cfg = load(os.path.join(ROOT, "config.yaml"))
    cfg.setdefault("index", {})["clip"] = False   # CLIP-модели не грузим (тяжело)
    db = db_abs_path(cfg)
    uri = "file:%s?mode=ro" % db.replace("\\", "/").replace("?", "%3f").replace("#", "%23")
    conn = sqlite3.connect(uri, uri=True)
    conn.row_factory = sqlite3.Row
    try:
        conn.enable_load_extension(True)
        sqlite_vec.load(conn)
        conn.enable_load_extension(False)
    except AttributeError:
        print("[!] sqlite3 без загрузки расширений — векторная ветка недоступна")

    emb, vec_ok, err = None, False, ""
    try:
        emb = make_embedder(cfg)
        emb.ping()
        vec_ok = True
    except Exception as e:  # noqa: BLE001
        err = str(e)[:200]

    out = {"db": db, "read_only": True, "vector_search_available": vec_ok,
           "embeddings_error": err, "queries": []}
    for q in QUERIES:
        t1 = time.time()
        results = searchmod.search(conn, emb, cfg, SEARCH_Q[q], limit=20)
        recs = [{"path": r["path"], "kind": r["kind"], "page": r["page"],
                 "t_start": r["t_start"], "score": r["score"],
                 "location": searchmod.format_location(r),
                 "snippet": r["snippet"]} for r in results]
        out["queries"].append({"query": q, "search_query": SEARCH_Q[q],
                               "elapsed_sec": round(time.time() - t1, 3),
                               "n": len(recs), "results": recs})
        print("[golden-db] %s -> %d результатов (%.2f с)"
              % (q, len(recs), time.time() - t1))
        for r in recs[:5]:
            print("   %s" % r["location"])
    conn.close()
    dst = os.path.join(BASE, "golden", "real_db_queries.json")
    with open(dst, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)
    print("Сохранено: %s (векторный поиск: %s)" % (dst, vec_ok))


if __name__ == "__main__":
    main()