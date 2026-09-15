"""SQLite-хранилище: файлы, чанки, FTS5-полнотекст и векторы (sqlite-vec)."""
import os
import sqlite3

import sqlite_vec

from .clip_index import CLIP_DIM

SCHEMA = """
CREATE TABLE IF NOT EXISTS files(
  id INTEGER PRIMARY KEY,
  path TEXT UNIQUE NOT NULL,
  ext TEXT,
  kind TEXT,
  size INTEGER,
  mtime REAL,
  content_hash TEXT,
  status TEXT DEFAULT 'new',
  error TEXT,
  chunk_count INTEGER DEFAULT 0,
  indexed_at REAL
);
CREATE INDEX IF NOT EXISTS idx_files_kind ON files(kind);
CREATE INDEX IF NOT EXISTS idx_files_hash ON files(content_hash);
CREATE INDEX IF NOT EXISTS idx_files_status ON files(status);
CREATE TABLE IF NOT EXISTS chunks(
  id INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  ord INTEGER,
  page INTEGER,
  t_start REAL,
  t_end REAL,
  text TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chunks_file ON chunks(file_id);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(text);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(embedding float[{dim}]);
"""


def connect(db_path, dim):
    d = os.path.dirname(os.path.abspath(db_path))
    if d:
        os.makedirs(d, exist_ok=True)
    conn = sqlite3.connect(db_path, check_same_thread=False)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA journal_mode=WAL")
    conn.execute("PRAGMA synchronous=NORMAL")
    conn.execute("PRAGMA foreign_keys=ON")
    conn.execute("PRAGMA busy_timeout=5000")  # параллельные записи (watcher + reconcile)
    vec_ok = True
    try:
        conn.enable_load_extension(True)
        sqlite_vec.load(conn)
        conn.enable_load_extension(False)
    except AttributeError:
        # python.org-сборка macOS собрана без загрузки расширений sqlite3:
        # векторные таблицы недоступны, поиск деградирует к ключевым словам
        # (FTS5). Система продолжает работать. Решение: brew install python
        vec_ok = False
        print("[db] sqlite3 без поддержки загрузки расширений — векторный поиск "
              "недоступен, работает только ключевой поиск (FTS5). "
              "Рекомендуется Python из Homebrew: brew install python")
    conn.execute("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT)")
    row = conn.execute("SELECT value FROM meta WHERE key='vec_dim'").fetchone()
    # проверяем фактическую размерность таблицы пробной вставкой с откатом
    mismatch = True
    if vec_ok:
        probe = b"\x00" * (int(dim) * 4)
        conn.execute("SAVEPOINT probe_sp")
        try:
            conn.execute(
                "INSERT OR REPLACE INTO chunks_vec(rowid, embedding) VALUES(1, ?)",
                (probe,),
            )
            mismatch = False
        except sqlite3.OperationalError:
            mismatch = True
        finally:
            conn.execute("ROLLBACK TO probe_sp")
            conn.execute("RELEASE probe_sp")
    if mismatch:
        # сменилась модель/размерность эмбеддингов — векторная таблица пересоздаётся,
        # векторы восстановятся при `index --full`
        conn.execute("DROP TABLE IF EXISTS chunks_vec")
        if vec_ok:
            print("[db] размерность эмбеддингов -> %d, векторная таблица пересоздана "
                  "(запустите 'index --full' для повторной векторизации)" % int(dim))
        row = None
    schema = SCHEMA if vec_ok else SCHEMA.replace(
        "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(embedding float[{dim}]);\n", "")
    conn.executescript(schema.format(dim=int(dim)))
    # CLIP-векторы картинок (поиск по содержанию); фиксированная размерность 512
    if vec_ok:
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS images_vec "
            "USING vec0(embedding float[{}])".format(CLIP_DIM)
        )
    # бэкфилл: файлы, проиндексированные до введения indexed_at
    try:
        c_old = conn.execute(
            "SELECT COUNT(*) FROM files WHERE status='indexed' AND indexed_at IS NULL"
        ).fetchone()[0]
        if c_old:
            conn.execute(
                "UPDATE files SET indexed_at = COALESCE(mtime, 0) + 10 "
                "WHERE status='indexed' AND indexed_at IS NULL")
            print("[db] indexed_at заполнен по mtime для %d старых записей" % c_old)
    except sqlite3.OperationalError:
        pass
    conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES('vec_dim', ?)", (str(int(dim)),))
    conn.commit()
    return conn


