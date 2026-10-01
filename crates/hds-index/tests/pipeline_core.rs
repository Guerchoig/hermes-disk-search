//! Тесты конвейера `process_file`/`run_index` без сети и Python:
//! инкрементальность (`unchanged`/`moved`/`force`), изоляция ошибок, `prune`,
//! `skipped_*`. Эмбеддинги не вызываются (чанки пустые), поэтому фасад не нужен.
//!
//! Полный паритет с golden (sidecar + фасад `:8011`) — `tests/pipeline_parity.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_core::config::Config;
use hds_core::db;
use hds_core::error::{CoreError, Result};
use hds_index::chunker::Segment;
use hds_index::embed::Embedder;
use hds_index::pipeline;
use hds_index::sidecar::{Extractor, TokenLemmatizer};

/// Извлекатель-заглушка: фиксированные сегменты или ошибка.
struct FakeExtractor {
    segments: Vec<Segment>,
    kind: String,
    fail: Option<String>,
}

impl Extractor for FakeExtractor {
    fn extract(&self, _path: &Path) -> Result<(String, Vec<Segment>)> {
        if let Some(e) = &self.fail {
            return Err(CoreError::Other(e.clone()));
        }
        Ok((self.kind.clone(), self.segments.clone()))
    }
}

fn cfg_yaml(max_file_mb: u64) -> Config {
    serde_yaml::from_str(&format!(
        "index:\n  max_file_mb: {max_file_mb}\n  max_media_mb: 2500\n  \
         max_chunks: 3000\n  exclude_dirs: [\"node_modules\"]\n\
         chunk: {{size: 800, overlap: 120}}\n\
         embedding: {{base_url: \"http://127.0.0.1:1/v1\", model: \"m\", batch_size: 1}}\n"
    ))
    .unwrap()
}

fn workdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hds-pipe-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Embedder-заглушка: реальных вызовов нет, пока чанки пустые.
fn dummy_embedder() -> Embedder {
    Embedder::new("http://127.0.0.1:1/v1", "m", 1, Duration::from_millis(50))
}

fn open(dir: &Path) -> rusqlite::Connection {
    db::connect(&dir.join("index.db"), 1024).unwrap()
}

fn fake(kind: &str, fail: Option<&str>) -> FakeExtractor {
    FakeExtractor {
        segments: vec![],
        kind: kind.into(),
        fail: fail.map(|s| s.to_string()),
    }
}

#[test]
fn skipped_type_and_office_lock() {
    let dir = workdir("skip");
    let conn = open(&dir);
    let cfg = cfg_yaml(200);
    let ex = fake("text", None);

    // неизвестное расширение → skipped_type, kind нет
    let p = dir.join("a.bin");
    std::fs::write(&p, b"x").unwrap();
    match pipeline::extract_file(&conn, &cfg, &p, false, &ex).unwrap() {
        pipeline::ExtractOutcome::Early { status, kind } => {
            assert_eq!(status, "skipped_type");
            assert!(kind.is_none());
        }
        _ => panic!("ожидался ранний выход"),
    }

    // служебный ~$файл Office → skipped_type с видом
    let lock = dir.join("~$реестр.xlsx");
    std::fs::write(&lock, b"x").unwrap();
    match pipeline::extract_file(&conn, &cfg, &lock, false, &ex).unwrap() {
        pipeline::ExtractOutcome::Early { status, kind } => {
            assert_eq!(status, "skipped_type");
            assert_eq!(kind.as_deref(), Some("xlsx"));
        }
        _ => panic!("ожидался ранний выход"),
    }
}

#[test]
fn skipped_big_respects_limit() {
    let dir = workdir("big");
    let conn = open(&dir);
    let cfg = cfg_yaml(0); // любой непустой файл больше лимита
    let ex = fake("text", None);
    let p = dir.join("a.txt");
    std::fs::write(&p, b"hello").unwrap();
    match pipeline::extract_file(&conn, &cfg, &p, false, &ex).unwrap() {
        pipeline::ExtractOutcome::Early { status, kind } => {
            assert_eq!(status, "skipped_big");
            assert_eq!(kind.as_deref(), Some("text"));
        }
        _ => panic!("ожидался ранний выход"),
    }
}

