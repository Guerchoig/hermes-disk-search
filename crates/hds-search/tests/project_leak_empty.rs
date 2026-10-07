//! Some(пусто) = ничего не выдать (M1 T4): пользователь без проектов в
//! server-режиме не должен видеть НИЧЕГО (это защита, а не «без фильтра»).

use hds_core::db;
use hds_index::Lemmatizer;
use hds_search::{fts_search_ids, vec_search_ids};

struct LowerLem;
impl Lemmatizer for LowerLem {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>, hds_core::error::CoreError> {
        Ok(texts.iter().map(|t| t.to_lowercase()).collect())
    }
}

#[test]
fn fts_empty_projects_show_nothing() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-leak0-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let conn = db::connect(&dir.join("index.db"), 4).unwrap();
    let f = db::upsert_file(
        &conn,
        "D:/x/a.txt",
        ".txt",
        "text",
        1,
        1.0,
        Some("h"),
        Some("Sales"),
    )
    .unwrap();
    let c = db::add_chunk(
        &conn,
        f,
        0,
        None,
        None,
        None,
        "маркер текст",
        "маркер текст",
    )
    .unwrap();
    db::add_vector(&conn, c, &[0u8; 16]).unwrap();

    let none = fts_search_ids(&conn, &LowerLem, "маркер", 10, None, None);
    assert_eq!(none.len(), 1, "без фильтра виден");
    let empty: Vec<String> = Vec::new();
    let got = fts_search_ids(&conn, &LowerLem, "маркер", 10, None, Some(&empty));
    assert!(got.is_empty(), "Some(пусто) не должен выдавать ничего");
    let vec_empty = vec_search_ids(&conn, &[0u8; 16], None, Some(&empty), 10).unwrap();
    assert!(vec_empty.is_empty(), "vec: Some(пусто) = ничего");
}
