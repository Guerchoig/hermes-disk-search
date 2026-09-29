//! Спайк 1 (W0 §4 п.3): чтение реальной index.db из Rust — FTS5 + KNN + sqlite-vec.
//!
//! Зафиксирован как интеграционный тест под #[ignore] (требует боевую БД).
//! Запуск:  cargo test --test spike1_db -- --ignored --nocapture
//! БД: env HDS_DB, по умолчанию D:\hermes-disk-search-db\index.db (read-only).
//!
//! Ловушка (зафиксирована в Приложении А, проба № 2): sqlite-vec регистрируется
//! ТОЛЬКО через sqlite3_auto_extension + transmute. Прямой вызов
//! sqlite_vec::sqlite3_vec_init() модуль vec0 не регистрирует
//! («no such module: vec0»).

use rusqlite::{Connection, OpenFlags};

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_DB: &str = r"D:\hermes-disk-search-db\index.db";

    fn open_db() -> Connection {
        let db = std::env::var("HDS_DB").unwrap_or_else(|_| DEFAULT_DB.to_string());
        println!("db: {}", db);
        Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("не удалось открыть index.db read-only")
    }

    #[test]
    #[ignore = "требует боевую index.db (см. HDS_DB)"]
    fn fts_and_knn_on_real_db() {
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
        let conn = open_db();

        let vec_version: String = conn
            .query_row("SELECT vec_version()", [], |r| r.get(0))
            .unwrap();
        println!("vec_version={}", vec_version);
        assert!(vec_version.contains("0.1.9"), "версия vec0 должна быть 0.1.9");

        let files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0)).unwrap();
        let chunks: i64 =
            conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)).unwrap();
        let vec_dim: i64 = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key='vec_dim'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let images_vec: i64 = conn
            .query_row("SELECT COUNT(*) FROM images_vec", [], |r| r.get(0))
            .unwrap_or(0);
        println!(
            "db files={} chunks={} vec_dim={} images_vec={}",
            files, chunks, vec_dim, images_vec
        );
        assert!(files > 0 && chunks > 0, "БД должна быть непустой");
        assert_eq!(vec_dim, 1024, "meta.vec_dim должен быть 1024 (bge-m3)");

        // FTS5: реальный запрос по лемме
        let fts: Vec<i64> = conn
            .prepare(
                "SELECT cf.rowid FROM chunks_fts cf \
                 WHERE chunks_fts MATCH ? ORDER BY bm25(chunks_fts) LIMIT 5",
            )
            .unwrap()
            .query_map(["\"документооборот\""], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        println!("fts_match={}", fts.len());
        assert!(!fts.is_empty(), "FTS5-запрос обязан находить чанки");

        // KNN по chunks_vec с JOIN (нулевой вектор — проверяется механика vec0+JOIN)
        let zero = vec![0f32; vec_dim as usize];
        let bytes: Vec<u8> = zero.iter().flat_map(|v| v.to_le_bytes()).collect();
        let knn: Vec<(i64, f32)> = conn
            .prepare(
                "SELECT v.rowid, v.distance FROM chunks_vec v \
                 JOIN chunks c ON c.id = v.rowid \
                 JOIN files f ON f.id = c.file_id \
                 WHERE v.embedding MATCH ?1 AND v.k = ?2 \
                 ORDER BY v.distance LIMIT 3",
            )
            .unwrap()
            .query_map(rusqlite::params![bytes, 3], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        println!("knn_rows={}", knn.len());
        assert_eq!(knn.len(), 3, "KNN с JOIN обязан вернуть 3 строки");

        println!("FTS_AND_KNN_ON_REAL_DB_OK");
    }
}