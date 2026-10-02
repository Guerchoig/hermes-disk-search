//! Пауза индексации (`index.pause`) — механизм ARB-1/ARB-2 (§8.6.2 основного плана).
//!
//! Python-версия понимает паузу как **наличие файла** `index.pause` в корне проекта
//! (`hds/indexer.py`: `if os.path.exists(pause_file)`, `hds/ui_server.py`:
//! `open(_PAUSE, "w").close()`), поэтому здесь файл тоже создаётся пустым — формат
//! обязан совпадать байт-в-байт, иначе UI/CLI не увидят паузу `llm-host`.
//!
//! Два правила, из-за которых это отдельный модуль, а не пара `fs::write`:
//! * **счётчик вложенности**: у `llm-host` может быть несколько одновременных
//!   запросов; пауза снимается только когда завершился последний;
//! * **чужую паузу не снимаем**: если пользователь поставил паузу сам (кнопка в UI),
//!   наш `resume()` обязан её сохранить — иначе запрос молча возобновит индексацию.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Имя файла-сигнала паузы (совпадает с Python).
pub const PAUSE_FILE: &str = "index.pause";
/// Имя файла-сигнала остановки (нужен только для распознавания состояния).
pub const STOP_FILE: &str = "index.stop";
/// Имя кросс-процессного heartbeat-файла индексации.
pub const HEARTBEAT_FILE: &str = "index.heartbeat.json";
/// Свежесть heartbeat, с которой работает UI (`hds/ui_server.py`): 30 с.
pub const HEARTBEAT_FRESH_SECS: u64 = 30;

/// Шлюз паузы индексации: владеет файлом `index.pause` и счётчиком вложенности.
#[derive(Debug)]
pub struct IndexPause {
    path: PathBuf,
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Сколько «leases» держат паузу прямо сейчас.
    depth: usize,
    /// Файл-пауза существовал до нашего первого `pause()` (поставил пользователь).
    user_pause: bool,
    /// Файл создан **нами** — только его мы и удаляем на `resume()`.
    ours_created: bool,
    /// Причина последней нашей паузы (для лога/статуса).
    reason: String,
}

impl IndexPause {
    /// Шлюз для каталога-владельца сигналов (корень проекта в Python-версии).
    pub fn new(dir: impl Into<PathBuf>) -> IndexPause {
        IndexPause {
            path: dir.into().join(PAUSE_FILE),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Путь файла-паузы (для `status`/логов).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Поставлена ли пауза сейчас (наличие файла — как видит Python-индексатор).
    pub fn is_paused(&self) -> bool {
        self.path.is_file()
    }

    /// Сколько наших вложенных пауз держится сейчас.
    pub fn depth(&self) -> usize {
        self.inner.lock().map(|i| i.depth).unwrap_or(0)
    }

    /// Была ли пауза поставлена пользователем (а не нами) — `status` показывает это.
    pub fn user_paused(&self) -> bool {
        self.inner.lock().map(|i| i.user_pause).unwrap_or(false)
    }

    /// Причина текущей паузы (для лога/статуса).
    pub fn reason(&self) -> String {
        self.inner
            .lock()
            .map(|i| i.reason.clone())
            .unwrap_or_default()
    }

    /// Поставить паузу. Возвращает `Ok(true)`, если паузу поставили **мы**
    /// (файл создан этим вызовом), `Ok(false)` — файл уже существовал
    /// (пауза пользователя: переиспользуем, но снимать не будем).
    pub fn pause(&self, reason: &str) -> std::io::Result<bool> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| std::io::Error::other("шлюз паузы: отравленный mutex"))?;
        if inner.depth == 0 {
            inner.user_pause = self.path.is_file();
            inner.ours_created = false;
            if !inner.user_pause {
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                // пустой файл: так же делает UI (`open(_PAUSE, "w").close()`)
                std::fs::write(&self.path, b"")?;
                inner.ours_created = true;
            }
        }
        inner.depth += 1;
        inner.reason = reason.to_string();
        let ours = !inner.user_pause;
        drop(inner);
        Ok(ours)
    }

