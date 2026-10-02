//! Тесты `forget` и `status`.

mod common;

use common::TempDir;
use hds_cli::cmd::status::{stats_json, stats_text};
use hds_core::db;

#[test]
fn forget_removes_file_and_chunks() {
    let td = TempDir::new("forget");
    let path = td.join("index.db");
    let conn = db::connect(&path, 1024).unwrap();
    conn.execute(
        "INSERT INTO files(path, status) VALUES('f.txt', 'indexed')",
        [],
    )
    .unwrap();
    db::add_chunk(&conn, 1, 0, None, None, None, "t", "t").unwrap();

    assert!(hds_cli::cmd::forget::forget(&conn, "f.txt").unwrap());
    let files: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    let chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!((files, chunks), (0, 0), "запись и её данные удалены");

    assert!(!hds_cli::cmd::forget::forget(&conn, "нет-такого.txt").unwrap());
}

#[test]
fn status_fields_match_python_shape() {
    let td = TempDir::new("status");
    let path = td.join("index.db");
    let conn = db::connect(&path, 1024).unwrap();
    conn.execute(
        "INSERT INTO files(path, kind, status) VALUES('a.md', 'text', 'indexed')",
        [],
    )
    .unwrap();
    db::add_chunk(&conn, 1, 0, None, None, None, "hello", "hello").unwrap();
    conn.execute(
        "INSERT INTO files(path, kind, status, error) VALUES('b.pdf', 'pdf', 'error', 'boom')",
        [],
    )
    .unwrap();

    let st = db::stats(&conn).unwrap();
    let j = stats_json(&st);
    assert!(j.get("by_kind").unwrap().is_array());
    assert!(j.get("by_status").unwrap().is_array());
    assert_eq!(j.get("chunks").unwrap().as_i64(), Some(1));
    assert!(j.get("last_indexed_at").is_some());
    assert_eq!(j.get("errors").unwrap().as_array().unwrap().len(), 1);

    let text = stats_text(&st);
    assert!(text.contains("Чанков всего: 1"));
    assert!(text.contains("Последние ошибки:"));
    assert!(text.contains("b.pdf :: boom"));
}
