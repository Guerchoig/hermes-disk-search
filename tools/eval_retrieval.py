"""Фаза 0: eval-набор и замер качества поиска (recall@k, MRR, латентность).

Запуск из корня проекта (см. PLAN_INDEX_QUALITY.md):

    # генерация golden-set локальной чат-моделью по случайным чанкам индекса:
    $env:PYTHONPATH='<корень проекта>'
    .\\.venv\\Scripts\\python.exe tools\\eval_retrieval.py --generate 60

    # замер по существующей БД (секунды) + сохранение/сравнение baseline:
    .\\.venv\\Scripts\\python.exe tools\\eval_retrieval.py --measure

Файлы (eval/ в .gitignore — персональные данные, в репозиторий не коммитятся):
- eval/golden.jsonl  — вопросы с ground-truth chunk_id и типом файла;
- eval/baseline.json — метрики текущего кода (для сравнения «до/после» фаз).
"""
import argparse
import json
import os
import statistics
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from hds import db as dbmod  # noqa: E402
from hds.config import db_abs_path, dig, load  # noqa: E402
from hds.search import fts_search_ids  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EVAL_DIR = os.path.join(ROOT, "eval")
GOLDEN = os.path.join(EVAL_DIR, "golden.jsonl")
BASELINE = os.path.join(EVAL_DIR, "baseline.json")

# Типы файлов, по которым строится golden-set (media = транскрипты)
GEN_KINDS = ("text", "docx", "pdf", "media", "xlsx", "pptx", "mpp")

Q_PROMPT = (
    "Придумай один короткий конкретный вопрос на русском языке (8-15 слов), "
    "ответ на который целиком содержится в приведённом ниже фрагменте документа. "
    "Используй формулировки фрагмента, но не копируй подряд длинные фразы. "
    "В ответе напиши только сам вопрос, без нумерации и пояснений.\n\nФрагмент:\n%s"
)


def _conn(cfg):
    return dbmod.connect(db_abs_path(cfg), int(dig(cfg, "embedding.dim", 1024)))


def _chat_generate(cfg, chunk_text, timeout=120):
    """Один вопрос по фрагменту через локальную чат-модель (None при сбое)."""
    import re

    import requests

    url = dig(cfg, "chat.base_url", "http://localhost:1234/v1").rstrip("/") \
        + "/chat/completions"
    payload = {
        "model": dig(cfg, "chat.model", "local-model"),
        "temperature": 0.4,
        # «думающим» моделям (qwen3.5) нужен запас: размышления съедают лимит,
        # при малых max_tokens ответ обрезается до пустого/недописанного
        "max_tokens": 400,
        "messages": [
            {"role": "system",
             "content": "Отвечай сразу, без рассуждений и пояснений. /no_think"},
            {"role": "user", "content": Q_PROMPT % chunk_text},
        ],
    }
    r = requests.post(url, json=payload, timeout=timeout)
    r.raise_for_status()
    msg = r.json()["choices"][0]["message"]
    q = (msg.get("content") or "").strip()
    if not q:
        # «думающие» модели (qwen3.5 и т.п.) кладут ответ в reasoning_content,
        # оставляя content пустым — берём текст оттуда
        q = (msg.get("reasoning_content") or "").strip()
    # выбросить блок размышлений <think>…</think>, если модель его писала
    q = re.sub(r"<think>.*?</think>", "", q, flags=re.S).strip()
    q = q.strip("\"'`«» \n\r\t")
    lines = [l.strip(" \"'`«» \t") for l in q.splitlines() if l.strip()]
    if not lines:
        return None
    # вопрос обычно в КОНЦЕ (после размышлений) и заканчивается «?»;
    # первая строка reasoning у «думающих» моделей — мусор («Thinking Process:»)
    q = lines[-1]
    if not q.endswith("?"):
        candidates = [l for l in lines if l.endswith("?")]
        if candidates:
            q = candidates[-1]
    if len(q) < 8 or q.rstrip(": ").lower() in ("thinking process", "thinking"):
        return None  # мусор вместо вопроса — чанк пропускается
    return q


