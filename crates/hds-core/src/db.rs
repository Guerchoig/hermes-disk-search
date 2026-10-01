//! Порт `hds/db.py`: схема `index.db` **байт-в-байт**, `PRAGMA`, подключение
//! `sqlite-vec` (vec0), `meta.vec_dim`, бэкфилл `indexed_at` и CRUD.
//!
//! Что намеренно сохранено 1:1 с Python:
//! * `SCHEMA` — та же последовательность `CREATE TABLE/INDEX/VIRTUAL TABLE`
//!   (обратная совместимость с Python-версией обязательна, §3.1 плана);
//! * порядок `PRAGMA`: `journal_mode=WAL`, `synchronous=NORMAL`,
//!   `foreign_keys=ON`, `busy_timeout=30000` (параллельные записи
//!   watcher + reconcile + MCP);
//! * vec0 регистрируется **только** через `sqlite3_auto_extension`
//!   (грабля спайка 1: прямой `sqlite3_vec_init()` модуль vec0 не регистрирует);
//! * `meta.vec_dim` читается без записи; при несовпадении — проба вставки
//!   `SAVEPOINT`/`ROLLBACK` и только затем `DROP chunks_vec` + запись `meta`
//!   (обычный `connect()` не должен превращаться в write-транзакцию);
//! * бэкфилл `indexed_at` (`mtime + 10`) для старых строк — молча.

use std::path::Path;
use std::sync::Once;

use rusqlite::{Connection, OptionalExtension};

use crate::error::{CoreError, Result};

/// Историческое имя ошибки слоя БД (все функции возвращают [`CoreError`]).
pub type DbError = CoreError;

/// Размерность CLIP-векторов (`hds/clip_index.py:CLIP_DIM`), таблица `images_vec`.
pub const CLIP_DIM: i64 = 512;

/// `SCHEMA` из `hds/db.py` — дословно (подставляется только `{dim}`).
pub const SCHEMA: &str = r#"
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
"#;

/// Строка схемы `chunks_vec`, которую Python вырезает, когда vec0 недоступен.
const CHUNKS_VEC_LINE: &str =
    "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_vec USING vec0(embedding float[{dim}]);\n";

static VEC_INIT: Once = Once::new();

/// Регистрирует vec0 на уровне sqlite (`sqlite3_auto_extension`), один раз на процесс.
///
/// Ловушка (спайк 1, `tools/parity/spikes/tests/spike1_db.rs`): прямой вызов
/// `sqlite_vec::sqlite3_vec_init()` модуль vec0 не регистрирует; работает только
/// auto-extension с `transmute`.
pub fn register_vec0() {
    VEC_INIT.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// True, если vec0 доступен в этом соединении (`SELECT vec_version()`).
pub fn vec_ok(conn: &Connection) -> bool {
    conn.query_row("SELECT vec_version()", [], |r| r.get::<_, String>(0))
        .is_ok()
}

/// True, если таблица `chunks_vec` существует (после `connect`).
pub fn has_vec(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name='chunks_vec'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Порт `hds/db.py:connect`: подключение + PRAGMA + схема + vec0.
///
/// Сообщения `[db] …` печатаются в stdout, как в Python-версии (формат сохранён).
pub fn connect(db_path: &Path, dim: i64) -> Result<Connection> {
    let dim = dim.max(1);
    if let Some(dir) = db_path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    register_vec0();
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;\n\
         PRAGMA synchronous=NORMAL;\n\
         PRAGMA foreign_keys=ON;\n\
         PRAGMA busy_timeout=30000;",
    )?;

    let vec_available = vec_ok(&conn);
    if !vec_available {
        println!(
            "[db] sqlite3 без поддержки загрузки расширений — векторный поиск \
             недоступен, работает только ключевой поиск (FTS5). Рекомендуется \
             Python из Homebrew: brew install python"
        );
    }

    conn.execute(
        "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT)",
        [],
    )?;
    let stored_dim: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key='vec_dim'", [], |r| r.get(0))
        .optional()?;
    let want = dim.to_string();
    let mut mismatch = stored_dim.as_deref() != Some(want.as_str());
    if mismatch && vec_available {
        // проба вставкой с откатом: размерность таблицы подтверждается реально
        let probe = vec![0u8; (dim as usize) * 4];
        conn.execute_batch("SAVEPOINT probe_sp")?;
        let ok = conn
            .execute(
                "INSERT OR REPLACE INTO chunks_vec(rowid, embedding) VALUES(1, ?1)",
                [&probe],
            )
            .is_ok();
        mismatch = !ok;
        conn.execute_batch("ROLLBACK TO probe_sp; RELEASE probe_sp")?;
    }
    if mismatch {
        conn.execute_batch("DROP TABLE IF EXISTS chunks_vec")?;
        if vec_available {
            println!(
                "[db] размерность эмбеддингов -> {}, векторная таблица пересоздана \
                 (запустите 'index --full' для повторной векторизации)",
                dim
            );
        }
    }

    let schema = if vec_available {
        SCHEMA.to_string()
    } else {
        SCHEMA.replace(CHUNKS_VEC_LINE, "")
    };
    conn.execute_batch(&schema.replace("{dim}", &want))?;

    if vec_available {
        conn.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS images_vec USING vec0(embedding float[{CLIP_DIM}])"
        ))?;
    }

    // бэкфилл: файлы, проиндексированные до введения indexed_at
    let old: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE status='indexed' AND indexed_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    if old > 0 {
        conn.execute(
            "UPDATE files SET indexed_at = COALESCE(mtime, 0) + 10 \
             WHERE status='indexed' AND indexed_at IS NULL",
            [],
        )?;
        println!("[db] indexed_at заполнен по mtime для {} старых записей", old);
    }

    if stored_dim.as_deref() != Some(want.as_str()) {
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES('vec_dim', ?1)",
            [&want],
        )?;
    }
    Ok(conn)
}

