//! Отставание индекса по проекту анонимной зоны (TR-4.7, D9).
//!
//! Образ файла появляется в `workspace` сразу, а в `index.db` попадает позже
//! (очередь `hds watch`, эмбеддинги). Агент не должен считать выдачу полной,
//! когда она неполная, поэтому ему нужен ответ «сколько файлов проекта ещё не
//! в индексе».
//!
//! Критерий «проиндексирован» — как в плане (§5.4 TR-4.7): в таблице `files`
//! есть запись с тем же путём, тем же `content_hash` (blake2b-16 из
//! [`crate::hash`], а не sha256 реестра) и успешным статусом. Быстрый путь —
//! «размер совпал и mtime не старше 2 c» (это ровно критерий `unchanged`, по
//! которому конвейер пропускает файл): иначе каждый вызов `search_local_files`
//! читал бы по 512 КБ на файл.
//!
//! Реестр артефактов (`indexed: pending|done|error`, TR-7.2) — фаза A4; здесь
//! отставание считается по ФС и `index.db` без записи (вопрос владельцу В-7).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use hds_core::config::{project_root, Config};
use hds_core::error::Result;
use hds_core::{db, roots};
use rusqlite::Connection;

use crate::hash::content_hash;
use crate::heartbeat::{self, HeartbeatFile};
use crate::kinds;
use crate::pipeline::{index_filter, mtime_secs, path_str};
use crate::walk::{walk_files, WalkOptions};

/// Сколько путей «в очереди» отдавать в предупреждении выдачи (TR-4.7 п.3).
pub const PENDING_PATHS_LIMIT: usize = 10;

/// Время жизни кэша отставания: агент зовёт поиск несколько раз за ход, а
/// полный обход дерева на каждый вызов дорог. Предупреждение может отставать
/// на этот интервал — это допустимо, точный статус даёт `index_status`.
const LAG_CACHE_TTL: Duration = Duration::from_secs(30);

fn lag_cache() -> &'static Mutex<HashMap<String, (Instant, ProjectLag)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, ProjectLag)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Отставание индекса проекта.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectLag {
    /// Имя проекта (`P0001`).
    pub project: String,
    /// Всего файлов проекта, которые должны быть в индексе.
    pub total: usize,
    /// Проиндексировано (запись с тем же хэшем и успешным статусом).
    pub done: usize,
    /// Ещё не в индексе (нет записи или хэш/статус не совпали).
    pub pending: usize,
    /// Ошибки индексации (запись `status = 'error'` или файл недоступен).
    pub errors: usize,
    /// Индексация идёт или индекс проекта пуст (первичное построение).
    pub building: bool,
    /// Недавно изменённые файлы «в очереди», свежие первыми (до
    /// [`PENDING_PATHS_LIMIT`]).
    pub pending_paths: Vec<String>,
    /// Файлы с ошибкой индексации (до [`PENDING_PATHS_LIMIT`]).
    pub error_paths: Vec<String>,
}

impl ProjectLag {
    /// Есть ли отставание, о котором нужно предупредить агента (TR-4.7 п.3).
    pub fn has_pending(&self) -> bool {
        self.pending > 0
    }
}

/// Посчитать отставание индекса проекта (`P0001`).
///
/// Проект без корня в конфиге → все нули (ошибки нет: агент мог обратиться к
/// проекту, образы которого ещё не созданы).
pub fn project_lag(conn: &Connection, cfg: &Config, project: &str) -> Result<ProjectLag> {
    let mut lag = ProjectLag {
        project: project.to_string(),
        ..ProjectLag::default()
    };
    let Some(root) = roots::effective_roots(cfg)
        .into_iter()
        .find(|r| r.project == project)
    else {
        return Ok(lag);
    };
    let (excludes, limits) = index_filter(cfg);
    let opts = WalkOptions::new(vec![PathBuf::from(&root.path)], excludes);
    let mut files = Vec::new();
    walk_files(&opts, |f| files.push(f.to_path_buf()), |_| {});

    let mut pending: Vec<(f64, String)> = Vec::new();
    for f in files {
        let Some(kind) = kinds::kind_of_path(&f) else {
            continue; // вид не поддержан — в индекс и не попадёт
        };
        if kinds::is_media(kind) || kinds::is_office_lock_file(&f) {
            continue; // медиа индексируется не текстом, `~$` — служебные
        }
        let meta = match std::fs::metadata(&f) {
            Ok(m) => m,
            Err(_) => {
                lag.errors += 1;
                if lag.error_paths.len() < PENDING_PATHS_LIMIT {
                    lag.error_paths.push(path_str(&f));
                }
                continue;
            }
        };
        if !limits.allows(kind, meta.len()) {
            continue; // сверх лимита — конвейер пропустит (`skipped_big`)
        }
        lag.total += 1;
        let size = meta.len();
        let mtime = mtime_secs(&meta);
        let p = path_str(&f);
        match db::get_file_by_path(conn, &p)? {
            None => pending.push((mtime, p)),
            Some(row) if row.status.as_deref() == Some("error") => {
                lag.errors += 1;
                if lag.error_paths.len() < PENDING_PATHS_LIMIT {
                    lag.error_paths.push(p);
                }
            }
            Some(row) => {
                // Быстрый путь — критерий `unchanged` конвейера.
                let unchanged = row.is_indexed()
                    && row.size == Some(size as i64)
                    && (row.mtime_or_zero() - mtime).abs() < 2.0;
                let same_hash = unchanged
                    || matches!(content_hash(&f, size), Ok(h) if row.content_hash.as_deref() == Some(h.as_str()));
                if row.is_indexed() && same_hash {
                    lag.done += 1;
                } else {
                    pending.push((mtime, p));
                }
            }
        }
    }

    pending.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    lag.pending = pending.len();
    lag.pending_paths = pending
        .into_iter()
        .take(PENDING_PATHS_LIMIT)
        .map(|(_, p)| p)
        .collect();
    let running = heartbeat::index_running(&HeartbeatFile::new(&project_root()), 30.0);
    lag.building = running || (lag.total > 0 && lag.done == 0);
    Ok(lag)
}

/// Отставание с кэшем на [`LAG_CACHE_TTL`] — для выдачи `search_local_files`.
///
/// `index_status` зовёт [`project_lag`] напрямую (точный статус), поиск —
/// этот вариант: агент делает несколько запросов за ход, и полный обход
/// дерева на каждый вызов был бы заметной задержкой.
pub fn project_lag_cached(conn: &Connection, cfg: &Config, project: &str) -> Result<ProjectLag> {
    let key = format!(
        "{}|{}",
        hds_core::config::db_abs_path(cfg).display(),
        project
    );
    if let Ok(map) = lag_cache().lock() {
        if let Some((at, lag)) = map.get(&key) {
            if at.elapsed() < LAG_CACHE_TTL {
                return Ok(lag.clone());
            }
        }
    }
    let lag = project_lag(conn, cfg, project)?;
    if let Ok(mut map) = lag_cache().lock() {
        map.insert(key, (Instant::now(), lag.clone()));
    }
    Ok(lag)
}
