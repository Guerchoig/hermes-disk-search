//! Тесты `db-move`: перенос БД, сохранение комментариев `config.yaml`, отказы.

mod common;

use std::time::Duration;

use common::TempDir;
use hds_cli::cmd::db_move::{move_db, MoveDbArgs};
use hds_core::db;

/// Подготовить БД с одной записью и одним чанком; вернуть путь.
fn make_db(path: &std::path::Path) {
    let conn = db::connect(path, 1024).expect("connect");
    conn.execute(
        "INSERT INTO files(path, status) VALUES(?1, 'indexed')",
        ["a.txt"],
    )
    .unwrap();
    conn.execute("INSERT INTO chunks(file_id, ord, text) VALUES(1, 0, 'x')", [])
        .unwrap();
    drop(conn);
}

fn args(td: &TempDir, cfg_path: std::path::PathBuf, old: std::path::PathBuf, new: std::path::PathBuf, force: bool) -> MoveDbArgs {
    MoveDbArgs {
        cfg_path,
        old_db: old,
        new_db: new,
        force,
        project_root: td.path.clone(),
        restart_watcher: false,
        stop_timeout: Duration::from_millis(10),
    }
}

#[test]
fn move_copies_db_and_preserves_config_comments() {
    let td = TempDir::new("dbmove");
    let old = td.join("index.db");
    make_db(&old);

    let cfg_path = td.write_config("# комментарий заказчика\nindex:\n  roots:\n    - \"D:\\\\\"\ndb_path: 'старый'\n");
    let new = td.path.join("sub").join("new.db");

    let res = move_db(&args(&td, cfg_path.clone(), old.clone(), new.clone(), false));
    assert!(res.ok, "перенос должен пройти: {}", res.msg);
    assert!(!res.watch_was_running);
    assert!(new.is_file(), "новая БД должна существовать");

    // счётчики совпадают
    let c = rusqlite::Connection::open(&new).unwrap();
    let files: i64 = c
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    let chunks: i64 = c
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!((files, chunks), (1, 1));

    // config.yaml: комментарий сохранён, db_path обновлён
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(text.contains("# комментарий заказчика"), "комментарии должны остаться");
    assert!(
        text.contains(&format!("db_path: '{}'", new.to_string_lossy())),
        "db_path должен указывать на новую БД:\n{text}"
    );
    assert!(!text.contains("db_path: 'старый'"));

    // старая БД переименована в .moved-<stamp>
    let moved = std::fs::read_dir(&td.path)
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".moved-"));
    assert!(moved, "должна остаться копия .moved-<stamp>");
    assert!(!old.exists(), "исходный файл переименован");
}

#[test]
fn move_rejects_same_path() {
    let td = TempDir::new("dbmove-same");
    let old = td.join("index.db");
    make_db(&old);
    let cfg_path = td.write_config(&format!("db_path: '{}'\n", old.to_string_lossy()));
    let res = move_db(&args(&td, cfg_path, old.clone(), old.clone(), false));
    assert!(!res.ok);
    assert!(res.msg.contains("совпадает"), "msg: {}", res.msg);
}

#[test]
fn move_rejects_existing_target_without_force() {
    let td = TempDir::new("dbmove-exists");
    let old = td.join("index.db");
    make_db(&old);
    let new = td.join("new.db");
    std::fs::write(&new, "занято".as_bytes()).unwrap();
    let cfg_path = td.write_config(&format!("db_path: '{}'\n", old.to_string_lossy()));
    let res = move_db(&args(&td, cfg_path, old.clone(), new, false));
    assert!(!res.ok);
    assert!(res.msg.contains("уже существует"), "msg: {}", res.msg);
}

#[test]
fn move_missing_source_reports_error() {
    let td = TempDir::new("dbmove-missing");
    let old = td.join("nope.db");
    let new = td.join("new.db");
    let cfg_path = td.write_config("db_path: 'x'\n");
    let res = move_db(&args(&td, cfg_path, old, new, false));
    assert!(!res.ok);
    assert!(res.msg.contains("не найдена"), "msg: {}", res.msg);
}
