"""Golden-генератор паритет-harness (W0 §4 п.2, §12.1 плана MIGRATION_PLAN_RUST.md).

Прогон ТЕКУЩЕЙ Python-реализации по фикстурам tools/parity/fixtures/ и запись
золотых артефактов в tools/parity/golden/:
  - <stem>.segments.json  — сегменты извлечения (kind + сегменты как в extractors);
  - <stem>.chunks.json    — чанки hds/chunker.py (байтово-точные), + число обрезанных;
  - <stem>.fts.json       — лемматизированный FTS-текст каждого чанка;
  - search_NN.json        — топ-20 поиска по контрольным запросам (queries.txt);
  - hash_manifest.json    — content_hash (Blake2b-16) всех фикстур;
  - manifest.json         — окружение golden-прогона (версии, параметры, доступность).

Запуск:  .venv/Scripts/python.exe tools/parity/golden.py
Опционально: HDS_PARITY_CLIP=1 — строить и CLIP-векторы картинок (нужны модели).
"""
import json
import os
import struct
import sys
import time

BASE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(BASE))
FIX = os.path.join(BASE, "fixtures")
GOLDEN = os.path.join(BASE, "golden")
OUT = os.path.join(BASE, "out")
sys.path.insert(0, ROOT)

from hds import chunker, clip_index, db as dbmod, extractors, indexer  # noqa: E402
from hds import lemmatizer, search as searchmod                        # noqa: E402
from hds.config import dig, load                                       # noqa: E402
from hds.embedder import make_embedder                                 # noqa: E402

# правило векторного покрытия: файлы с <= 1000 чанков получают векторы,
# гигантские csv/лог — только FTS (детерминированное правило; фиксируется
# в манифесте и повторяется Rust-реализацией при сверке)
VECTOR_MAX_CHUNKS = 1000


def build(cfg):
    """Извлечение + чанкинг + FTS-текст по всем фикстурам."""
    size = int(dig(cfg, "chunk.size", 800))
    overlap = int(dig(cfg, "chunk.overlap", 120))
    max_chunks = int(dig(cfg, "index.max_chunks", 3000))
    dim = int(dig(cfg, "embedding.dim", 1024))

    index_db = os.path.join(OUT, "index.db")
    for p in (index_db, index_db + "-wal", index_db + "-shm"):
        if os.path.exists(p):
            os.remove(p)
    conn = dbmod.connect(index_db, dim)

    emb, vec_ok, emb_err = None, False, ""
    try:
        emb = make_embedder(cfg)
        emb.ping()
        vec_ok = True
    except Exception as e:  # noqa: BLE001
        emb_err = str(e)[:200]
        print("[golden] эмбеддинги недоступны — golden-поиск будет только FTS: %s" % emb_err)
    bs = max(1, int(dig(cfg, "embedding.batch_size", 64)))

    files_meta = {}
    for name in sorted(os.listdir(FIX)):
        path = os.path.join(FIX, name)
        if not os.path.isfile(path):
            continue
        stem = os.path.splitext(name)[0]
        err, kind, segments = "", None, []
        try:
            kind, segments = extractors.extract(path, cfg)
        except Exception as e:  # noqa: BLE001
            err = str(e)[:300]
        if kind is None:
            kind, segments = "unknown", []
        chunks = chunker.make_chunks(segments, size, overlap) if segments else []
        n_cut = 0
        if max_chunks > 0 and len(chunks) > max_chunks:
            n_cut = len(chunks) - max_chunks
            chunks = chunks[:max_chunks]
        sz = os.path.getsize(path)
        chash = indexer.content_hash(path, sz)
        _w(stem + ".segments.json",
           {"file": name, "kind": kind, "error": err,
            "segments": [{"text": s.get("text"), "page": s.get("page"),
                          "t_start": s.get("t_start"), "t_end": s.get("t_end"),
                          "head": s.get("head")} for s in segments]})
        _w(stem + ".chunks.json",
           {"file": name, "kind": kind, "cut": n_cut,
            "chunks": [{"text": c["text"], "page": c.get("page"),
                        "t_start": c.get("t_start"), "t_end": c.get("t_end")}
                       for c in chunks]})
        _w(stem + ".fts.json",
           {"file": name, "fts": [lemmatizer.normalize(c["text"]) for c in chunks]})
        files_meta[name] = {"kind": kind, "size": sz, "content_hash": chash,
                            "n_segments": len(segments), "n_chunks": len(chunks),
                            "cut": n_cut, "error": err}
        fid = dbmod.upsert_file(conn, path, os.path.splitext(name)[1], kind,
                                sz, os.stat(path).st_mtime, chash)
        dbmod.delete_file_data(conn, fid)
        want_vec = vec_ok and chunks and len(chunks) <= VECTOR_MAX_CHUNKS
        if want_vec:
            try:
                for i in range(0, len(chunks), bs):
                    vecs = emb.embed([indexer.clip_for_embedding(c["text"])
                                      for c in chunks[i:i + bs]])
                    for c, v in zip(chunks[i:i + bs], vecs):
                        c["_blob"] = struct.pack("<%df" % len(v), *v)
                for i, c in enumerate(chunks):
                    cid = dbmod.add_chunk(conn, fid, i, c.get("page"),
                                          c.get("t_start"), c.get("t_end"), c["text"])
                    dbmod.add_vector(conn, cid, c["_blob"])
            except Exception as e:  # noqa: BLE001
                vec_ok = False
                emb_err = str(e)[:200]
                print("[golden] векторы отключены после ошибки: %s" % emb_err)
        else:
            for i, c in enumerate(chunks):
                dbmod.add_chunk(conn, fid, i, c.get("page"),
                                c.get("t_start"), c.get("t_end"), c["text"])
        files_meta[name]["vectorized"] = bool(want_vec and vec_ok)
        print("[golden] %-44s kind=%-7s segs=%-4d chunks=%-5d cut=%-2d vec=%s"
              % (name, kind, len(segments), len(chunks), n_cut,
                 files_meta[name]["vectorized"]), flush=True)
    return conn, emb, vec_ok, emb_err, files_meta