def generate(cfg, n):
    """Сгенерировать golden-set: N вопросов по случайным чанкам индекса."""
    conn = _conn(cfg)
    rows = conn.execute(
        "SELECT c.id, c.text, f.kind FROM chunks c JOIN files f ON f.id=c.file_id "
        "WHERE length(c.text) BETWEEN 200 AND 3000 AND f.kind IN (%s) "
        "ORDER BY RANDOM() LIMIT ?"
        % ",".join("'%s'" % k for k in GEN_KINDS),
        (n,),
    ).fetchall()
    if not rows:
        print("В индексе нет подходящих чанков (200-3000 символов).")
        return 1
    os.makedirs(EVAL_DIR, exist_ok=True)
    print("Генерация %d вопросов локальной чат-моделью (%s)..."
          % (len(rows), dig(cfg, "chat.model", "?")))
    ok, skipped = 0, 0
    with open(GOLDEN, "w", encoding="utf-8") as f:
        for i, (cid, text, kind) in enumerate(rows, 1):
            try:
                q = _chat_generate(cfg, text)
            except Exception as e:  # noqa: BLE001
                print("  [skip] чанк %d: %s" % (cid, str(e)[:100]))
                skipped += 1
                continue
            if not q:
                print("  [skip] чанк %d: модель вернула пустой ответ" % cid)
                skipped += 1
                continue
            f.write(json.dumps({"qid": i, "chunk_id": cid, "kind": kind,
                                "question": q}, ensure_ascii=False) + "\n")
            ok += 1
            if i % 10 == 0 or i == len(rows):
                print("  %d/%d" % (i, len(rows)), flush=True)
    print("[done] golden-set: %d вопросов, %d пропущено -> %s" % (ok, skipped, GOLDEN))
    conn.close()
    return 0


def _fts_rank(conn, question, limit):
    """FTS-ветка (BM25, AND->OR с префиксом — как в поиске): chunk_id + время, мс."""
    t0 = time.perf_counter()
    ids = fts_search_ids(conn, question, limit)
    return ids, (time.perf_counter() - t0) * 1000.0


def _vec_rank(conn, emb, question, limit):
    """Векторная ветка: список chunk_id + время, мс (None — ветка недоступна)."""
    if emb is None or not getattr(emb, "available", False):
        return None, 0.0
    t0 = time.perf_counter()
    try:
        qv = emb.embed_query(question)
        blob = struct.pack("<%df" % len(qv), *qv)
        rows = conn.execute(
            "SELECT rowid FROM chunks_vec WHERE embedding MATCH ? "
            "AND k = ? ORDER BY distance", (blob, limit)).fetchall()
    except Exception as e:  # noqa: BLE001
        print("  [vec] недоступен: %s" % str(e)[:100], file=sys.stderr)
        return None, 0.0
    return [r[0] for r in rows], (time.perf_counter() - t0) * 1000.0


def _hybrid_rank(fts_ids, vec_ids, rrf_k, fts_w, vec_w):
    """Слияние веток по RRF (та же формула, что в hds.search) -> chunk_id."""
    scores = {}
    for ids, w in ((fts_ids, fts_w), (vec_ids, vec_w)):
        if ids is None:
            continue
        for rank, ch in enumerate(ids):
            scores[ch] = scores.get(ch, 0.0) + w / (rrf_k + rank)
    return [ch for ch, _ in sorted(scores.items(), key=lambda kv: -kv[1])]


def _metrics(ranks_ms):
    """ranks_ms: [(rank, ms)] -> recall@5, recall@10, MRR, медиана мс."""
    n = len(ranks_ms)
    ranks = [r for r, _ in ranks_ms]
    ms = [m for _, m in ranks_ms]
    r5 = sum(1 for r in ranks if 0 < r <= 5) / n
    r10 = sum(1 for r in ranks if 0 < r <= 10) / n
    mrr = sum(1.0 / r if r else 0.0 for r in ranks) / n
    return {"recall@5": round(r5, 4), "recall@10": round(r10, 4),
            "mrr": round(mrr, 4), "median_ms": round(statistics.median(ms), 1)}


