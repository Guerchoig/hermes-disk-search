//! Тесты БД для проектов (M1 T2): `files.project` хранится/обновляется,
//! файл вне корней — NULL, старая схема (без `project`) дропается при `connect`.

use hds_core::db;

fn tmp_db(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hds-db-project-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("index.db")
}

#[test]
fn db_stores_and_updates_project() {
    let p = tmp_db("store");
    let conn = db::connect(&p, 1024).unwrap();
    let fid = db::upsert_file(
        &conn,
        "D:/Work/Sales/q1.xlsx",
        ".xlsx",
        "xlsx",
        10,
        1.0,
        Some("h"),
        Some("Sales"),
    )
    .unwrap();
    let row = db::get_file_by_path(&conn, "D:/Work/Sales/q1.xlsx")
        .unwrap()
        .unwrap();
    assert_eq!(row.id, fid);
    assert_eq!(row.project.as_deref(), Some("Sales"));
    // Повторный upsert (файл переехал в другой корень) — проект обновляется.
    db::upsert_file(
        &conn,
        "D:/Work/Sales/q1.xlsx",
        ".xlsx",
        "xlsx",
        10,
        1.0,
        Some("h"),
        Some("Docs"),
    )
    .unwrap();
    let row = db::get_file_by_path(&conn, "D:/Work/Sales/q1.xlsx")
        .unwrap()
        .unwrap();
    assert_eq!(row.project.as_deref(), Some("Docs"));
    // Вне корней — NULL.
    db::upsert_file(
        &conn,
        "E:/other/a.txt",
        ".txt",
        "text",
        1,
        1.0,
        Some("h2"),
        None,
    )
    .unwrap();
    let row = db::get_file_by_path(&conn, "E:/other/a.txt")
        .unwrap()
        .unwrap();
    assert_eq!(row.project, None);
}

#[test]
fn old_schema_without_project_is_dropped() {
    let p = tmp_db("olddb");
    // Строим базу СТАРОЙ схемы (без project) руками.
    let conn = rusqlite::Connection::open(&p).unwrap();
    conn.execute_batch(
        "CREATE TABLE files(id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, ext TEXT, \
         kind TEXT, size INTEGER, mtime REAL, content_hash TEXT, status TEXT DEFAULT 'new', \
         error TEXT, chunk_count INTEGER DEFAULT 0, indexed_at REAL);
         INSERT INTO files(path) VALUES('D:/old/a.txt');
         CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);",
    )
    .unwrap();
    drop(conn);
    // connect() дропает старую схему и создаёт новую с project.
    let conn = db::connect(&p, 1024).unwrap();
    let cnt: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cnt, 0, "старые записи дропнуты");
    let has_project: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('files') WHERE name='project'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_project, 1);
    // Новая запись пишется нормально.
    db::upsert_file(
        &conn,
        "D:/new/a.txt",
        ".txt",
        "text",
        1,
        1.0,
        None,
        Some("D:"),
    )
    .unwrap();
}

#[test]
fn project_survives_reopen() {
    let p = tmp_db("reopen");
    {
        let conn = db::connect(&p, 1024).unwrap();
        db::upsert_file(
            &conn,
            "D:/x/a.txt",
            ".txt",
            "text",
            1,
            1.0,
            None,
            Some("D:"),
        )
        .unwrap();
    }
    let conn = db::connect(&p, 1024).unwrap();
    let row = db::get_file_by_path(&conn, "D:/x/a.txt").unwrap().unwrap();
    assert_eq!(row.project.as_deref(), Some("D:"));
}