def get_file_by_path(conn, path):
    return conn.execute("SELECT * FROM files WHERE path=?", (path,)).fetchone()


def get_file_by_hash(conn, chash):
    if not chash:
        return None
    return conn.execute(
        "SELECT * FROM files WHERE content_hash=? LIMIT 1", (chash,)
    ).fetchone()


def upsert_file(conn, path, ext, kind, size, mtime, content_hash):
    conn.execute(
        """INSERT INTO files(path, ext, kind, size, mtime, content_hash, status)
           VALUES(?,?,?,?,?,?,'new')
           ON CONFLICT(path) DO UPDATE SET
             ext=excluded.ext, kind=excluded.kind, size=excluded.size,
             mtime=excluded.mtime, content_hash=excluded.content_hash""",
        (path, ext, kind, size, mtime, content_hash),
    )
    conn.commit()
    return get_file_by_path(conn, path)["id"]


def rename_path(conn, old_path, new_path):
    conn.execute("UPDATE files SET path=? WHERE path=?", (new_path, old_path))
    conn.commit()


def remove_path(conn, path):
    row = get_file_by_path(conn, path)
    if not row:
        return False
    delete_file_data(conn, row["id"])
    conn.execute("DELETE FROM files WHERE id=?", (row["id"],))
    conn.commit()
    return True


def delete_file_data(conn, file_id):
    ids = [r[0] for r in conn.execute("SELECT id FROM chunks WHERE file_id=?", (file_id,))]
    for cid in ids:
        conn.execute("DELETE FROM chunks_fts WHERE rowid=?", (cid,))
        conn.execute("DELETE FROM chunks_vec WHERE rowid=?", (cid,))
    conn.execute("DELETE FROM chunks WHERE file_id=?", (file_id,))
    # CLIP-вектор картинки (rowid = files.id)
    try:
        conn.execute("DELETE FROM images_vec WHERE rowid=?", (file_id,))
    except sqlite3.OperationalError:
        pass  # таблицы может не быть в старых БД до первого connect() с CLIP-схемой


def add_chunk(conn, file_id, ord_, page, t_start, t_end, text):
    cur = conn.execute(
        "INSERT INTO chunks(file_id, ord, page, t_start, t_end, text) VALUES(?,?,?,?,?,?)",
        (file_id, ord_, page, t_start, t_end, text),
    )
    cid = cur.lastrowid
    conn.execute("INSERT INTO chunks_fts(rowid, text) VALUES(?,?)", (cid, text))
    return cid


def add_vector(conn, chunk_id, vector_blob):
    conn.execute(
        "INSERT OR REPLACE INTO chunks_vec(rowid, embedding) VALUES(?,?)",
        (chunk_id, vector_blob),
    )


def finish_file(conn, file_id, status, error=None, chunks=None):
    import time

    if status == "indexed":
        conn.execute(
            "UPDATE files SET status=?, error=NULL, indexed_at=?, chunk_count=? WHERE id=?",
            (status, time.time(), chunks if chunks is not None else 0, file_id))
    else:
        conn.execute("UPDATE files SET status=?, error=? WHERE id=?",
                     (status, error, file_id))


def all_files(conn):
    return conn.execute("SELECT id, path FROM files").fetchall()


def stats(conn):
    by_kind = conn.execute("SELECT kind, COUNT(*) FROM files GROUP BY kind").fetchall()
    by_status = conn.execute("SELECT status, COUNT(*) FROM files GROUP BY status").fetchall()
    chunks_total = conn.execute("SELECT COUNT(*) FROM chunks").fetchone()[0]
    last = conn.execute("SELECT MAX(indexed_at) FROM files").fetchone()[0]
    errors = conn.execute(
        "SELECT path, error FROM files WHERE status='error' LIMIT 10"
    ).fetchall()
    return {
        "by_kind": [tuple(r) for r in by_kind],
        "by_status": [tuple(r) for r in by_status],
        "chunks": chunks_total,
        "last_indexed_at": last,
        "errors": [tuple(r) for r in errors],
    }