//! CRUD-паритет `hds-core::db` с `hds/db.py`: upsert/rename/hash/moved,
//! чанки + FTS + векторы, `finish_file`, `delete_file_data`, `remove_path`, `stats`.

use std::path::PathBuf;

use hds_core::db::{self, FileRow};

fn tmp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hds-core-crud-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("tmp");
    dir.join("index.db")
}

fn f32_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[test]
fn crud_round_trip_matches_python() {
    let p = tmp_db("crud");
    let conn = db::connect(&p, 1024).expect("connect");

    // upsert + get_file_by_path (первый раз — 'new' по DEFAULT)
    let fid = db::upsert_file(&conn, "D:/x/a.md", ".md", "text", 100, 5.0, Some("hash1")).unwrap();
    let row: FileRow = db::get_file_by_path(&conn, "D:/x/a.md").unwrap().unwrap();
    assert_eq!(row.id, fid);
    assert_eq!(row.status.as_deref(), Some("new"));
    assert_eq!(row.size, Some(100));
    assert_eq!(row.content_hash.as_deref(), Some("hash1"));

    // повторный upsert обновляет поля, id сохраняется
    let fid2 = db::upsert_file(&conn, "D:/x/a.md", ".md", "text", 120, 6.0, Some("hash2")).unwrap();
    assert_eq!(fid, fid2, "ON CONFLICT(path) сохраняет id");
    let row = db::get_file_by_path(&conn, "D:/x/a.md").unwrap().unwrap();
    assert_eq!(row.size, Some(120));

    // get_file_by_hash
    assert!(db::get_file_by_hash(&conn, Some("hash2")).unwrap().is_some());
    assert!(db::get_file_by_hash(&conn, Some("nope")).unwrap().is_none());
    assert!(db::get_file_by_hash(&conn, None).unwrap().is_none());

    // add_chunk: отображаемый текст и лемматизированный FTS — разные поля
    let cid = db::add_chunk(
        &conn,
        fid,
        0,
        None,
        None,
        None,
        "Настройка скрипта",
        "настройка скрипт",
    )
    .unwrap();
    db::add_vector(&conn, cid, &f32_blob(&[0.0f32; 1024])).unwrap();
    db::finish_file(&conn, fid, "indexed", None, Some(1), 123.5).unwrap();

    let row = db::get_file_by_path(&conn, "D:/x/a.md").unwrap().unwrap();
    assert!(row.is_indexed());
    assert_eq!(row.chunk_count, Some(1));
    assert_eq!(row.indexed_at, Some(123.5));
    assert_eq!(row.error, None);

    // FTS-запрос по лемме находит чанк
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks_fts WHERE chunks_fts MATCH ?1",
            ["\"настройка\""],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1, "FTS должен находить лемматизированный текст");

    // rename_path (порт moved)
    db::rename_path(&conn, "D:/x/a.md", "D:/x/b.md").unwrap();
    assert!(db::get_file_by_path(&conn, "D:/x/a.md").unwrap().is_none());
    assert!(db::get_file_by_path(&conn, "D:/x/b.md").unwrap().is_some());

    // stats
    let st = db::stats(&conn).unwrap();
    assert_eq!(st.chunks, 1);
    assert!(st.by_kind.iter().any(|(k, c)| k.as_deref() == Some("text") && *c == 1));

    // remove_path: удаляет запись, чанки и FTS/vec (питоновская семантика)
    assert!(db::remove_path(&conn, "D:/x/b.md").unwrap());
    assert!(!db::remove_path(&conn, "D:/x/b.md").unwrap());
    assert_eq!(db::all_files(&conn).unwrap().len(), 0);
    let chunks_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(chunks_left, 0);
    let fts_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fts_left, 0);
    let vec_left: i64 = conn
        .query_row("SELECT COUNT(*) FROM chunks_vec", [], |r| r.get(0))
        .unwrap();
    assert_eq!(vec_left, 0);
}

#[test]
fn finish_file_error_keeps_status_and_message() {
    let p = tmp_db("err");
    let conn = db::connect(&p, 1024).expect("connect");
    let fid = db::upsert_file(&conn, "D:/x/bad.docx", ".docx", "docx", 1, 1.0, None).unwrap();
    db::finish_file(&conn, fid, "error", Some("BadZipFile"), None, 0.0).unwrap();
    let row = db::get_file_by_path(&conn, "D:/x/bad.docx").unwrap().unwrap();
    assert_eq!(row.status.as_deref(), Some("error"));
    assert_eq!(row.error.as_deref(), Some("BadZipFile"));
    assert_eq!(row.indexed_at, None);
}
