//! Машинные коды ошибок воркера (`kind`, §3) и их тексты для UI/CLI.

use std::fmt;

/// Машинный код — по нему UI/CLI выбирают текст и поведение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Веса NER-моделей не установлены (`--models-dir` пуст).
    ModelMissing,
    /// Файл занят (открыт в Word/LibreOffice).
    FileBusy,
    /// Формат файла не поддерживается (P1/P1b-набор).
    Unsupported,
    /// БД токенов отсутствует/пуста (деанонимизация невозможна).
    DbMissing,
    /// Выходной путь вне разрешённого каталога.
    OutOfDir,
    /// GPU занят chat-сеансом, lease не дождался (P3).
    GpuBusy,
    /// Ошибка в параметрах запроса.
    BadRequest,
    /// Прочее: сбой транспорта/воркера.
    Internal,
}

impl ErrorKind {
    /// Короткая подпись в ответах пользователю.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::ModelMissing => "model_missing",
            ErrorKind::FileBusy => "file_busy",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::DbMissing => "db_missing",
            ErrorKind::OutOfDir => "out_of_dir",
            ErrorKind::GpuBusy => "gpu_busy",
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Internal => "internal",
        }
    }
}

/// Ошибка акта анонимизации (транспорт `hds-extract` или `kind` воркера).
#[derive(Debug, Clone)]
pub struct AnonymizeError {
    pub kind: ErrorKind,
    pub message: String,
    pub hint: Option<String>,
}

impl AnonymizeError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        AnonymizeError {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AnonymizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(h) = &self.hint {
            write!(f, " — {h}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AnonymizeError {}

/// Маппинг ошибки транспорта (+ `kind`/`hint` воркера) в доменную ошибку.
pub fn from_transport(
    err: &hds_core::error::CoreError,
    rpc_kind: Option<&str>,
    hint: Option<&str>,
) -> AnonymizeError {
    let kind = match rpc_kind {
        Some("model_missing") => ErrorKind::ModelMissing,
        Some("file_busy") => ErrorKind::FileBusy,
        Some("unsupported") => ErrorKind::Unsupported,
        Some("db_missing") => ErrorKind::DbMissing,
        Some("out_of_dir") => ErrorKind::OutOfDir,
        Some("gpu_busy") => ErrorKind::GpuBusy,
        _ => ErrorKind::Internal,
    };
    let e = AnonymizeError::new(kind, err.to_string());
    match hint {
        Some(h) => e.with_hint(h),
        None => e,
    }
}
