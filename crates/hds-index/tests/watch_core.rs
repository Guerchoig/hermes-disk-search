//! Тесты наблюдателя (`hds/watcher.py`): разбор `ReadDirectoryChangesW`,
//! `watch.lock`, `wait_stable`, обработка событий (без реальных событий ОС).

use std::path::{Path, PathBuf};

use hds_core::config::Config;
use hds_core::db;
use hds_core::error::{CoreError, Result};
use hds_index::chunker::Segment;
use hds_index::embed::Embedder;
use hds_index::sidecar::{Extractor, TokenLemmatizer};
use hds_index::watch::{self, WatchEvent, WatchLock, WatchState};

struct FakeExtractor;
impl Extractor for FakeExtractor {
    fn extract(&self, _p: &Path) -> Result<(String, Vec<Segment>)> {
        Ok(("text".into(), vec![]))
    }
}

struct FailExtractor;
impl Extractor for FailExtractor {
    fn extract(&self, p: &Path) -> Result<(String, Vec<Segment>)> {
        Err(CoreError::Other(format!("сбой: {}", p.display())))
    }
}

fn cfg_yaml() -> Config {
    serde_yaml::from_str(
        "index:\n  max_file_mb: 200\n  max_media_mb: 2500\n  max_chunks: 3000\n  \
         exclude_dirs: [\"$RECYCLE.BIN\", \".Trash\"]\n\
         chunk: {size: 800, overlap: 120}\n\
         embedding: {base_url: \"http://127.0.0.1:1/v1\", model: \"m\", batch_size: 1}\n\
         watch: {debounce_seconds: 2, max_stable_wait: 10}\n",
    )
    .unwrap()
}

fn workdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hds-watch-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn dummy_embedder() -> Embedder {
    Embedder::new("http://127.0.0.1:1/v1", "m", 1, std::time::Duration::from_millis(50))
}

/// Запись `FILE_NOTIFY_INFORMATION` (next, action, len, UTF-16 имя).
fn notify(next: u32, action: u32, name: &str) -> Vec<u8> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut b = Vec::new();
    b.extend_from_slice(&next.to_le_bytes());
    b.extend_from_slice(&action.to_le_bytes());
    b.extend_from_slice(&((units.len() * 2) as u32).to_le_bytes());
    for u in units {
        b.extend_from_slice(&u.to_le_bytes());
    }
    b
}

#[test]
fn parse_notifications_maps_actions() {
    let root = Path::new("D:/root");
    let mut pending = None;

    // ADDED + MODIFIED → Modified (первая запись ссылается на вторую через `next`)
    let mut buf = notify(notify(0, 3, "b.txt").len() as u32, 1, "a.txt");
    buf.extend_from_slice(&notify(0, 3, "b.txt"));
    let ev = watch::parse_notifications(&buf, root, &mut pending);
    assert_eq!(
        ev,
        vec![
            WatchEvent::Modified(root.join("a.txt")),
            WatchEvent::Modified(root.join("b.txt")),
        ]
    );

    // REMOVED → Deleted
    let ev = watch::parse_notifications(&notify(0, 2, "c.txt"), root, &mut pending);
    assert_eq!(ev, vec![WatchEvent::Deleted(root.join("c.txt"))]);

    // RENAMED_OLD + RENAMED_NEW → Moved (двумя записями, next != 0)
    let mut buf = notify(notify(0, 5, "new.txt").len() as u32, 4, "old.txt");
    buf.extend_from_slice(&notify(0, 5, "new.txt"));
    let ev = watch::parse_notifications(&buf, root, &mut pending);
    assert_eq!(
        ev,
        vec![WatchEvent::Moved(root.join("old.txt"), root.join("new.txt"))]
    );
}

#[test]
fn lock_is_atomic_and_releases() {
    let dir = workdir("lock");
    let l1 = WatchLock::acquire(&dir, 3).unwrap().expect("первый захват");
    assert!(l1.path().exists());
    assert!(WatchLock::acquire(&dir, 3).unwrap().is_none());
    l1.release();
    assert!(!l1.path().exists());
    assert!(WatchLock::acquire(&dir, 3).unwrap().is_some());
}

#[test]
fn stale_lock_is_reclaimed() {
    let dir = workdir("stale");
    let lock = dir.join("watch.lock");
    std::fs::write(&lock, "0").unwrap();
    assert!(watch::lock_is_stale(&lock));
    let got = WatchLock::acquire(&dir, 3).unwrap();
    assert!(got.is_some(), "устаревший lock должен быть перехвачен");
    assert_eq!(
        std::fs::read_to_string(&lock).unwrap().trim(),
        std::process::id().to_string()
    );
}