def _w(name, obj):
    with open(os.path.join(GOLDEN, name), "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False, indent=1)
def run_search(conn, emb, cfg):
    """Топ-20 по контрольным запросам; golden включает и format_location."""
    queries = [ln.strip() for ln in open(os.path.join(BASE, "queries.txt"),
                                         encoding="utf-8") if ln.strip()
               and not ln.startswith("#")]
    out = []
    for i, q in enumerate(queries, 1):
        t1 = time.time()
        results = searchmod.search(conn, emb, cfg, q, limit=20)
        dt = time.time() - t1
        recs = [{"path": r["path"], "ext": r["ext"], "kind": r["kind"],
                 "page": r["page"], "t_start": r["t_start"], "t_end": r["t_end"],
                 "score": r["score"],
                 "location": searchmod.format_location(r),
                 "snippet": r["snippet"], "text": r["text"]} for r in results]
        _w("search_%02d.json" % i, {"query": q, "elapsed_sec": round(dt, 3),
                                    "n": len(recs), "results": recs})
        print("[golden] Q%02d %-46s -> %d результатов (%.2f с)"
              % (i, q, len(recs), dt), flush=True)
        out.append(q)
    return out


def manifest(cfg, vec_ok, emb_err, files_meta, clip_ok, queries):
    import sqlite_vec
    import pymorphy3

    m = {
        "generated": time.strftime("%Y-%m-%d %H:%M:%S"),
        "python": sys.version.split()[0],
        "sqlite_vec": sqlite_vec.__dict__.get("version", "0.1.9"),
        "pymorphy3": getattr(pymorphy3, "__version__", "2.0.6"),
        "lemmatizer_available": lemmatizer.available(),
        "chunk": {"size": int(dig(cfg, "chunk.size", 800)),
                  "overlap": int(dig(cfg, "chunk.overlap", 120))},
        "max_chunks": int(dig(cfg, "index.max_chunks", 3000)),
        "vector_max_chunks": VECTOR_MAX_CHUNKS,
        "content_hash": {"algo": "blake2b", "digest_size": 16,
                         "head_bytes": 256 * 1024,
                         "input": "str(size) + head[0:262144] + tail[-262144:]"},
        "vector_search_available": vec_ok,
        "embeddings_error": emb_err,
        "embeddings": {"base_url": dig(cfg, "embedding.base_url"),
                       "model": dig(cfg, "embedding.model"),
                       "dim": dig(cfg, "embedding.dim")},
        "transcribe": bool(dig(cfg, "index.transcribe", False)),
        "clip": clip_ok,
        "ocr": bool(dig(cfg, "index.ocr", False)),
        "queries": queries,
        "files": files_meta,
    }
    _w("manifest.json", m)
    _w("hash_manifest.json",
       {n: {"kind": v["kind"], "size": v["size"], "content_hash": v["content_hash"]}
        for n, v in files_meta.items()})
    print("[golden] manifest.json записан; векторный поиск: %s" % vec_ok)


def main():
    os.makedirs(GOLDEN, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)
    cfg = load(os.path.join(ROOT, "config.yaml"))
    cfg.setdefault("index", {})["roots"] = []          # изоляция: только фикстуры
    # транскрипция: включаем только если модель уже скачана (без сетевых загрузок)
    wdir = os.path.join(ROOT, "models",
                        "whisper-" + str(dig(cfg, "index.whisper_model", "small")))
    transcribe_ok = os.path.exists(os.path.join(wdir, "model.bin"))
    cfg["index"]["transcribe"] = transcribe_ok
    # CLIP: по умолчанию выключен (модели тяжёлые); HDS_PARITY_CLIP=1 включает
    clip_enabled = os.environ.get("HDS_PARITY_CLIP", "") == "1"
    cfg["index"]["clip"] = clip_enabled

    t0 = time.time()
    conn, emb, vec_ok, emb_err, files_meta = build(cfg)
    print("[golden] извлечение+чанкинг+векторы: %.1f с" % (time.time() - t0), flush=True)
    clip_ok = False
    if clip_enabled:
        clip_ok = clip_index.available()
        if clip_ok:
            for name, meta in files_meta.items():
                if meta["kind"] == "image":
                    fid = conn.execute("SELECT id FROM files WHERE path=?",
                                       (os.path.join(FIX, name),)).fetchone()
                    if fid:
                        clip_index.store_for_file(conn, fid[0], os.path.join(FIX, name))
    queries = run_search(conn, emb, cfg)
    manifest(cfg, vec_ok, emb_err, files_meta, clip_ok, queries)
    conn.close()
    print("[golden] ГОТОВО: %s" % GOLDEN)


if __name__ == "__main__":
    main()