    /// Снять одну нашу паузу. Возвращает `Ok(true)`, если файл реально удалён.
    ///
    /// Удаляется **только файл, созданный нами** (`ours_created`). Пауза
    /// пользователя или вызов без активной аренды файл не трогают — иначе запрос
    /// молча возобновит индексацию, которую остановил человек.
    pub fn resume(&self) -> std::io::Result<bool> {
        let removed = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| std::io::Error::other("шлюз паузы: отравленный mutex"))?;
            if inner.depth > 0 {
                inner.depth -= 1;
            }
            if inner.depth > 0 || !inner.ours_created {
                inner.reason.clear();
                false
            } else {
                inner.ours_created = false;
                inner.user_pause = false;
                inner.reason.clear();
                true
            }
        };
        if removed && self.path.is_file() {
            std::fs::remove_file(&self.path)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Снять паузу независимо от вложенности и авторства (остановка `llm-host`,
    /// аварийный выход). Возвращает `true`, если файл удалён.
    pub fn force_resume(&self) -> std::io::Result<bool> {
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| std::io::Error::other("шлюз паузы: отравленный mutex"))?;
            inner.depth = 0;
            inner.user_pause = false;
            inner.ours_created = false;
            inner.reason.clear();
        }
        if self.path.is_file() {
            std::fs::remove_file(&self.path)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// RAII-аренда: пауза снимается на `Drop` — даже если запрос упал с ошибкой
    /// (иначе индексация осталась бы стоять до перезапуска `llm-host`).
    pub fn lease(self: &Arc<Self>, reason: &str) -> std::io::Result<PauseLease> {
        self.pause(reason)?;
        Ok(PauseLease {
            gate: Arc::clone(self),
        })
    }
}

/// Аренда паузы: пока живёт объект, счётчик вложенности не нулевой.
#[derive(Debug)]
pub struct PauseLease {
    gate: Arc<IndexPause>,
}

impl PauseLease {
    /// Текущий шлюз (для лога/статуса внутри запроса).
    pub fn gate(&self) -> &Arc<IndexPause> {
        &self.gate
    }
}

impl Drop for PauseLease {
    fn drop(&mut self) {
        let _ = self.gate.resume();
    }
}

/// Состояние индексации по heartbeat-файлу (кросс-процессный статус).
///
/// Поля — как в `ProgressReporter.heartbeat_data()` (`hds/progress.py`), чтобы
/// UI/MCP видели одинаковые цифры. `R30`: свежести файла **недостаточно** —
/// нужен флаг `paused`, иначе паузная/зависшая сессия выглядит как «идёт индексация».
#[derive(Debug, Clone, Default)]
pub struct HeartbeatState {
    /// `ts` не старше [`HEARTBEAT_FRESH_SECS`].
    pub fresh: bool,
    pub paused: bool,
    pub age_secs: u64,
    pub seen: u64,
    pub processed: u64,
    pub errors: u64,
    pub chunks: u64,
    pub total: Option<u64>,
    pub eta_sec: Option<u64>,
    pub remaining: Option<u64>,
    pub rate_min: Option<u64>,
    pub current_path: Option<String>,
    pub phase: Option<String>,
}

impl HeartbeatState {
    /// Живой прогон (свежий heartbeat и не пауза) — для UI-бейджа.
    pub fn is_live(&self) -> bool {
        self.fresh && !self.paused
    }

    /// Человекочитаемое состояние для `llm-host status`.
    pub fn label(&self) -> &'static str {
        match (self.fresh, self.paused) {
            (true, true) => "пауза (index.pause)",
            (true, false) => "идёт индексация",
            (false, _) => "нет свежего heartbeat (прогон завершён или завис)",
        }
    }
}

/// Прочитать `index.heartbeat.json` из каталога сигналов.
/// `None` — файла нет/не разобран (в Python это тоже просто «нет данных»).
pub fn read_heartbeat(dir: &Path) -> Option<HeartbeatState> {
    let text = std::fs::read_to_string(dir.join(HEARTBEAT_FILE)).ok()?;
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let now = unix_now_secs();
    let ts = v.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let age_secs = if ts > 0.0 {
        (now as f64 - ts).max(0.0) as u64
    } else {
        u64::MAX
    };
    let num = |key: &str| v.get(key).and_then(|x| x.as_i64()).map(|x| x.max(0) as u64);
    let mut hb = HeartbeatState {
        fresh: ts > 0.0 && age_secs <= HEARTBEAT_FRESH_SECS,
        paused: v.get("paused").and_then(|x| x.as_bool()).unwrap_or(false),
        age_secs,
        seen: num("seen").unwrap_or(0),
        processed: num("processed").unwrap_or(0),
        errors: num("errors").unwrap_or(0),
        chunks: num("chunks").unwrap_or(0),
        total: num("total"),
        eta_sec: num("eta_sec"),
        remaining: num("remaining"),
        rate_min: num("rate_min"),
        // в heartbeat это плоские ключи `path`/`phase` (см. `heartbeat_data`)
        current_path: v
            .get("path")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        phase: v
            .get("phase")
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
    };
    if ts <= 0.0 {
        hb.paused = false; // без `ts` это не heartbeat, а мусор — не пугаем паузой
    }
    Some(hb)
}

/// Есть ли файл-сигнал остановки (`index.stop`) — для `status`.
pub fn stop_requested(dir: &Path) -> bool {
    dir.join(STOP_FILE).is_file()
}

/// Текущее время в секундах от Unix epoch (без внешних крейтов).
fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