def measure(cfg):
    """Замер recall@5/10, MRR и латентности по веткам fts/vec/hybrid."""
    from hds.embedder import make_embedder

    if not os.path.exists(GOLDEN):
        print("Нет %s — сначала запустите --generate" % GOLDEN)
        return 1
    items = [json.loads(l) for l in open(GOLDEN, encoding="utf-8") if l.strip()]
    # пропустить строки без вопроса (модель вернула пустой ответ при генерации)
    bad = [x for x in items if not x.get("question")]
    if bad:
        print("Пропущено вопросов без текста: %d" % len(bad))
        items = [x for x in items if x.get("question")]
    if not items:
        print("В %s нет ни одного вопроса — перегенерируйте: --generate" % GOLDEN)
        return 1
    conn = _conn(cfg)
    emb = make_embedder(cfg)
    fts_k = max(10, int(dig(cfg, "search.fts_k", 40)))
    vec_k = max(10, int(dig(cfg, "search.vec_k", 40)))
    rrf_k = int(dig(cfg, "search.rrf_k", 60))
    fts_w = float(dig(cfg, "search.fts_weight", 1.0))
    vec_w = float(dig(cfg, "search.vec_weight", 1.0))
    all_stats = {}   # ветка -> [(rank, ms)]
    kind_stats = {}  # kind -> ветка -> [(rank, ms)]
    t0 = time.time()
    for it in items:
        gold = it["chunk_id"]
        fts_ids, fms = _fts_rank(conn, it["question"], fts_k)
        vec_ids, vms = _vec_rank(conn, emb, it["question"], vec_k)
        hyb_ids = _hybrid_rank(fts_ids, vec_ids, rrf_k, fts_w, vec_w)
        for br, ids, ms in (("fts", fts_ids, fms), ("vec", vec_ids, vms),
                            ("hybrid", hyb_ids, fms + vms)):
            if ids is None:
                continue
            rank = ids.index(gold) + 1 if gold in ids else 0
            all_stats.setdefault(br, []).append((rank, ms))
            kind_stats.setdefault(it["kind"], {}).setdefault(br, []).append((rank, ms))
    print("Вопросов: %d | веса FTS/вектор: %.1f/%.1f | rrf_k=%d | замер %.1f с"
          % (len(items), fts_w, vec_w, rrf_k, time.time() - t0))
    out = {"n_questions": len(items), "branches": {}, "by_kind": {},
           "generated_at": time.strftime("%Y-%m-%d %H:%M:%S"),
           "config": {k: dig(cfg, k) for k in
                      ("search.fts_k", "search.vec_k", "search.rrf_k",
                       "search.fts_weight", "search.vec_weight")}}
    prev = None
    if os.path.exists(BASELINE):
        try:
            with open(BASELINE, encoding="utf-8") as f:
                prev = json.load(f)
        except Exception:  # noqa: BLE001
            prev = None
    print("\n%-8s %10s %10s %8s %11s" % ("ветка", "recall@5", "recall@10", "MRR", "медиана,мс"))
    for br in ("fts", "vec", "hybrid"):
        if br in all_stats:
            m = _metrics(all_stats[br])
            out["branches"][br] = m
            print("%-8s %10.3f %10.3f %8.3f %11.1f"
                  % (br, m["recall@5"], m["recall@10"], m["mrr"], m["median_ms"]))
    for kind, branches in sorted(kind_stats.items()):
        out["by_kind"][kind] = {}
        print("\n  [%s]" % kind)
        for br in ("fts", "vec", "hybrid"):
            if br in branches:
                mm = _metrics(branches[br])
                out["by_kind"][kind][br] = mm
                print("  %-6s %10.3f %10.3f %8.3f %11.1f"
                      % (br, mm["recall@5"], mm["recall@10"], mm["mrr"], mm["median_ms"]))
    if prev and prev.get("branches"):
        print("\nСравнение с предыдущим baseline (%s):" % prev.get("generated_at", "?"))
        for br, m in out["branches"].items():
            p = prev["branches"].get(br)
            if p:
                print("  %-8s recall@10 %.3f -> %.3f | MRR %.3f -> %.3f"
                      % (br, p["recall@10"], m["recall@10"], p["mrr"], m["mrr"]))
    with open(BASELINE, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, indent=1)
    print("[done] baseline сохранён: %s" % BASELINE)
    conn.close()
    return 0


def main():
    ap = argparse.ArgumentParser(description="Eval качества поиска (Фаза 0)")
    ap.add_argument("--generate", type=int, metavar="N",
                    help="сгенерировать N вопросов golden-set (локальная чат-модель)")
    ap.add_argument("--measure", action="store_true",
                    help="замер метрик по eval/golden.jsonl -> eval/baseline.json")
    args = ap.parse_args()
    cfg = load()
    if args.generate:
        return generate(cfg, args.generate)
    if args.measure:
        return measure(cfg)
    ap.print_help()
    return 1


if __name__ == "__main__":
    sys.exit(main())