/// Строка таблицы `files` (порт `sqlite3.Row`: доступ по имени поля).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FileRow {
    pub id: i64,
    pub path: String,
    pub ext: Option<String>,
    pub kind: Option<String>,
    pub size: Option<i64>,
    pub mtime: Option<f64>,
    pub content_hash: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub chunk_count: Option<i64>,
    pub indexed_at: Option<f64>,
}

impl FileRow {
    /// Разбор строки `SELECT * FROM files …` (доступ по имени колонки).
    pub fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(FileRow {
            id: r.get("id")?,
            path: r.get("path")?,
            ext: r.get("ext")?,
            kind: r.get("kind")?,
            size: r.get("size")?,
            mtime: r.get("mtime")?,
            content_hash: r.get("content_hash")?,
            status: r.get("status")?,
            error: r.get("error")?,
            chunk_count: r.get("chunk_count")?,
            indexed_at: r.get("indexed_at")?,
        })
    }

    /// `row["status"] == "indexed"` с учётом `NULL`.
    pub fn is_indexed(&self) -> bool {
        self.status.as_deref() == Some("indexed")
    }

    /// `row["mtime"] or 0` — как в Python.
    pub fn mtime_or_zero(&self) -> f64 {
        self.mtime.unwrap_or(0.0)
    }
}

/// Порт `db.get_file_by_path`.
pub fn get_file_by_path(conn: &Connection, path: &str) -> Result<Option<FileRow>> {
    Ok(conn
        .query_row("SELECT * FROM files WHERE path=?1", [path], FileRow::from_row)
        .optional()?)
}

/// Порт `db.get_file_by_hash` (пустой хэш ⇒ `None`).
pub fn get_file_by_hash(conn: &Connection, chash: Option<&str>) -> Result<Option<FileRow>> {
    let h = match chash {
        Some(h) if !h.is_empty() => h,
        _ => return Ok(None),
    };
    Ok(conn
        .query_row(
            "SELECT * FROM files WHERE content_hash=?1 LIMIT 1",
            [h],
            FileRow::from_row,
        )
        .optional()?)
}

/// Порт `db.upsert_file`: `INSERT … ON CONFLICT(path) DO UPDATE` + id.
pub fn upsert_file(
    conn: &Connection,
    path: &str,
    ext: &str,
    kind: &str,
    size: i64,
    mtime: f64,
    content_hash: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO files(path, ext, kind, size, mtime, content_hash, status) \
         VALUES(?1,?2,?3,?4,?5,?6,'new') \
         ON CONFLICT(path) DO UPDATE SET \
           ext=excluded.ext, kind=excluded.kind, size=excluded.size, \
           mtime=excluded.mtime, content_hash=excluded.content_hash",
        rusqlite::params![path, ext, kind, size, mtime, content_hash],
    )?;
    get_file_by_path(conn, path)?
        .map(|r| r.id)
        .ok_or_else(|| CoreError::Other(format!("файл не найден после upsert: {path}")))
}

/// Порт `db.rename_path`.
pub fn rename_path(conn: &Connection, old_path: &str, new_path: &str) -> Result<()> {
    conn.execute(
        "UPDATE files SET path=?1 WHERE path=?2",
        rusqlite::params![new_path, old_path],
    )?;
    Ok(())
}

