//! Тесты `reindex-fts`: `chunks_fts` перестроен, `meta.fts_normalized='1'`.

mod common;

use common::TempDir;
use hds_cli::cmd::reindex_fts::reindex_fts;
use hds_core::db;
use hds_index::sidecar::tokens_joined;
use hds_index::TokenLemmatizer;

#[test]
fn reindex_fts_rebuilds_and_sets_meta() {
    let td = TempDir::new("fts");
    let path = td.join("index.db");
    let conn = db::connect(&path, 1024).unwrap();
    conn.execute(
        "INSERT INTO files(path, status) VALUES('f.txt', 'indexed')",
        [],
    )
    .unwrap();
    // чанки с «мусорным» FTS-текстом, как после старой версии без лемматизации
    let cid1 = db::add_chunk(&conn, 1, 0, None, None, None, "Привет мир", "junk").unwrap();
    let cid2 = db::add_chunk(&conn, 1, 1, None, None, None, "second CHUNK", "junk2").unwrap();

    let n = reindex_fts(&conn, &TokenLemmatizer, 0).unwrap();
    assert_eq!(n, 2, "перестроены оба чанка");

    let t1: String = conn
        .query_row("SELECT text FROM chunks_fts WHERE rowid=?1", [cid1], |r| r.get(0))
        .unwrap();
    assert_eq!(t1, tokens_joined("Привет мир"));
    let t2: String = conn
        .query_row("SELECT text FROM chunks_fts WHERE rowid=?1", [cid2], |r| r.get(0))
        .unwrap();
    assert_eq!(t2, tokens_joined("second CHUNK"));

    let meta: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='fts_normalized'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(meta, "1");
}

#[test]
fn reindex_fts_empty_index_is_noop() {
    let td = TempDir::new("fts-empty");
    let path = td.join("index.db");
    let conn = db::connect(&path, 1024).unwrap();
    let n = reindex_fts(&conn, &TokenLemmatizer, 0).unwrap();
    assert_eq!(n, 0);
    let has_meta: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key='fts_normalized'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_meta, 0, "пустой индекс не должен ставить отметку");
}
