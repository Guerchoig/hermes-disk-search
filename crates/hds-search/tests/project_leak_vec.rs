//! Векторные ветки фильтра проектов (M1 T3): `vec_search_ids` (chunks_vec)
//! и `clip_search_ids` (images_vec, rowid = files.id). Фикстура — как в
//! `project_leak.rs`; векторы произвольные, важен только состав выдачи.

use hds_core::db;
use hds_search::{clip_search_ids, vec_search_ids};
use std::path::PathBuf;

fn tmp_db(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-leakv-{tag}-{}-{nanos}", std::process::id()));
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

fn projects(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Два файла в разных проектах, dim=4.
fn setup(tag: &str) -> (rusqlite::Connection, i64, i64) {
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
    let c1 = db::add_chunk(&conn, s, 0, Some(1), None, None, "текст один", "текст один").unwrap();
    let c2 = db::add_chunk(&conn, d, 0, Some(1), None, None, "текст два", "текст два").unwrap();
    db::add_vector(&conn, c1, &blob(&[1.0, 0.0, 0.0, 0.0])).unwrap();
    db::add_vector(&conn, c2, &blob(&[0.0, 1.0, 0.0, 0.0])).unwrap();
    (conn, s, d)
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
fn vec_branch_filters_by_project() {
    let (conn, _s, _d) = setup("vec");
    let q = blob(&[1.0, 0.0, 0.0, 0.0]); // ближе к Sales-чанку
    let all = vec_search_ids(&conn, &q, None, None, 10).unwrap();
    assert_eq!(all.len(), 2);
    let sales = vec_search_ids(&conn, &q, None, Some(&projects(&["Sales"])), 10).unwrap();
    assert_eq!(sales.len(), 1);
    assert!(path_of(&conn, sales[0].0).contains("Sales"), "утечка");
    let docs = vec_search_ids(&conn, &q, None, Some(&projects(&["Docs"])), 10).unwrap();
    assert_eq!(docs.len(), 1);
    assert!(path_of(&conn, docs[0].0).contains("Docs"), "утечка");
}

/// Вектор 512-мерный: первые две координаты a/b, остальное — нули.
fn vec_of_512(a: f32, b: f32) -> Vec<f32> {
    let mut v = vec![0.0f32; 512];
    v[0] = a;
    v[1] = b;
    v
}

#[test]
fn clip_branch_filters_by_project() {
    let conn = db::connect(&tmp_db("clip"), 4).unwrap();
    let s = db::upsert_file(
        &conn,
        "D:/Work/Sales/photo.png",
        ".png",
        "image",
        10,
        1.0,
        Some("h1"),
        Some("Sales"),
    )
    .unwrap();
    let d = db::upsert_file(
        &conn,
        "D:/Work/Docs/diagram.png",
        ".png",
        "image",
        10,
        1.0,
        Some("h2"),
        Some("Docs"),
    )
    .unwrap();
    let clip_s = blob(&vec_of_512(1.0, 0.0));
    let clip_d = blob(&vec_of_512(0.0, 1.0));
    db::add_image_vector(&conn, s, &clip_s).unwrap();
    db::add_image_vector(&conn, d, &clip_d).unwrap();

    let q = clip_s.clone();
    let all = clip_search_ids(&conn, &q, None, 10).unwrap();
    assert_eq!(all.len(), 2);
    let sales = clip_search_ids(&conn, &q, Some(&projects(&["Sales"])), 10).unwrap();
    assert_eq!(sales.len(), 1);
    assert_eq!(sales[0].0, s);
    let docs = clip_search_ids(&conn, &q, Some(&projects(&["Docs"])), 10).unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].0, d);
}