/// Порт `db.delete_file_data` (без `commit` — вызывающий решает про транзакцию).
pub fn delete_file_data(conn: &Connection, file_id: i64) -> Result<()> {
    let ids: Vec<i64> = {
        let mut st = conn.prepare("SELECT id FROM chunks WHERE file_id=?1")?;
        let rows = st.query_map([file_id], |r| r.get::<_, i64>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for cid in ids {
        conn.execute("DELETE FROM chunks_fts WHERE rowid=?1", [cid])?;
        conn.execute("DELETE FROM chunks_vec WHERE rowid=?1", [cid])?;
    }
    conn.execute("DELETE FROM chunks WHERE file_id=?1", [file_id])?;
    // CLIP-вектор картинки (rowid = files.id); таблицы может не быть в старых БД
    let _ = conn.execute("DELETE FROM images_vec WHERE rowid=?1", [file_id]);
    Ok(())
}

/// Порт `db.remove_path` (удаляет запись и её данные, если файл был в индексе).
pub fn remove_path(conn: &Connection, path: &str) -> Result<bool> {
    let row = match get_file_by_path(conn, path)? {
        Some(r) => r,
        None => return Ok(false),
    };
    delete_file_data(conn, row.id)?;
    conn.execute("DELETE FROM files WHERE id=?1", [row.id])?;
    Ok(true)
}

/// Порт `db.add_chunk`: `chunks` (отображаемый текст) + `chunks_fts` (лемматизированный).
///
/// Лемматизация вынесена наружу (Python зовёт `lemmatizer.normalize` внутри
/// `add_chunk`; здесь текст FTS передаётся параметром — за него отвечает
/// `Lemmatizer` в `hds-index`, вариант A из `SPIKES.md` §10).
pub fn add_chunk(
    conn: &Connection,
    file_id: i64,
    ord_: i64,
    page: Option<i64>,
    t_start: Option<f64>,
    t_end: Option<f64>,
    text: &str,
    fts_text: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO chunks(file_id, ord, page, t_start, t_end, text) VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![file_id, ord_, page, t_start, t_end, text],
    )?;
    let cid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO chunks_fts(rowid, text) VALUES(?1,?2)",
        rusqlite::params![cid, fts_text],
    )?;
    Ok(cid)
}

/// Порт `db.add_vector` (`struct.pack("<%df", *vector)` → тот же blob).
pub fn add_vector(conn: &Connection, chunk_id: i64, vector_blob: &[u8]) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO chunks_vec(rowid, embedding) VALUES(?1,?2)",
        rusqlite::params![chunk_id, vector_blob],
    )?;
    Ok(())
}

/// Порт `db.finish_file`: `indexed` пишет `indexed_at`/`chunk_count`, иначе статус+ошибку.
///
/// `indexed_at` передаётся вызывающим (`time.time()` в Python) — так проще тестировать.
pub fn finish_file(
    conn: &Connection,
    file_id: i64,
    status: &str,
    error: Option<&str>,
    chunks: Option<i64>,
    indexed_at: f64,
) -> Result<()> {
    if status == "indexed" {
        conn.execute(
            "UPDATE files SET status=?1, error=NULL, indexed_at=?2, chunk_count=?3 WHERE id=?4",
            rusqlite::params![status, indexed_at, chunks.unwrap_or(0), file_id],
        )?;
    } else {
        conn.execute(
            "UPDATE files SET status=?1, error=?2 WHERE id=?3",
            rusqlite::params![status, error, file_id],
        )?;
    }
    Ok(())
}

/// Порт `db.all_files` — пары `(id, path)` для prune.
pub fn all_files(conn: &Connection) -> Result<Vec<(i64, String)>> {
    let mut st = conn.prepare("SELECT id, path FROM files")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Сводка `db.stats` (для `status`; поля как в Python-версии).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub by_kind: Vec<(Option<String>, i64)>,
    pub by_status: Vec<(Option<String>, i64)>,
    pub chunks: i64,
    pub last_indexed_at: Option<f64>,
    pub errors: Vec<(String, Option<String>)>,
}

/// Порт `db.stats`.
pub fn stats(conn: &Connection) -> Result<Stats> {
    let mut by_kind = Vec::new();
    {
        let mut st = conn.prepare("SELECT kind, COUNT(*) FROM files GROUP BY kind")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)))?;
        for r in rows {
            by_kind.push(r?);
        }
    }
    let mut by_status = Vec::new();
    {
        let mut st = conn.prepare("SELECT status, COUNT(*) FROM files GROUP BY status")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?)))?;
        for r in rows {
            by_status.push(r?);
        }
    }
    let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let last_indexed_at: Option<f64> =
        conn.query_row("SELECT MAX(indexed_at) FROM files", [], |r| r.get(0))?;
    let mut errors = Vec::new();
    {
        let mut st = conn.prepare("SELECT path, error FROM files WHERE status='error' LIMIT 10")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        for r in rows {
            errors.push(r?);
        }
    }
    Ok(Stats {
        by_kind,
        by_status,
        chunks,
        last_indexed_at,
        errors,
    })
}

