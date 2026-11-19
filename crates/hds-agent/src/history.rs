//! «Очистить историю» (TR-6.6): переписать сохранённые файлы сессии SDK
//! через словарь фильтра (значения → метки) с бэкапом.
//!
//! SDK хранит историю внутри `<agent-state>\.cline\data\sessions\<id>\`:
//! `<id>.json` (манифест) и `<id>.messages.json` (сообщения, вот здесь текст).
//! `sessions.db` текстов не содержит — его чистить не нужно.
//!
//! Перезапись атомарная: сначала `<файл>.bak`, затем временный файл рядом и
//! `rename`. Ошибка на любом шаге возвращается вызывающему; частично
//! перезаписанных файлов не остаётся.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::filter::Filter;

/// Результат очистки.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryOutcome {
    /// Перезаписанные файлы.
    pub files: Vec<PathBuf>,
    /// Бэкапы рядом с файлами.
    pub backups: Vec<PathBuf>,
    /// Сколько раз значения заменены метками (по всем файлам).
    pub replaced: usize,
}

/// Причина отказа очистки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// Файл истории не найден.
    Missing(PathBuf),
    /// Файл не разбирается как JSON.
    Invalid(PathBuf, String),
    /// Словарь недоступен (fail-closed).
    NoDictionary(String),
    /// Ошибка ввода-вывода.
    Io(PathBuf, String),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HistoryError::Missing(path) => write!(f, "файл истории не найден: {}", path.display()),
            HistoryError::Invalid(path, why) => {
                write!(f, "файл истории не разбирается ({}): {why}", path.display())
            }
            HistoryError::NoDictionary(why) => write!(f, "словарь недоступен: {why}"),
            HistoryError::Io(path, why) => {
                write!(f, "сбой ввода-вывода ({}): {why}", path.display())
            }
        }
    }
}

impl std::error::Error for HistoryError {}

/// Каталог сессии внутри `agent-state`.
pub fn session_dir(state_dir: &Path, session_id: &str) -> PathBuf {
    state_dir
        .join(".cline")
        .join("data")
        .join("sessions")
        .join(session_id)
}

/// Файлы истории сессии в порядке очистки.
pub fn session_files(state_dir: &Path, session_id: &str) -> Vec<PathBuf> {
    let dir = session_dir(state_dir, session_id);
    vec![
        dir.join(format!("{session_id}.messages.json")),
        dir.join(format!("{session_id}.json")),
    ]
}

/// Почистить историю сессии. Отсутствующий манифест — не ошибка (SDK мог его
/// не создать), отсутствующий `.messages.json` — ошибка.
pub fn clear(
    state_dir: &Path,
    session_id: &str,
    filter: &Filter,
) -> Result<HistoryOutcome, HistoryError> {
    let mut outcome = HistoryOutcome::default();
    let messages_name = format!("{session_id}.messages.json");
    for path in session_files(state_dir, session_id) {
        if !path.is_file() {
            if path
                .file_name()
                .map(|n| n.to_string_lossy() == messages_name)
                .unwrap_or(false)
            {
                return Err(HistoryError::Missing(path));
            }
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| HistoryError::Io(path.clone(), e.to_string()))?;
        let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| HistoryError::Invalid(path.clone(), e.to_string()))?;
        let cleaned = filter
            .redact_stored(&value)
            .map_err(|e| HistoryError::NoDictionary(e.reason()))?;
        outcome.replaced += cleaned.hits.len();
        let encoded = serde_json::to_string_pretty(&cleaned.payload)
            .map_err(|e| HistoryError::Invalid(path.clone(), e.to_string()))?;
        let backup = path.with_extension("json.bak");
        std::fs::copy(&path, &backup)
            .map_err(|e| HistoryError::Io(backup.clone(), e.to_string()))?;
        atomic_write(&path, &encoded).map_err(|e| HistoryError::Io(path.clone(), e.to_string()))?;
        outcome.backups.push(backup);
        outcome.files.push(path);
    }
    Ok(outcome)
}

/// Прочитать массив сообщений из `.messages.json` для `initialMessages`.
pub fn read_messages(state_dir: &Path, session_id: &str) -> Result<Vec<Value>, HistoryError> {
    let path = session_dir(state_dir, session_id).join(format!("{session_id}.messages.json"));
    if !path.is_file() {
        return Err(HistoryError::Missing(path));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| HistoryError::Io(path.clone(), e.to_string()))?;
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| HistoryError::Invalid(path.clone(), e.to_string()))?;
    Ok(value
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
