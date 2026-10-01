//! Live-тест наблюдателя B5: реальные события ОС (`ReadDirectoryChangesW`)
//! → `handle_event` → БД. Шесть сценариев: create, modify, rename, delete,
//! mass-write, «пропуск корзины».
//!
//! Только Windows (на прочих платформах backend — опрос, отдельный тест не нужен).

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hds_core::config::Config;
use hds_core::db;
use hds_core::error::Result;
use hds_index::chunker::Segment;
use hds_index::embed::Embedder;
use hds_index::sidecar::{Extractor, TokenLemmatizer};
use hds_index::watch::{self, WatchEvent, WatchState};

struct FakeExtractor;
impl Extractor for FakeExtractor {
    fn extract(&self, _p: &Path) -> Result<(String, Vec<Segment>)> {
        Ok(("text".into(), vec![]))
    }
}

fn cfg_yaml() -> Config {
    serde_yaml::from_str(
        "index:\n  max_file_mb: 200\n  max_media_mb: 2500\n  max_chunks: 3000\n  \
         exclude_dirs: [\"$RECYCLE.BIN\"]\n\
         chunk: {size: 800, overlap: 120}\n\
         embedding: {base_url: \"http://127.0.0.1:1/v1\", model: \"m\", batch_size: 1}\n\
         watch: {debounce_seconds: 2, max_stable_wait: 10}\n",
    )
    .unwrap()
}

fn workdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hds-wlive-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn dummy_embedder() -> Embedder {
    Embedder::new("http://127.0.0.1:1/v1", "m", 1, Duration::from_millis(50))
}

/// Ждёт событие, удовлетворяющее предикату (секунды).
fn wait_event<F: Fn(&WatchEvent) -> bool>(rx: &Receiver<WatchEvent>, pred: F, secs: u64) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(e) => {
                if pred(&e) {
                    return true;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(_) => return false,
        }
    }
    false
}

/// Гоняет события через `handle_event`, пока `until` не станет истиной.
fn pump<F: Fn() -> bool>(
    rx: &Receiver<WatchEvent>,
    conn: &rusqlite::Connection,
    cfg: &Config,
    emb: &Embedder,
    st: &mut WatchState,
    until: F,
    secs: u64,
) {
    let (ex, lem) = (FakeExtractor, TokenLemmatizer);
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if until() {
            return;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ev) => {
                let _ = watch::handle_event(conn, cfg, emb, &ex, &lem, &ev, st);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
    }
}

fn row(conn: &rusqlite::Connection, path: &Path) -> bool {
    db::get_file_by_path(conn, &hds_index::pipeline::path_str(path))
        .unwrap()
        .is_some()
}

#[test]
fn windows_watcher_six_scenarios() {
    let dir = workdir("six");
    let conn = db::connect(&dir.join("index.db"), 1024).unwrap();
    let cfg = cfg_yaml();
    let emb = dummy_embedder();
    let mut st = WatchState::default();

    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    watch::spawn_root_watcher(&dir, tx, Arc::clone(&stop));
    std::thread::sleep(Duration::from_millis(700)); // дать потоку открыть хэндл

    // 1. create → индексируется
    let a = dir.join("a.txt");
    std::fs::write(&a, b"hello").unwrap();
    assert!(
        wait_event(&rx, |e| matches!(e, WatchEvent::Modified(p) if p.ends_with("a.txt")), 15),
        "событие создания a.txt не пришло"
    );
    // дожидаемся индексации (wait_stable ~2 с) — докидываем события
    pump(&rx, &conn, &cfg, &emb, &mut st, || row(&conn, &a), 30);
    assert!(row(&conn, &a), "a.txt не проиндексирован");

    // 2. modify → переиндексация (размер меняется)
    std::fs::write(&a, b"hello world bigger").unwrap();
    pump(&rx, &conn, &cfg, &emb, &mut st, || false, 5);
    let meta = db::get_file_by_path(&conn, &hds_index::pipeline::path_str(&a))
        .unwrap()
        .unwrap();
    assert_eq!(meta.size, Some(18), "размер после modify не обновился");

    // 3. rename a.txt → b.txt
    let b = dir.join("b.txt");
    std::fs::rename(&a, &b).unwrap();
    pump(&rx, &conn, &cfg, &emb, &mut st, || row(&conn, &b), 20);
    assert!(row(&conn, &b) && !row(&conn, &a), "rename не отразился в индексе");

    // 4. delete b.txt
    std::fs::remove_file(&b).unwrap();
    pump(&rx, &conn, &cfg, &emb, &mut st, || !row(&conn, &b), 20);
    assert!(!row(&conn, &b), "delete не убрал запись из индекса");

    // 5. mass-write: 10 файлов
    for i in 0..10 {
        std::fs::write(dir.join(format!("m{i}.txt")), b"x").unwrap();
    }
    pump(
        &rx,
        &conn,
        &cfg,
        &emb,
        &mut st,
        || {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM files WHERE path LIKE '%m%.txt'",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            n >= 10
        },
        60,
    );
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM files WHERE path LIKE '%m%.txt'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 10, "mass-write: проиндексировано {n} из 10");

    // 6. корзина → пропуск
    let rec = dir.join("$RECYCLE.BIN");
    std::fs::create_dir_all(&rec).unwrap();
    let rf = rec.join("gone.txt");
    std::fs::write(&rf, b"x").unwrap();
    std::thread::sleep(Duration::from_secs(2));
    assert!(!row(&conn, &rf), "файл из корзины не должен индексироваться");

    stop.store(true, Ordering::Relaxed);
}