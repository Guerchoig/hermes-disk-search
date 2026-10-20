//! Тест авто-перезапуска: упавший воркер поднимается заново и запрос повторяется.
//!
//! Дефект релиза 0.20.5: Python-воркер падал на первом `extract` (нечитаемый
//! `config.yaml` в server-раскладке), `stdin` клиента оставался мёртвым, и **каждый**
//! следующий файл получал «воркер: запись: Идёт закрытие канала» — один прогон
//! накопил 43 537 записей `error`. Тест фиксирует, что одной смерти процесса
//! достаточно только для одного запроса: он повторяется на свежем воркере.
//!
//! Нужен интерпретатор Python: `discover_python` (sidecar/python, `.venv` или
//! `HDS_EXTRACT_PYTHON`) — иначе тест пропускается (грабля §9.7.7).

use std::path::PathBuf;
use std::time::Duration;

use hds_extract::worker::{discover_python, Worker, WorkerConfig};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

#[test]
fn crashed_worker_is_restarted_and_request_retried() {
    let root = repo_root();
    let py = match discover_python(&root) {
        Some(p) => p,
        None => {
            println!("пропуск: нет интерпретатора Python (sidecar/python или .venv)");
            return;
        }
    };
    let tests_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut cfg = WorkerConfig::new(&py, &tests_dir);
    cfg.script = tests_dir.join("mock_worker_crashy.py");
    cfg.request_timeout = Duration::from_secs(20);

    // Маркер падения: первый `extract` убивает процесс и создаёт этот файл.
    let marker = std::env::temp_dir().join(format!(
        "hds-crashy-worker-{}-{}.marker",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&marker);

    let mut w = Worker::spawn(cfg).expect("mock-воркер должен запуститься");
    let pid1 = w.pid();

    // Запрос на умирающем воркере: клиент обязан поднять свежий и повторить.
    let r = w
        .extract(&marker)
        .expect("после падения воркера запрос должен выполниться на свежем процессе");
    assert_eq!(r.kind, "text");
    assert_eq!(r.segments.len(), 1);
    assert_ne!(pid1, w.pid(), "воркер должен быть перезапущен");
    assert!(w.is_alive(), "свежий воркер должен быть жив");

    // Клиент не «отравлен»: следующий запрос идёт в тот же процесс.
    let pid2 = w.pid();
    let r2 = w.extract(&marker).expect("второй запрос должен пройти");
    assert_eq!(r2.segments.len(), 1);
    assert_eq!(
        pid2,
        w.pid(),
        "второй запрос не должен требовать перезапуска"
    );

    // Лемматизация тем же объектом (единый объект Extractor+Lemmatizer).
    let lemmas = w.normalize(&["тест".to_string()]).unwrap();
    assert_eq!(lemmas, vec!["M:тест"]);

    w.shutdown();
    assert!(!w.is_alive());
    let _ = std::fs::remove_file(&marker);
}
