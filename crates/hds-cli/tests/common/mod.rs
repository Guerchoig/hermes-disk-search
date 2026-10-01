//! Общие помощники интеграционных тестов `hds-cli`: временный каталог с уборкой.

#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Временный каталог (удаляется в `Drop`). Без внешних зависимостей (`tempfile` offline нет).
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    /// Создать `<TEMP>/hds-cli-test-<tag>-<pid>-<nanos>/`.
    pub fn new(tag: &str) -> TempDir {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!("hds-cli-test-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).expect("создать temp-каталог");
        TempDir { path }
    }

    /// Путь внутри каталога.
    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Записать файл `config.yaml` c заданным телом; вернуть его путь.
    pub fn write_config(&self, body: &str) -> PathBuf {
        let p = self.path.join("config.yaml");
        std::fs::write(&p, body.as_bytes()).expect("записать config.yaml");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
