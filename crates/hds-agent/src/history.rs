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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::filter::Filter;

/// Отметка «история очищена» (AG-32): `<agent-state>\history.cleared.json`.
///
/// Внутри только идентификаторы сессий и время — значений из истории файл не
/// содержит (I8). По ней `state_json` показывает предупреждение «история не
/// очищена» (В-A6-10: `warn`, без блокировки и автоочистки).
pub const CLEARED_FILE: &str = "history.cleared.json";

/// Отметки очистки по сессиям: `session_id → unix-время` (секунды).
#[derive(Debug, Default, Serialize, Deserialize)]
struct ClearedMarks {
    #[serde(default)]
    sessions: BTreeMap<String, f64>,
}

fn cleared_path(state_dir: &Path) -> PathBuf {
    state_dir.join(CLEARED_FILE)
}

fn read_marks(state_dir: &Path) -> ClearedMarks {
    std::fs::read_to_string(cleared_path(state_dir))
        .ok()
        .and_then(|text| serde_json::from_str(text.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

/// Записать отметку очистки (ошибка — не повод отменять очистку).
fn mark_cleared(state_dir: &Path, session_id: &str, ts: f64) -> std::io::Result<()> {
    let mut marks = read_marks(state_dir);
    marks.sessions.insert(session_id.to_string(), ts);
    let text = serde_json::to_string_pretty(&marks)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    atomic_write(&cleared_path(state_dir), &text)
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

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
    /// Недопустимый идентификатор сессии (пути, `..`, слишком длинный).
    InvalidSession(String),
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
            HistoryError::InvalidSession(id) => write!(
                f,
                "недопустимый идентификатор сессии {id:?}: только буквы, цифры, '-' и '_' (до 128)"
            ),
            HistoryError::NoDictionary(why) => write!(f, "словарь недоступен: {why}"),
            HistoryError::Io(path, why) => {
                write!(f, "сбой ввода-вывода ({}): {why}", path.display())
            }
        }
    }
}

impl std::error::Error for HistoryError {}

/// Идентификатор сессии: только буквы/цифры/`-`/`_`, до 128 символов.
///
/// `session_id` приходит из CLI (`--session`) и подставляется в путь файлов
/// истории: `..`, `/`, `\` и двоеточие вывели бы чтение/перезапись за пределы
/// каталога сессий.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

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
    if !valid_session_id(session_id) {
        return Err(HistoryError::InvalidSession(session_id.to_string()));
    }
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
    // AG-32: отметка «очищено» — по ней `state_json` перестаёт предупреждать.
    let _ = mark_cleared(state_dir, session_id, now_secs());
    Ok(outcome)
}

/// История сессии на диске есть и менялась после последней очистки (AG-32).
///
/// Читаются только время изменения и отметка `history.cleared.json`; значения
/// из истории не разбираются (I8). Предупреждение ничего не блокирует и не
/// очищает автоматически — решение В-A6-10.
pub fn needs_cleanup(state_dir: &Path, session_id: &str) -> bool {
    if !valid_session_id(session_id) {
        return false;
    }
    let newest = session_files(state_dir, session_id)
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok()?.modified().ok())
        .max();
    let Some(newest) = newest else {
        return false;
    };
    let cleared = read_marks(state_dir)
        .sessions
        .get(session_id)
        .copied()
        .unwrap_or(0.0);
    let modified = newest
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    modified > cleared + 1e-3
}

/// Прочитать массив сообщений из `.messages.json` для `initialMessages`.
///
/// SDK пишет объект `{ "messages": [...] }`; массив верхнего уровня тоже
/// принимаем — формат менялся между версиями (A0 §3.6).
pub fn read_messages(state_dir: &Path, session_id: &str) -> Result<Vec<Value>, HistoryError> {
    if !valid_session_id(session_id) {
        return Err(HistoryError::InvalidSession(session_id.to_string()));
    }
    let path = session_dir(state_dir, session_id).join(format!("{session_id}.messages.json"));
    if !path.is_file() {
        return Err(HistoryError::Missing(path));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| HistoryError::Io(path.clone(), e.to_string()))?;
    let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| HistoryError::Invalid(path.clone(), e.to_string()))?;
    Ok(match value {
        Value::Array(items) => items,
        other => other
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    })
}

/// Последняя (самая свежая) сессия проекта — по времени изменения
/// `.messages.json` в `<state>\.cline\data\sessions\<id>\`.
///
/// Нужна `hds agent history clear` без `--session`: чистить «ту сессию, в
/// которой пользователь работал последней». Нет ни одной — ошибка
/// [`HistoryError::Missing`] с путём каталога сессий.
pub fn latest_session(state_dir: &Path) -> Result<String, HistoryError> {
    let dir = state_dir.join(".cline").join("data").join("sessions");
    let entries = std::fs::read_dir(&dir).map_err(|_| HistoryError::Missing(dir.clone()))?;
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
            continue;
        };
        let messages = path.join(format!("{name}.messages.json"));
        let Ok(meta) = std::fs::metadata(&messages) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if best.as_ref().map(|(ts, _)| modified > *ts).unwrap_or(true) {
            best = Some((modified, name));
        }
    }
    best.map(|(_, name)| name).ok_or(HistoryError::Missing(dir))
}

fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_validated() {
        assert!(valid_session_id("S-OLD_1"));
        assert!(valid_session_id("1794925343101_lh7d7"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("..\\..\\Windows\\evil"));
        assert!(!valid_session_id("a/b"));
        assert!(!valid_session_id("a:b"));
        assert!(!valid_session_id(&"x".repeat(129)));
    }

    #[test]
    fn traversal_session_is_rejected_before_touching_files() {
        let dir = std::env::temp_dir().join(format!("hds-history-{}", std::process::id()));
        let err = read_messages(&dir, "..\\..\\secret").unwrap_err();
        assert!(matches!(err, HistoryError::InvalidSession(_)), "{err}");
    }

    /// AG-32: предупреждение «история не очищена» — по файлам сессии и отметке
    /// `history.cleared.json`; значения из истории не читаются.
    #[test]
    fn cleanup_state_follows_marker() {
        let dir = std::env::temp_dir().join(format!("hds-history-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = "S-CLEAN_1";
        let messages = session_dir(&dir, session).join(format!("{session}.messages.json"));
        std::fs::create_dir_all(messages.parent().unwrap()).unwrap();

        assert!(!needs_cleanup(&dir, session), "файлов нет — чистить нечего");
        std::fs::write(&messages, r#"{"messages":[]}"#).unwrap();
        assert!(needs_cleanup(&dir, session), "история появилась");

        mark_cleared(&dir, session, now_secs() + 5.0).expect("отметка");
        assert!(!needs_cleanup(&dir, session), "отметка новее файла");

        // «SDK дописал историю после очистки» — предупреждение возвращается.
        mark_cleared(&dir, session, 1.0).expect("старая отметка");
        assert!(needs_cleanup(&dir, session), "файл новее отметки");

        assert!(!needs_cleanup(&dir, "../evil"), "плохой id — не читаем");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
