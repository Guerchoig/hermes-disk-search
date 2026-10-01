//! Ошибки общего слоя (`hds-core`): БД, конфиг, файловые операции.

use std::io;

/// Результат операций `hds-core`.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Ошибки слоя: sqlite, ввод-вывод, YAML и «прочее» с текстом как в Python.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("{0}")]
    Other(String),
}

impl CoreError {
    /// Строка для `files.error` (как `str(e)` в Python — обрезается вызывающим).
    pub fn message(&self) -> String {
        self.to_string()
    }
}
