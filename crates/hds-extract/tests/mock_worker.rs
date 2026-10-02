//! Тест клиента на mock-воркере (stdlib-only) — изолирует транспорт/жизненный
//! цикл от реального Python-окружения (hds/pymupdf).

use std::path::PathBuf;
use std::time::Duration;

use hds_extract::worker::{discover_python, Worker, WorkerConfig};

fn mock_cfg() -> Option<WorkerConfig> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let py = discover_python(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .parent()?,
    )?;
    let mut cfg = WorkerConfig::new(&py, &root);
    cfg.script = root.join("mock_worker.py");
    cfg.request_timeout = Duration::from_secs(20);
    Some(cfg)
}

#[test]
fn mock_worker_handshake_and_calls() {
    let cfg = match mock_cfg() {
        Some(c) => c,
        None => {
            println!("пропуск: нет интерпретатора Python");
            return;
        }
    };
    let mut w = Worker::spawn(cfg).expect("mock-воркер должен запуститься");
    assert_eq!(w.capabilities().protocol, 1);
    assert!(w.capabilities().has("normalize"));

    let lemmas = w.normalize(&["тест".to_string()]).unwrap();
    assert_eq!(lemmas, vec!["M:тест"]);

    let r = w.extract(std::path::Path::new("x.txt")).unwrap();
    assert_eq!(r.kind, "text");
    assert_eq!(r.segments.len(), 1);

    // неизвестный метод → структурированная ошибка
    let err = w.clip_image(&[PathBuf::from("a.jpg")]).unwrap_err();
    assert!(err.message().contains("-32601"), "{}", err.message());

    w.shutdown();
    assert!(!w.is_alive(), "mock-воркер должен завершиться");
}
