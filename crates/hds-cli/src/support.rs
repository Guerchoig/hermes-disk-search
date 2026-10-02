//! Общие помощники CLI: конфиг/БД, адресация ролей фасада, резолвинг `shared:<role>`,
//! поиск бинарей в `PATH`, формат даты.
//!
//! Всё здесь — тонкие обёртки над `hds-core`/`hds-index`, сохраняющие поведение
//! Python-версии (`hds/cli.py`, `hds/llama_server.py`, `hds/llama_runtime.py`).
//! Функции принимают конфиг/пути явно (не читают `HDS_CONFIG` сами) — так их
//! проверяют тесты без глобального состояния.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hds_core::config::{self, dig, Config};
use hds_core::db;
use hds_core::error::{CoreError, Result};
use hds_index::transcribe::MediaRouter;
use hds_index::{Embedder, Sidecar};

// Проверки окружения переехали в `hds-index::diag` (общий код с веб-интерфейсом);
// реэкспорт сохраняет прежний путь `hds_cli::support::…` (тесты, подкоманды).
pub use hds_index::diag::{
    norm_path, probe_role, props_context, resolve_model, role_addr, runtime_dir, tesseract_ready,
    which, Probe, PROBE_TIMEOUT,
};

/// `embedding.dim` (по умолчанию 1024).
pub fn dim_of(cfg: &Config) -> i64 {
    dig(cfg, "embedding.dim")
        .and_then(|v| v.as_i64())
        .unwrap_or(1024)
}

/// Подключение к индексной БД (`db::connect` + `db_abs_path`) — как `cli._conn`.
pub fn open_conn(cfg: &Config) -> Result<rusqlite::Connection> {
    db::connect(&config::db_abs_path(cfg), dim_of(cfg))
}

/// Клиент эмбеддингов (фасад `:8011`) — как `cli._emb`/`make_embedder`.
pub fn build_embedder(cfg: &Config) -> Embedder {
    Embedder::from_config(cfg)
}

/// Idle-timeout воркера для CLI (`reindex-fts`/`index`): между запросами бывают
/// долгие операции родителя (FTS-`DELETE`, эмбеддинги), за которые дефолтные 60 с
/// воркер успевал выйти по простою. Batch-процесс живёт недолго — держим воркер.
pub const CLI_WORKER_IDLE: Duration = Duration::from_secs(3600);

/// Запуск Python-воркера извлечения/лемматизации (`sidecar/hds_extract/worker.py`).
///
/// Интерпретатор ищется как в `hds-extract` (`HDS_EXTRACT_PYTHON` → `sidecar/python`
/// → `.venv`). Лемматизация в CLI — **только** через воркер (`PLAN_W2_LLM_HOST.md` §5).
pub fn build_sidecar(root: &Path) -> Result<Sidecar> {
    let py = hds_extract::discover_python(root).ok_or_else(|| {
        CoreError::Other(format!(
            "не найден интерпретатор воркера (sidecar/python или .venv\\Scripts\\python.exe) в {}",
            root.display()
        ))
    })?;
    Sidecar::spawn_with(&py, root, false, CLI_WORKER_IDLE)
}

/// Извлекатель боевого пути: медиа (аудио/видео) → владелец GPU по HTTP
/// (`/internal/transcribe`), остальные виды — Python-воркер.
///
/// Возвращает обёртку, которую конвейер (`run_index`/`run_watch`) использует как
/// `&dyn Extractor`; лемматизация остаётся на том же `Sidecar`.
pub fn build_media_extractor<'a>(cfg: &Config, sidecar: &'a Sidecar) -> MediaRouter<&'a Sidecar> {
    MediaRouter::new(sidecar, cfg)
}

/// `--roots a;b;c` → список путей (пустые элементы игнорируются, как `args.roots.split(';')`).
pub fn parse_roots(s: &str) -> Vec<PathBuf> {
    s.split(';')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// `--kinds text,pdf` → `Some(["text","pdf"])` или `None` (`cli._kinds`).
pub fn parse_kinds(s: &str) -> Option<Vec<String>> {
    let parts: Vec<String> = s
        .split(',')
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

/// `time.time()`.
pub fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Локальная дата-время из epoch-секунд (порт `datetime.fromtimestamp(ts)`):
/// `YYYY-MM-DD HH:MM:SS` и микросекунды, если ненулевые (как Python `str(datetime)`).
pub fn fmt_local_datetime(ts: f64) -> String {
    let local = ts + local_offset_secs() as f64;
    let mut secs = local.floor() as i64;
    let mut micro = (local.fract() * 1_000_000.0).round() as i64;
    if micro >= 1_000_000 {
        secs += 1;
        micro = 0;
    }
    let (y, m, d, hh, mm, ss) = civil_from_epoch(secs);
    if micro == 0 {
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}.{micro:06}")
    }
}

/// Смещение локального времени от UTC (секунды), округлённое до минуты.
#[cfg(windows)]
fn local_offset_secs() -> i64 {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(p: *mut SystemTime);
    }
    let mut st = SystemTime::default();
    unsafe { GetLocalTime(&mut st) };
    let local = days_from_civil(st.year as i64, st.month as u32, st.day as u32) * 86400
        + st.hour as i64 * 3600
        + st.minute as i64 * 60
        + st.second as i64;
    let utc = now_epoch() as i64;
    ((local - utc) as f64 / 60.0).round() as i64 * 60
}

/// Вне Windows локальное смещение не вычисляем — используем UTC.
#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

/// Дни с 1970-01-01 (Howard Hinnant `days_from_civil`). Нужен только Windows-ветке.
#[cfg(windows)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `(year, month, day, hour, min, sec)` из epoch-секунд (UTC).
fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hh, mm, ss) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hh, mm, ss)
}
