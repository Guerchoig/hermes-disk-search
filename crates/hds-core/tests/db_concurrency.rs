//! M4 T1 (`PLAN_M4_CONCURRENCY_SLA.md` §3.4/§5): БД под конкурентность.
//!
//! * `open_ro` — read-only читатель: не пишет, работает под активным писателем (WAL);
//! * стресс «5 параллельных читателей + писатель» — без `SQLITE_BUSY`/«database is locked»;
//! * ключи `db.*` — профильные дефолты (server: 5000/true; desktop: 30000/false)
//!   и явное переопределение из конфига.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use hds_core::config::{self, Profile};
use hds_core::db::{self, DbOptions};
use serde_yaml::Value;

fn tmp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hds-db-conc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// База с одной записью `files` (id=1) и одним чанком — как в `db_crud`.
fn make_db(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("index.db");
    let conn = db::connect(&path, 1024).expect("connect");
    db::upsert_file(&conn, "a.txt", "txt", "text", 10, 1.0, None, None).unwrap();
    db::add_chunk(
        &conn,
        1,
        0,
        None,
        None,
        None,
        "исходный чанк",
        "исходный чанк",
    )
    .unwrap();
    drop(conn);
    path
}

/// Читатель (`open_ro`) работает, пока писатель держит write-транзакцию (WAL),
/// наследует WAL из файла и отказывается писать.
#[test]
fn open_ro_reads_while_writer_holds_lock() {
    let dir = tmp_dir("ro-writer");
    let path = make_db(&dir);
    let writer = db::open_rw(&path, 1024, &DbOptions::default()).expect("open_rw");
    writer.execute_batch("BEGIN IMMEDIATE;").unwrap();
    writer
        .execute(
            "INSERT INTO chunks(file_id, ord, text) VALUES(1, 1, 'в транзакции')",
            [],
        )
        .unwrap();

    let ro = db::open_ro(&path, &DbOptions::default()).expect("open_ro");
    let jm: String = ro
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(jm.to_ascii_lowercase(), "wal", "читатель наследует WAL");

    let n: i64 = ro
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert!(n >= 1, "чтение под активной write-транзакцией прошло");

    let w = ro.execute(
        "INSERT INTO chunks(file_id, ord, text) VALUES(1, 2, 'запись запрещена')",
        [],
    );
    assert!(w.is_err(), "read-only соединение обязано отказать в записи");

    writer.execute_batch("ROLLBACK;").unwrap();
}

/// Стресс (§3.4): 5 параллельных читателей + один писатель → ни одного
/// `SQLITE_BUSY`/«database is locked».
#[test]
fn stress_five_readers_plus_writer_no_busy() {
    let dir = tmp_dir("stress");
    let path = make_db(&dir);
    let opts = DbOptions {
        busy_timeout_ms: 2000,
        ..DbOptions::default()
    };
    stress_body(&path, &opts);
}

fn stress_body(path: &std::path::Path, opts: &DbOptions) {
    let wpath = path.to_path_buf();
    let wopts = opts.clone();
    let writer = thread::spawn(move || {
        let conn = db::open_rw(&wpath, 1024, &wopts).expect("open_rw");
        for i in 0..200 {
            conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
            db::add_chunk(
                &conn,
                1,
                i + 1,
                None,
                None,
                None,
                &format!("w{i}"),
                &format!("w{i}"),
            )
            .unwrap();
            conn.execute_batch("COMMIT;").unwrap();
            thread::sleep(Duration::from_micros(200));
        }
    });

    let busy = Arc::new(AtomicUsize::new(0));
    let mut readers = Vec::new();
    for _ in 0..5 {
        let p = path.to_path_buf();
        let o = opts.clone();
        let err = Arc::clone(&busy);
        readers.push(thread::spawn(move || {
            let conn = match db::open_ro(&p, &o) {
                Ok(c) => c,
                Err(_) => {
                    err.fetch_add(1, Ordering::SeqCst);
                    return;
                }
            };
            for _ in 0..400 {
                if let Err(e) = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| {
                    row.get::<_, i64>(0)
                }) {
                    let s = e.to_string().to_lowercase();
                    if s.contains("locked") || s.contains("busy") {
                        err.fetch_add(1, Ordering::SeqCst);
                    }
                }
                thread::sleep(Duration::from_micros(200));
            }
        }));
    }

    writer.join().unwrap();
    for r in readers {
        r.join().unwrap();
    }
    assert_eq!(
        busy.load(Ordering::SeqCst),
        0,
        "обнаружены SQLITE_BUSY/«database is locked»"
    );
}

/// `open_ro` не создаёт отсутствующую базу — явная ошибка (читатель без DDL).
#[test]
fn open_ro_missing_db_is_explicit_error() {
    let dir = tmp_dir("missing");
    let path = dir.join("index.db");
    assert!(
        db::open_ro(&path, &DbOptions::default()).is_err(),
        "open_ro на отсутствующей базе обязан вернуть ошибку"
    );
}

/// Ключи `db.*`: профильные дефолты (server 5000/true/wal; desktop 30000/false)
/// и явное переопределение из конфига.
#[test]
fn db_keys_profile_defaults_and_overrides() {
    let desktop = serde_yaml::from_str::<Value>("").unwrap();
    assert_eq!(
        config::db_busy_timeout_ms(&desktop),
        30_000,
        "desktop — прежнее зашитое значение"
    );
    assert!(!config::db_read_only_conns(&desktop));
    assert_eq!(config::db_journal_mode(&desktop), "wal");

    let server: Value = serde_yaml::from_str("profile: server").unwrap();
    assert_eq!(config::db_busy_timeout_ms(&server), 5000);
    assert!(config::db_read_only_conns(&server));
    assert_eq!(config::db_busy_timeout_default(Profile::Server), 5000);
    assert_eq!(config::db_busy_timeout_default(Profile::Desktop), 30_000);
    assert!(config::db_read_only_conns_default(Profile::Server));
    assert!(!config::db_read_only_conns_default(Profile::Desktop));

    let override_bt: Value = serde_yaml::from_str("db:\n  busy_timeout_ms: 7000").unwrap();
    assert_eq!(config::db_busy_timeout_ms(&override_bt), 7000);

    let override_ro: Value =
        serde_yaml::from_str("profile: server\ndb:\n  read_only_conns: false").unwrap();
    assert!(!config::db_read_only_conns(&override_ro));

    let override_jm: Value = serde_yaml::from_str("db:\n  journal_mode: delete").unwrap();
    assert_eq!(config::db_journal_mode(&override_jm), "delete");
}