#[test]
fn wait_stable_and_missing() {
    let dir = workdir("stable");
    let f = dir.join("a.txt");
    std::fs::write(&f, b"data").unwrap();
    assert!(watch::wait_stable(&f, 2, 10), "размер не меняется — стабилен");
    assert!(!watch::wait_stable(&dir.join("nope.txt"), 2, 10), "нет файла → false");
}

#[test]
fn handle_event_modified_indexes_and_skips() {
    let dir = workdir("modified");
    let conn = db::connect(&dir.join("index.db"), 1024).unwrap();
    let cfg = cfg_yaml();
    let (ex, lem, emb) = (FakeExtractor, TokenLemmatizer, dummy_embedder());
    let mut st = WatchState::default();

    let f = dir.join("a.txt");
    std::fs::write(&f, b"hello").unwrap();
    let status = watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Modified(f.clone()), &mut st,
    )
    .unwrap();
    assert_eq!(status.as_deref(), Some("indexed(0 чанков)"));
    assert!(db::get_file_by_path(&conn, &hds_index::pipeline::path_str(&f)).unwrap().is_some());

    // .tmp → пропуск
    let t = dir.join("x.tmp");
    std::fs::write(&t, b"x").unwrap();
    let status = watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Modified(t.clone()), &mut st,
    )
    .unwrap();
    assert!(status.is_none());

    // корзина (exclude_dirs) → пропуск
    let rec = dir.join("$RECYCLE.BIN");
    std::fs::create_dir_all(&rec).unwrap();
    let rf = rec.join("gone.txt");
    std::fs::write(&rf, b"x").unwrap();
    let status = watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Modified(rf), &mut st,
    )
    .unwrap();
    assert!(status.is_none());
}

#[test]
fn handle_event_deleted_and_moved() {
    let dir = workdir("delmov");
    let conn = db::connect(&dir.join("index.db"), 1024).unwrap();
    let cfg = cfg_yaml();
    let (ex, lem, emb) = (FakeExtractor, TokenLemmatizer, dummy_embedder());
    let mut st = WatchState::default();

    let src = dir.join("src.txt");
    std::fs::write(&src, b"data").unwrap();
    watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Modified(src.clone()), &mut st,
    )
    .unwrap();

    let dst = dir.join("dst.txt");
    std::fs::rename(&src, &dst).unwrap();
    let status = watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Moved(src.clone(), dst.clone()), &mut st,
    )
    .unwrap();
    assert_eq!(status.as_deref(), Some("moved"));
    assert!(db::get_file_by_path(&conn, &hds_index::pipeline::path_str(&src)).unwrap().is_none());
    assert!(db::get_file_by_path(&conn, &hds_index::pipeline::path_str(&dst)).unwrap().is_some());

    std::fs::remove_file(&dst).unwrap();
    let status = watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Deleted(dst.clone()), &mut st,
    )
    .unwrap();
    assert_eq!(status.as_deref(), Some("removed_from_index"));
    assert!(db::get_file_by_path(&conn, &hds_index::pipeline::path_str(&dst)).unwrap().is_none());

    // перемещение в корзину → удаление из индекса
    let s2 = dir.join("s2.txt");
    std::fs::write(&s2, b"x").unwrap();
    watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Modified(s2.clone()), &mut st,
    )
    .unwrap();
    let rec = dir.join("$RECYCLE.BIN").join("s2.txt");
    std::fs::create_dir_all(dir.join("$RECYCLE.BIN")).unwrap();
    std::fs::rename(&s2, &rec).unwrap();
    let status = watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Moved(s2.clone(), rec), &mut st,
    )
    .unwrap();
    assert_eq!(status.as_deref(), Some("moved"));
    assert!(db::get_file_by_path(&conn, &hds_index::pipeline::path_str(&s2)).unwrap().is_none());
}

#[test]
fn handle_event_error_does_not_panic() {
    let dir = workdir("err");
    let conn = db::connect(&dir.join("index.db"), 1024).unwrap();
    let cfg = cfg_yaml();
    let (ex, lem, emb) = (FailExtractor, TokenLemmatizer, dummy_embedder());
    let mut st = WatchState::default();
    let f = dir.join("bad.txt");
    std::fs::write(&f, b"x").unwrap();
    let status = watch::handle_event(
        &conn, &cfg, &emb, &ex, &lem, &WatchEvent::Modified(f), &mut st,
    )
    .unwrap();
    assert!(status.unwrap().starts_with("error: сбой"));
    assert_eq!(st.errors, 1);
}