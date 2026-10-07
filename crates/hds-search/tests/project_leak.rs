//! Тесты на утечку между проектами (M1 T3, приёмка §5): при `projects=["Sales"]`
//! выдача НЕ содержит файлов других проектов — во всех ветках (FTS/vec/CLIP)
//! и через `search()` целиком. Тесты подключаются ДО auth (T4).
//!
//! Векторы произвольные (4-мерные): для проверки ФИЛЬТРА важен только состав
//! выдачи. CLIP-векторы — 512-мерные (CLIP_DIM). Vec/CLIP — `project_leak_vec.rs`.

use hds_core::db;
use hds_index::Lemmatizer;
use hds_search::{fts_search_ids, search};
use std::path::PathBuf;

struct LowerLem;
impl Lemmatizer for LowerLem {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>, hds_core::error::CoreError> {
        Ok(texts.iter().map(|t| t.to_lowercase()).collect())
    }
}

fn tmp_db(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-leak-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("index.db")
}

fn blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Фикстура: два файла в разных проектах, у каждого чанк + вектор (dim 4).
fn setup(tag: &str) -> rusqlite::Connection {
    let conn = db::connect(&tmp_db(tag), 4).unwrap();
    let s = db::upsert_file(
        &conn,
        "D:/Work/Sales/contract.pdf",
        ".pdf",
        "pdf",
        10,
        1.0,
        Some("h1"),
        Some("Sales"),
    )
    .unwrap();
    let d = db::upsert_file(
        &conn,
        "D:/Work/Docs/manual.pdf",
        ".pdf",
        "pdf",
        10,
        1.0,
        Some("h2"),
        Some("Docs"),
    )
    .unwrap();
    let c1 = db::add_chunk(
        &conn,
        s,
        0,
        Some(1),
        None,
        None,
        "договор поставки маркер",
        "договор поставки маркер",
    )
    .unwrap();
    let c2 = db::add_chunk(
        &conn,
        d,
        0,
        Some(1),
        None,
        None,
        "руководство пользователя маркер",
        "руководство пользователя маркер",
    )
    .unwrap();
    db::add_vector(&conn, c1, &blob(&[1.0, 0.0, 0.0, 0.0])).unwrap();
    db::add_vector(&conn, c2, &blob(&[0.0, 1.0, 0.0, 0.0])).unwrap();
    db::finish_file(&conn, s, "indexed", None, Some(1), 1.0).unwrap();
    db::finish_file(&conn, d, "indexed", None, Some(1), 1.0).unwrap();
    conn
}

fn projects(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn path_of(conn: &rusqlite::Connection, cid: i64) -> String {
    conn.query_row(
        "SELECT f.path FROM chunks c JOIN files f ON f.id=c.file_id WHERE c.id=?1",
        [cid],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn fts_branch_filters_by_project() {
    let conn = setup("fts");
    // Без фильтра — оба файла.
    let all = fts_search_ids(&conn, &LowerLem, "маркер", 10, None, None);
    assert_eq!(all.len(), 2, "без фильтра видны оба");
    // Фильтр Sales — только чанк Sales-файла.
    let sales = fts_search_ids(
        &conn,
        &LowerLem,
        "маркер",
        10,
        None,
        Some(&projects(&["Sales"])),
    );
    assert_eq!(sales.len(), 1);
    assert!(path_of(&conn, sales[0]).contains("Sales"), "утечка");
    // Фильтр Docs — только Docs.
    let docs = fts_search_ids(
        &conn,
        &LowerLem,
        "маркер",
        10,
        None,
        Some(&projects(&["Docs"])),
    );
    assert_eq!(docs.len(), 1);
    assert!(path_of(&conn, docs[0]).contains("Docs"), "утечка");
}

#[test]
fn search_top_level_filters_by_project() {
    let conn = setup("search");
    // emb=None → только FTS-ветка; clip выключен явно.
    let cfg: hds_core::config::Config = serde_yaml::from_str("index: {clip: false}").unwrap();
    let no_filter = search(&conn, None, &LowerLem, &cfg, "маркер", None, None, 10);
    assert_eq!(no_filter.len(), 2);
    let sales = search(
        &conn,
        None,
        &LowerLem,
        &cfg,
        "маркер",
        None,
        Some(&projects(&["Sales"])),
        10,
    );
    assert_eq!(sales.len(), 1);
    assert!(sales[0].path.contains("Sales"), "утечка: {}", sales[0].path);
    let docs = search(
        &conn,
        None,
        &LowerLem,
        &cfg,
        "маркер",
        None,
        Some(&projects(&["Docs"])),
        10,
    );
    assert_eq!(docs.len(), 1);
    assert!(docs[0].path.contains("Docs"), "утечка: {}", docs[0].path);
}
