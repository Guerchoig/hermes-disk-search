//! `hds forget <path>` — порт `hds/cli.py::cmd_forget` (`db.remove_path`).

use std::path::PathBuf;

use hds_core::error::Result;

use crate::support::open_conn;

/// Абсолютный путь как в Python `os.path.abspath`.
pub fn abs_path(path: &str) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// Порт `db.remove_path`: удалить запись файла и её данные (чанки/FTS/vec/CLIP).
/// Возвращает `true`, если запись была и удалена.
pub fn forget(conn: &rusqlite::Connection, path: &str) -> Result<bool> {
    hds_core::db::remove_path(conn, path)
}

/// `cmd_forget`: печатает результат, всегда 0 (как Python).
pub fn cmd_forget(path: &str) -> i32 {
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let conn = match open_conn(&cfg) {
        Ok(c) => c,
        Err(e) => {
            println!("База данных недоступна: {}", e.message());
            return 1;
        }
    };
    let abs = abs_path(path);
    match forget(&conn, &abs.to_string_lossy()) {
        Ok(true) => println!("Удалено из индекса"),
        Ok(false) => println!("Файл в индексе не найден"),
        Err(e) => {
            println!("Ошибка: {}", e.message());
            return 1;
        }
    }
    0
}