#[test]
fn unchanged_and_force() {
    let dir = workdir("unchanged");
    let conn = open(&dir);
    let cfg = cfg_yaml(200);
    let ex = fake("text", None);
    let p = dir.join("a.txt");
    std::fs::write(&p, b"hello world").unwrap();
    let meta = std::fs::metadata(&p).unwrap();
    let size = meta.len() as i64;
    let mtime = pipeline::mtime_secs(&meta);
    db::upsert_file(&conn, &pipeline::path_str(&p), ".txt", "text", size, mtime, Some("h")).unwrap();
    let fid = db::get_file_by_path(&conn, &pipeline::path_str(&p)).unwrap().unwrap().id;
    db::finish_file(&conn, fid, "indexed", None, Some(1), 1.0).unwrap();

    match pipeline::extract_file(&conn, &cfg, &p, false, &ex).unwrap() {
        pipeline::ExtractOutcome::Early { status, .. } => assert_eq!(status, "unchanged"),
        _ => panic!("ожидался unchanged"),
    }
    match pipeline::extract_file(&conn, &cfg, &p, true, &ex).unwrap() {
        pipeline::ExtractOutcome::Ready { .. } => {}
        _ => panic!("force должен пропустить проверку unchanged"),
    }
}

#[test]
fn moved_renames_when_old_path_gone() {
    let dir = workdir("moved");
    let conn = open(&dir);
    let cfg = cfg_yaml(200);
    let ex = fake("text", None);
    let new_path = dir.join("new.txt");
    std::fs::write(&new_path, b"same content").unwrap();
    let size = std::fs::metadata(&new_path).unwrap().len();
    let chash = hds_index::content_hash(&new_path, size).unwrap();
    let old = dir.join("old.txt");
    db::upsert_file(
        &conn,
        &pipeline::path_str(&old),
        ".txt",
        "text",
        size as i64,
        1.0,
        Some(&chash),
    )
    .unwrap();
    let fid = db::get_file_by_path(&conn, &pipeline::path_str(&old)).unwrap().unwrap().id;
    db::finish_file(&conn, fid, "indexed", None, Some(3), 1.0).unwrap();

    match pipeline::extract_file(&conn, &cfg, &new_path, false, &ex).unwrap() {
        pipeline::ExtractOutcome::Early { status, .. } => assert_eq!(status, "moved"),
        _ => panic!("ожидался moved"),
    }
    assert!(db::get_file_by_path(&conn, &pipeline::path_str(&old)).unwrap().is_none());
    assert!(db::get_file_by_path(&conn, &pipeline::path_str(&new_path)).unwrap().is_some());
}

#[test]
fn process_file_error_is_isolated_and_saved() {
    let dir = workdir("err");
    let conn = open(&dir);
    let cfg = cfg_yaml(200);
    let ex = fake("docx", Some("BadZipFile: file is not a zip file"));
    let (emb, lem) = (dummy_embedder(), TokenLemmatizer);
    let p = dir.join("bad.docx");
    std::fs::write(&p, b"not a zip").unwrap();

    let (status, kind) =
        pipeline::process_file(&conn, &cfg, &emb, &p, false, &ex, &lem, None).unwrap();
    assert!(status.starts_with("error: BadZipFile"), "status={status}");
    assert_eq!(kind.as_deref(), Some("docx"));
    let row = db::get_file_by_path(&conn, &pipeline::path_str(&p)).unwrap().unwrap();
    assert_eq!(row.status.as_deref(), Some("error"));
    assert!(row.error.unwrap().contains("BadZipFile"));
}

#[test]
fn process_file_empty_chunks_indexed() {
    let dir = workdir("empty");
    let conn = open(&dir);
    let cfg = cfg_yaml(200);
    let ex = fake("text", None);
    let (emb, lem) = (dummy_embedder(), TokenLemmatizer);
    let p = dir.join("empty.txt");
    std::fs::write(&p, b"").unwrap();
    let (status, _kind) =
        pipeline::process_file(&conn, &cfg, &emb, &p, false, &ex, &lem, None).unwrap();
    assert_eq!(status, "indexed(0 чанков)");
    let row = db::get_file_by_path(&conn, &pipeline::path_str(&p)).unwrap().unwrap();
    assert!(row.is_indexed());
    assert_eq!(row.chunk_count, Some(0));
    assert!(row.indexed_at.is_some());
}

#[test]
fn prune_blocks_over_20_percent_without_confirm() {
    let dir = workdir("prune");
    let conn = open(&dir);
    let cfg = cfg_yaml(200);
    for i in 0..3 {
        db::upsert_file(
            &conn,
            &format!("D:/gone/missing{i}.txt"),
            ".txt",
            "text",
            1,
            1.0,
            None,
        )
        .unwrap();
    }
    let live = dir.join("live.txt");
    std::fs::write(&live, b"x").unwrap();
    db::upsert_file(&conn, &pipeline::path_str(&live), ".txt", "text", 1, 1.0, None).unwrap();

    let removed = pipeline::prune_deleted(&conn, &cfg, false).unwrap();
    assert_eq!(removed, 0, "без --confirm-delete prune заблокирован");
    assert_eq!(db::all_files(&conn).unwrap().len(), 4);

    let removed = pipeline::prune_deleted(&conn, &cfg, true).unwrap();
    assert_eq!(removed, 3);
    assert_eq!(db::all_files(&conn).unwrap().len(), 1);
}