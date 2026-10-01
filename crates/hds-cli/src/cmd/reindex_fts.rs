//! `hds reindex-fts` — порт `hds/cli.py::cmd_reindex_fts`: перестроить `chunks_fts`
//! из `chunks.text` (лемматизация через воркер, `busy_timeout=600000`) и поставить
//! отметку `meta.fts_normalized='1'`.
//!
//! Watcher может параллельно писать в БД — каждое действие терпеливо ждёт write-lock
//! (`DELETE` — 3 попытки), текст `chunks` при этом не меняется, поэтому постраничный
//! обход стабилен.

use hds_core::config::project_root;
use hds_core::error::{CoreError, Result};
use hds_index::Lemmatizer;

use crate::support::{build_sidecar, now_epoch, open_conn};

/// Вставка батча лемматизированных текстов в `chunks_fts`; увеличивает счётчик `n`.
fn flush(
    conn: &rusqlite::Connection,
    lemmatizer: &dyn Lemmatizer,
    items: &[(i64, String)],
    n: &mut u64,
) -> Result<()> {
    let texts: Vec<String> = items.iter().map(|(_, t)| t.clone()).collect();
    let norm = lemmatizer.normalize_many(&texts)?;
    for ((cid, _), ft) in items.iter().zip(norm.iter()) {
        conn.execute(
            "INSERT INTO chunks_fts(rowid, text) VALUES(?1, ?2)",
            rusqlite::params![cid, ft],
        )?;
    }
    *n += items.len() as u64;
    Ok(())
}

/// Перестройка FTS. Возвращает число вставленных чанков (0 — таблица пуста).
pub fn reindex_fts(
    conn: &rusqlite::Connection,
    lemmatizer: &dyn Lemmatizer,
    progress_sec: u64,
) -> Result<u64> {
    // 10 минут: DELETE и батчи вставок терпеливо ждут write-lock (держит watcher)
    conn.execute_batch("PRAGMA busy_timeout=600000")?;
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    if total == 0 {
        println!("В индексе нет чанков — перестраивать нечего.");
        return Ok(0);
    }

    for attempt in 1..=3 {
        match conn.execute("DELETE FROM chunks_fts", []) {
            Ok(_) => break,
            Err(e) => {
                let es = e.to_string().to_lowercase();
                if attempt == 3 || !es.contains("locked") {
                    return Err(CoreError::Sqlite(e));
                }
                println!(
                    "[warn] база занята другим процессом (watcher пишет) — \
                     повтор через 30 с ({attempt}/3)"
                );
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        }
    }

    let rep = hds_index::ProgressReporter::new(progress_sec);
    rep.start();
    let t0 = now_epoch();
    let mut n: u64 = 0;
    let mut last_id: i64 = 0;
    loop {
        let batch: Vec<(i64, String)> = {
            let mut stmt =
                conn.prepare("SELECT id, text FROM chunks WHERE id > ?1 ORDER BY id LIMIT 500")?;
            let mut rows = stmt.query([last_id])?;
            let mut v = Vec::new();
            while let Some(row) = rows.next()? {
                v.push((row.get(0)?, row.get(1)?));
            }
            v
        };
        if batch.is_empty() {
            break;
        }
        last_id = batch.last().map(|(id, _)| *id).unwrap_or(last_id);
        flush(conn, lemmatizer, &batch, &mut n)?;
        rep.seen();
    }

    let elapsed = now_epoch() - t0;
    rep.finish(Some(&serde_json::json!({
        "chunks": n,
        "elapsed_sec": (elapsed * 10.0).round() / 10.0,
    })));
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES('fts_normalized','1')",
        [],
    )?;
    Ok(n)
}

/// `cmd_reindex_fts`: печатает заголовок (как Python) и запускает перестройку.
pub fn cmd_reindex_fts(progress_sec: u64) -> i32 {
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
    let total: i64 = match conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0)) {
        Ok(t) => t,
        Err(e) => {
            println!("База данных: {e}");
            return 1;
        }
    };
    if total == 0 {
        println!("В индексе нет чанков — перестраивать нечего.");
        return 0;
    }
    let root = project_root();
    let sidecar = match build_sidecar(&root) {
        Ok(s) => s,
        Err(e) => {
            println!("Воркер лемматизации: {}", e.message());
            return 1;
        }
    };
    let has_norm = sidecar.capabilities().has("normalize");
    println!(
        "Перестройка FTS: {total} чанков{}",
        if has_norm {
            " (лемматизация pymorphy3)"
        } else {
            " (pymorphy3 не установлен — БЕЗ морфологии)"
        }
    );
    let res = reindex_fts(&conn, &sidecar, progress_sec);
    sidecar.shutdown();
    match res {
        Ok(_) => 0,
        Err(e) => {
            println!("Ошибка: {}", e.message());
            1
        }
    }
}
