//! `index.heartbeat.json` — кросс-процессный статус индексации для UI/MCP
//! (порт части `hds/indexer.py:run_index`) и **R30** (`PLAN_W2_LLM_HOST.md` §5/B4):
//! различать «живой прогон» и «паузную/зависшую сессию» по **времени последнего
//! прогресса**, а не только по свежести файла.
//!
//! Осознанные отличия от Python:
//! * запись **атомарная** (временный файл + `rename`): читатель (UI каждые 2 с)
//!   не может увидеть частичный JSON;
//! * `SessionState` различает `Paused`/`Stale`, поэтому паузная или зависшая
//!   сессия **не блокирует** новый прогон (в Python свежий heartbeat на паузе
//!   блокировал всё — находка W0 `SPIKES.md` §11.1).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Файл `index.heartbeat.json` в корне проекта.
#[derive(Debug, Clone)]
pub struct HeartbeatFile {
    path: PathBuf,
}

impl HeartbeatFile {
    /// `<project_root>/index.heartbeat.json`.
    pub fn new(project_root: &Path) -> Self {
        HeartbeatFile {
            path: project_root.join("index.heartbeat.json"),
        }
    }

    /// Путь файла (для отчётов/тестов).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Атомарная запись: `{"ts": <epoch>, …data}` (ts перезаписывается всегда).
    pub fn write(&self, data: &Value) {
        let mut obj = match data.clone() {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        obj.insert("ts".to_string(), serde_json::json!(epoch_now()));
        let text = Value::Object(obj).to_string();
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, text.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }

    /// Удаление файла (после завершения прогона) — ошибки глушим, как Python.
    pub fn remove(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    /// Разбор файла (`None` — нет файла/битый JSON).
    pub fn read(&self) -> Option<Value> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Возраст файла по `mtime` (секунды; `None` — файла нет).
    pub fn age_secs(&self) -> Option<f64> {
        let meta = std::fs::metadata(&self.path).ok()?;
        let mtime = meta.modified().ok()?;
        let age = SystemTime::now().duration_since(mtime).ok()?;
        Some(age.as_secs_f64())
    }

    /// Время последнего прогресса: `ts` из файла (если есть), иначе `mtime`.
    pub fn last_progress_age(&self) -> Option<f64> {
        if let Some(v) = self.read() {
            if let Some(ts) = v.get("ts").and_then(|t| t.as_f64()) {
                return Some((epoch_now() - ts).max(0.0));
            }
        }
        self.age_secs()
    }

    /// `paused` из файла (false, если поля нет).
    pub fn paused(&self) -> bool {
        self.read()
            .and_then(|v| v.get("paused").and_then(|p| p.as_bool()))
            .unwrap_or(false)
    }
}

fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Состояние индекс-сессии (R30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Файла нет — индексация не запущена.
    None,
    /// Свежий heartbeat и нет паузы — живой прогон.
    Live,
    /// Свежий heartbeat, но `paused: true` — сессия на паузе (`index.pause`).
    Paused,
    /// Heartbeat старше `max_age` — зависшая/закрытая сессия.
    Stale,
}

impl SessionState {
    /// Блокирует ли сессия новый прогон (R30: блокирует только `Live`).
    pub fn blocks_new_run(&self) -> bool {
        matches!(self, SessionState::Live)
    }
}

/// Определение состояния по «времени последнего прогресса» (R30).
pub fn session_state(hb: &HeartbeatFile, max_age: f64) -> SessionState {
    match hb.last_progress_age() {
        None => SessionState::None,
        Some(age) if age >= max_age => SessionState::Stale,
        Some(_) if hb.paused() => SessionState::Paused,
        Some(_) => SessionState::Live,
    }
}

/// Порт `indexer.index_running(max_age=30)`, уточнённый под R30: True только для
/// живого прогона (паузная/зависшая сессия новый прогон не блокирует).
pub fn index_running(hb: &HeartbeatFile, max_age: f64) -> bool {
    session_state(hb, max_age).blocks_new_run()
}

/// Периодический рефреш heartbeat в фоне (порт `_hb_loop`, интервал 5 с).
///
/// `data` вызывается в потоке и должен собирать актуальный снимок
/// (`ProgressReporter::heartbeat_data` + extra). Поток — daemon: при выходе
/// процесса он не мешает завершению (флаг `stop` останавливает его явно).
pub fn spawn_refresh<F>(
    hb: HeartbeatFile,
    stop: Arc<AtomicBool>,
    every: Duration,
    data: F,
) -> std::thread::JoinHandle<()>
where
    F: Fn() -> Value + Send + 'static,
{
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(every);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            hb.write(&data());
        }
    })
}
