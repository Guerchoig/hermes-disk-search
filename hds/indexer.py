"""Инкрементальный индексатор: обход дисков, извлечение, чанкинг, эмбеддинги."""
import hashlib
import os
import struct
import time

from . import chunker, db as dbmod, extractors
from .config import dig, db_abs_path

MEDIA_KINDS = {"media"}


def content_hash(path, size):
    """Быстрый отпечаток: размер + первые/последние 256 КБ содержимого."""
    h = hashlib.blake2b(digest_size=16)
    h.update(str(size).encode())
    head = 256 * 1024
    try:
        with open(path, "rb") as f:
            h.update(f.read(head))
            if size > head:
                f.seek(-head, 2)
                h.update(f.read(head))
    except OSError:
        return None
    return h.hexdigest()


def iter_files(cfg, roots=None):
    roots = roots or dig(cfg, "index.roots", [])
    excl = {str(e).lower() for e in dig(cfg, "index.exclude_dirs", [])}
    for root in roots:
        root = os.path.abspath(root)
        if not os.path.isdir(root):
            print("[skip] корень не найден: %s" % root)
            continue
        print("[scan] %s" % root)
        for dirpath, dirnames, filenames in os.walk(root, onerror=lambda e: None):
            dirnames[:] = [d for d in dirnames if d.lower() not in excl]
            for fn in filenames:
                yield os.path.join(dirpath, fn)


def _limit_mb(kind, cfg):
    cap = dig(cfg, "index.max_media_mb", 1500) if kind in MEDIA_KINDS \
        else dig(cfg, "index.max_file_mb", 200)
    return int(cap) * 1024 * 1024


def _kind_of(ext):
    from .extract_av import kind_for_ext_media as av_kind
    from .extract_static import kind_for_ext_media as st_kind
    return extractors.kind_for_ext(ext) or st_kind(ext) or av_kind(ext)  # noqa: E501


def process_file(conn, emb, cfg, path, force=False):
    """Индексирует один файл. Возвращает (статус, kind)."""
    ext = os.path.splitext(path)[1].lower()
    kind = _kind_of(ext)
    if not kind:
        return "skipped_type", None

    try:
        st = os.stat(path)
    except OSError as e:
        return "stat_error: %s" % e, kind
    size, mtime = st.st_size, st.st_mtime
    if size > _limit_mb(kind, cfg):
        return "skipped_big", kind

    row = dbmod.get_file_by_path(conn, path)
    if not force and row and row["status"] == "indexed" \
            and row["size"] == size and abs((row["mtime"] or 0) - mtime) < 2:
        return "unchanged", kind

    # переименование/переезд: контент уже в индексе под другим путём
    chash = content_hash(path, size)
    if not force and (row is None or row["status"] != "indexed"):
        same = dbmod.get_file_by_hash(conn, chash)
        if same is not None and same["path"] != path and same["chunk_count"] > 0:
            dbmod.rename_path(conn, same["path"], path)
            return "moved", kind

    fid = dbmod.upsert_file(conn, path, ext, kind, size, mtime, chash)
    try:
        kind2, segments = extractors.extract(path, cfg)
        chunks = chunker.make_chunks(
            segments,
            dig(cfg, "chunk.size", 1200),
            dig(cfg, "chunk.overlap", 200),
        ) if segments else []
        vectors = emb.embed([c["text"] for c in chunks]) if chunks else []
        for c, v in zip(chunks, vectors):
            c["_blob"] = struct.pack("<%df" % len(v), *v)
    except Exception as e:  # noqa: BLE001
        dbmod.finish_file(conn, fid, "error", str(e)[:500])
        conn.commit()
        return "error: %s" % str(e)[:200], kind

    dbmod.delete_file_data(conn, fid)
    for i, c in enumerate(chunks):
        cid = dbmod.add_chunk(conn, fid, i, c["page"], c["t_start"], c["t_end"], c["text"])
        dbmod.add_vector(conn, cid, c["_blob"])
    dbmod.finish_file(conn, fid, "indexed")
    conn.commit()
    return "indexed(%d чанков)" % len(chunks), kind
def run_index(conn, emb, cfg, roots=None, kinds=None, full=False,
              limit=None, single_paths=None, prune=True, confirm_delete=False):
    """Проход по корням; возвращает сводку. Выводит прогресс в stdout."""
    t0 = time.time()
    counters = {}
    seen_roots = single_paths is None
    paths = single_paths if single_paths is not None else iter_files(cfg, roots)
    n = 0
    for path in paths:
        n += 1
        status, kind = process_file(conn, emb, cfg, path, force=full)
        key = status.split("(")[0]
        counters[key] = counters.get(key, 0) + 1
        if not status.startswith("unchanged") and (n % 20 == 1 or status.startswith(("indexed", "error"))):
            print("[%d] %s -> %s" % (n, path, status), flush=True)
        if limit and n >= limit:
            break

    if prune and seen_roots:
        _prune_deleted(conn, cfg, confirm_delete=confirm_delete)

    counters["elapsed_sec"] = round(time.time() - t0, 1)
    counters["files_seen"] = n
    print("[done] %s" % counters)
    return counters


def _prune_deleted(conn, cfg, confirm_delete=False):
    total = conn.execute("SELECT COUNT(*) FROM files").fetchone()[0]
    if not total:
        return
    missing = []
    for fid, path in dbmod.all_files(conn):
        if not os.path.exists(path):
            missing.append((fid, path))
    if not missing:
        return
    if len(missing) > 0.2 * total and not confirm_delete:
        print("[prune] Пропало %d из %d файлов (>%d%%). Похоже, диск был отключён. "
              "Удаление заблокировано; перезапустите с --confirm-delete, если это ожидаемо."
              % (len(missing), total, 20))
        return
    for fid, path in missing:
        dbmod.delete_file_data(conn, fid)
        conn.execute("DELETE FROM files WHERE id=?", (fid,))
    conn.commit()
    print("[prune] Удалено из индекса: %d" % len(missing))


def reindex_path(conn, emb, cfg, path, force=True):
    p = os.path.abspath(path)
    if os.path.isfile(p):
        return [process_file(conn, emb, cfg, p, force=force)]
    res = []
    excl = {str(e).lower() for e in dig(cfg, "index.exclude_dirs", [])}
    for dirpath, dirnames, filenames in os.walk(p, onerror=lambda e: None):
        dirnames[:] = [d for d in dirnames if d.lower() not in excl]
        for fn in filenames:
            res.append(process_file(conn, emb, cfg, os.path.join(dirpath, fn), force=force))
    return res