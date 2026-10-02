//! Тесты схемы/подключения `hds-core::db` — паритет с `hds/db.py` (§3.1 плана).
//!
//! Проверяем:
//! * `PRAGMA` (`journal_mode=WAL`, `foreign_keys=ON`, `busy_timeout=30000`);
//! * наличие всех объектов схемы Python-версии (включая `images_vec`);
//! * `vec0` через `sqlite3_auto_extension` (см. спайк 1) и `meta.vec_dim`;
//! * повторный `connect` не пересоздаёт таблицу при совпадающей размерности;
//! * `#[ignore]`-тесты: Python-версия читает БД, созданную Rust (и наоборот).

use std::path::PathBuf;

use hds_core::db;

fn tmp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hds-core-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmp");
    dir.join("index.db")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn pragmas_objects_and_vec0_match_python() {
    let p = tmp_db("schema");
    let conn = db::connect(&p, 1024).expect("connect");

    let jm: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(jm.to_lowercase(), "wal", "journal_mode");
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1, "foreign_keys");
    let bt: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bt, 30000, "busy_timeout");

    assert!(db::has_vec(&conn), "chunks_vec должен существовать");
    let v: String = conn
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .unwrap();
    assert!(v.contains("0.1.9"), "vec_version={v}");

    let mut names: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type IN ('table','index') \
             AND name NOT LIKE 'sqlite_%'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    names.sort();
    for want in [
        "files",
        "chunks",
        "chunks_fts",
        "chunks_vec",
        "images_vec",
        "meta",
        "idx_files_kind",
        "idx_files_hash",
        "idx_files_status",
        "idx_chunks_file",
    ] {
        assert!(
            names.iter().any(|n| n == want),
            "нет объекта {want}: {names:?}"
        );
    }

    let dim: String = conn
        .query_row("SELECT value FROM meta WHERE key='vec_dim'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(dim, "1024", "meta.vec_dim");
}

#[test]
fn second_connect_keeps_vec_table_and_dim() {
    let p = tmp_db("reconnect");
    {
        let conn = db::connect(&p, 1024).expect("connect #1");
        db::upsert_file(&conn, "D:/x/a.txt", ".txt", "text", 10, 1.0, Some("h")).unwrap();
    }
    let conn = db::connect(&p, 1024).expect("connect #2");
    assert!(
        db::has_vec(&conn),
        "chunks_vec не должен пропасть при повторном connect"
    );
    let dim: String = conn
        .query_row("SELECT value FROM meta WHERE key='vec_dim'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(dim, "1024");
    let files: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files, 1);
}

/// Python-версия открывает БД, созданную Rust: читает схему, пишет чанк+FTS+vec.
///
/// Запуск: `cargo test -p hds-core --test db_schema -- --ignored --nocapture`
#[test]
#[ignore = "требует .venv (Python-версия читает Rust-БД)"]
fn python_reads_rust_db() {
    let py = repo_root().join(".venv").join("Scripts").join("python.exe");
    if !py.exists() {
        println!("пропуск: нет {}", py.display());
        return;
    }
    let p = tmp_db("py-reads-rust");
    {
        let conn = db::connect(&p, 1024).expect("connect");
        conn.execute(
            "INSERT INTO files(path, ext, kind, size, mtime, content_hash, status) \
             VALUES('D:/x/инструкция.md','.md','text',10,1.0,'abc','new')",
            [],
        )
        .unwrap();
    }
    let script = format!(
        "import sys; sys.path.insert(0, r'{root}')\n\
         from hds import db as d\n\
         c = d.connect(r'{db}', 1024)\n\
         fid = c.execute('SELECT id FROM files').fetchone()[0]\n\
         cid = d.add_chunk(c, fid, 0, None, None, None, 'Настройка скрипта')\n\
         d.add_vector(c, cid, b'\\x00'*4096)\n\
         d.finish_file(c, fid, 'indexed', None, 1)\n\
         c.commit()\n\
         n = c.execute('SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH ?', ('настройка',)).fetchone()[0]\n\
         print('PY_OK', n, c.execute('SELECT vec_version()').fetchone()[0])\n",
        root = repo_root().display(),
        db = p.display()
    );
    let out = std::process::Command::new(&py)
        .args(["-c", &script])
        .output()
        .expect("python");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("PY_OK 1 0.1.9"),
        "python не прочитал Rust-БД\nstdout: {stdout}\nstderr: {stderr}"
    );
    println!("{}", stdout.trim());
}

/// Rust читает боевую БД Python-версии (read-only) — регресс спайка 1.
///
/// БД: env `HDS_DB`, по умолчанию `D:\\hermes-disk-search-db\\index.db`.
#[test]
#[ignore = "требует боевую index.db (см. HDS_DB)"]
fn rust_reads_python_db() {
    let db_path = std::env::var("HDS_DB")
        .unwrap_or_else(|_| r"D:\hermes-disk-search-db\index.db".to_string());
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    db::register_vec0();
    let conn = rusqlite::Connection::open_with_flags(&db_path, flags).expect("боевая БД");
    let files: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    let chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    let v: String = conn
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .unwrap();
    println!("files={files} chunks={chunks} vec={v} ({db_path})");
    assert!(files > 0 && chunks > 0);
}
