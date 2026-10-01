//! Приёмка B4 №2: инкрементальный прогон на **копии** боевой БД — контрольная
//! выборка чанков до/после совпадает, БД остаётся читаемой Python-версией.
//!
//! Требует боевую `index.db` (env `HDS_DB`, по умолчанию `D:\hermes-disk-search-db\index.db`).
//! Копирование тяжёлое — тест под `#[ignore]`.
//!
//! Запуск: `cargo test -p hds-index --test pipeline_incremental -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_core::config::{Config, project_root};
use hds_core::db;
use hds_core::error::{CoreError, Result};
use hds_index::chunker::Segment;
use hds_index::embed::Embedder;
use hds_index::pipeline;
use hds_index::sidecar::{Extractor, TokenLemmatizer};

/// Извлекатель-сторож: при `unchanged` не вызывается; если вызван — тест провален.
struct PanicExtractor;

impl Extractor for PanicExtractor {
    fn extract(&self, path: &Path) -> Result<(String, Vec<Segment>)> {
        Err(CoreError::Other(format!(
            "extractor не должен вызываться для unchanged: {}",
            path.display()
        )))
    }
}

fn dummy_embedder() -> Embedder {
    Embedder::new("http://127.0.0.1:1/v1", "m", 1, Duration::from_millis(50))
}

fn cfg_empty() -> Config {
    serde_yaml::from_str("index: {max_chunks: 3000}\nchunk: {size: 800, overlap: 120}\n").unwrap()
}

#[test]
#[ignore = "требует боевую index.db (HDS_DB) + копирование БД"]
fn incremental_on_copy_of_prod_db() {
    let src = std::env::var("HDS_DB")
        .unwrap_or_else(|_| r"D:\hermes-disk-search-db\index.db".to_string());
    let src = PathBuf::from(src);
    if !src.exists() {
        println!("пропуск: нет {}", src.display());
        return;
    }
    let dir = std::env::temp_dir().join(format!("hds-inc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("index.db");
    std::fs::copy(&src, &copy).expect("копия боевой БД");
    println!("копия: {}", copy.display());

    let conn = db::connect(&copy, 1024).unwrap();
    let dim: i64 = conn
        .query_row("SELECT CAST(value AS INTEGER) FROM meta WHERE key='vec_dim'", [], |r| {
            r.get(0)
        })
        .unwrap_or(1024);
    let _ = dim;

    // контрольная выборка: проиндексированные файлы, которые есть на диске
    let mut rows: Vec<(i64, String)> = Vec::new();
    {
        let mut st = conn
            .prepare(
                "SELECT id, path FROM files WHERE status='indexed' AND chunk_count>0 LIMIT 200",
            )
            .unwrap();
        let it = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .unwrap();
        for r in it.flatten() {
            if Path::new(&r.1).exists() {
                rows.push(r);
                if rows.len() >= 30 {
                    break;
                }
            }
        }
    }
    if rows.is_empty() {
        println!("пропуск: в БД нет доступных проиндексированных файлов");
        return;
    }

    // снимок чанков «до»
    let before: Vec<Vec<(i64, Option<i64>, Option<f64>, String)>> =
        rows.iter().map(|(fid, _)| snapshot(&conn, *fid)).collect();

    // инкрементальный прогон (force=false) — все обязаны быть unchanged
    let (ex, lem, emb) = (PanicExtractor, TokenLemmatizer, dummy_embedder());
    let cfg = cfg_empty();
    let mut unchanged = 0usize;
    for (_, path) in &rows {
        let res = pipeline::reindex_path(&conn, &cfg, &emb, &ex, &lem, Path::new(path), false)
            .expect("reindex_path");
        for (status, _) in res {
            assert_eq!(status, "unchanged", "файл {path}: ожидался unchanged, получено {status}");
            unchanged += 1;
        }
    }
    assert_eq!(unchanged, rows.len());

    // снимок «после» — совпадает
    for ((fid, path), snap) in rows.iter().zip(before.iter()) {
        assert_eq!(snapshot(&conn, *fid), *snap, "чанки изменились: {path}");
    }
    println!("инкремент: {} файлов unchanged, чанки совпали", rows.len());

    // БД остаётся читаемой Python-версией
    let py = project_root().join(".venv").join("Scripts").join("python.exe");
    if py.exists() {
        let script = format!(
            "import sys; sys.path.insert(0, r'{root}')\n\
             from hds import db as d\n\
             c = d.connect(r'{db}', 1024)\n\
             print('PY_OK', c.execute('SELECT COUNT(*) FROM chunks').fetchone()[0], \
             c.execute('SELECT vec_version()').fetchone()[0])\n",
            root = project_root().display(),
            db = copy.display()
        );
        let out = std::process::Command::new(&py)
            .args(["-c", &script])
            .output()
            .expect("python");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("PY_OK"), "python не прочитал БД: {stdout}");
        println!("{}", stdout.trim());
    } else {
        println!("пропуск Python-проверки: нет .venv");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

fn snapshot(conn: &rusqlite::Connection, fid: i64) -> Vec<(i64, Option<i64>, Option<f64>, String)> {
    let mut st = conn
        .prepare("SELECT ord, page, t_start, text FROM chunks WHERE file_id=?1 ORDER BY ord")
        .unwrap();
    let it = st
        .query_map([fid], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<f64>>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap();
    it.filter_map(|r| r.ok()).collect()
}