//! Live-тест клиента воркера: `hello`/`extract`/`normalize`/ошибка/перезапуск/
//! `shutdown` (реальный процесс `sidecar/hds_extract/worker.py`).
//!
//! Пропускается, если интерпретатор воркера не найден (грабля §9.7.7: тесты с
//! внешними зависимостями не должны ломать `cargo test`).

use std::path::PathBuf;
use std::time::Duration;

use hds_extract::worker::{discover_python, Worker, WorkerConfig};

fn repo_root() -> PathBuf {
    // без `..`: родитель `crates/hds-extract` → родитель `crates` → корень репозитория
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

fn spawn() -> Option<Worker> {
    let root = repo_root();
    let py = match discover_python(&root) {
        Some(p) => p,
        None => {
            println!("пропуск: нет интерпретатора воркера (sidecar/python или .venv)");
            return None;
        }
    };
    let mut cfg = WorkerConfig::new(&py, &root);
    cfg.request_timeout = Duration::from_secs(120);
    cfg.stderr_log = Some(std::env::temp_dir().join("hds-extract-worker.err.log"));
    match Worker::spawn(cfg) {
        Ok(w) => Some(w),
        Err(e) => {
            let log = std::fs::read_to_string(std::env::temp_dir().join("hds-extract-worker.err.log"))
                .unwrap_or_default();
            println!("пропуск: воркер не запустился: {}\nstderr: {}", e.message(), log);
            None
        }
    }
}

#[test]
fn worker_hello_extract_normalize_shutdown() {
    let mut w = match spawn() {
        Some(w) => w,
        None => return,
    };
    assert_eq!(w.capabilities().protocol, 1);
    assert!(w.capabilities().has("text"), "text должен быть в возможностях");
    assert!(w.capabilities().has("normalize"), "normalize должен быть (pymorphy3)");
    assert!(w.pid() > 0);

    // extract: md-фикстура → kind text, есть сегменты
    let root = repo_root();
    let fixture = root
        .join("tools")
        .join("parity")
        .join("fixtures")
        .join("инструкция_документооборот.md");
    if fixture.exists() {
        let r = w.extract(&fixture).unwrap();
        assert_eq!(r.kind, "text");
        assert!(!r.segments.is_empty(), "сегменты md не должны быть пусты");
        assert!(r.elapsed_ms >= 0.0);
    }

    // normalize: лемматизация (pymorphy3)
    let lemmas = w.normalize(&["Настройки скриптов".to_string()]).unwrap();
    assert_eq!(lemmas.len(), 1);
    assert!(lemmas[0].contains("настройк"), "лемма: {:?}", lemmas);

    // ошибка извлекателя: нет файла → структурированная ошибка (без паники)
    let missing = root
        .join("tools")
        .join("parity")
        .join("fixtures")
        .join("nope-xyz.pdf");
    let err = w.extract(&missing).unwrap_err();
    assert!(!err.message().is_empty());

    // shutdown → процесс завершился (EOF по stdin, §5.1)
    w.shutdown();
    assert!(!w.is_alive(), "воркер должен завершиться по shutdown/EOF");
}

#[test]
fn worker_restart_after_max_requests() {
    let root = repo_root();
    let py = match discover_python(&root) {
        Some(p) => p,
        None => {
            println!("пропуск: нет интерпретатора воркера");
            return;
        }
    };
    let mut cfg = WorkerConfig::new(&py, &root);
    cfg.max_requests = 1; // hello уже потратил 1 запрос → следующий вызов перезапустит
    let mut w = match Worker::spawn(cfg) {
        Ok(w) => w,
        Err(e) => {
            println!("пропуск: {}", e.message());
            return;
        }
    };
    let pid1 = w.pid();
    let _ = w.normalize(&["тест".to_string()]).unwrap();
    let pid2 = w.pid();
    assert_ne!(pid1, pid2, "после max_requests воркер должен перезапуститься");
    w.shutdown();
}