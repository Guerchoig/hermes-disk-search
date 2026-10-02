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
use hds_core::error::{CoreError, Result};
use hds_core::{db, http};
use hds_index::transcribe::MediaRouter;
use hds_index::{Embedder, Sidecar};
use serde_json::Value;

/// Таймаут сетевого зонда `check` (как `_PROBE_TIMEOUT = 3` в Python).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

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

/// Каталог общего llama-рантайма (`llama_runtime.runtime_dir`): env → LOCALAPPDATA/home.
pub fn runtime_dir() -> PathBuf {
    if let Ok(v) = std::env::var("LLAMA_RUNTIME_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(v) = std::env::var("LOCALAPPDATA") {
            if !v.is_empty() {
                return PathBuf::from(v).join("llama-runtime");
            }
        }
        if let Ok(h) = std::env::var("USERPROFILE") {
            return PathBuf::from(h)
                .join("AppData")
                .join("Local")
                .join("llama-runtime");
        }
        PathBuf::from("llama-runtime")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("llama-runtime")
    }
}

/// Порт `llama_runtime.resolve_model` + `llama_server._abs_model`: путь GGUF роли.
///
/// `shared:<role>` → манифест `models/<role>/current.json` (`{"file": …}`), а если
/// манифеста нет — **единственный** `*.gguf` каталога (как Python). Если разрешить
/// не удалось — возвращаем каталог роли (в Python `_abs_model` ловит
/// `FileNotFoundError` и отдаёт `models_dir(role)`).
pub fn resolve_model(cfg: &Config, role: &str) -> PathBuf {
    let spec = dig(cfg, &format!("llm_server.{role}.model"))
        .and_then(|v| v.as_str())
        .unwrap_or(role)
        .trim()
        .to_string();
    if let Some(rest) = spec.strip_prefix("shared:") {
        let r = if rest.trim().is_empty() {
            role
        } else {
            rest.trim()
        };
        let dir = runtime_dir().join("models").join(r);
        if let Some(name) = manifest_file(&dir.join("current.json")) {
            let p = dir.join(&name);
            if p.is_file() {
                return p;
            }
            // имя в манифесте могло отличаться регистром (ФС Windows регистронезависима)
            if let Some(cand) = gguf_files(&dir)
                .into_iter()
                .find(|c| c.file_name().map(|n| n == name.as_str()).unwrap_or(false))
            {
                return cand;
            }
            return dir;
        }
        let ggufs = gguf_files(&dir);
        if ggufs.len() == 1 {
            return ggufs.into_iter().next().unwrap();
        }
        return dir;
    }
    let p = PathBuf::from(&spec);
    if p.is_absolute() {
        p
    } else {
        config::project_root().join(p)
    }
}

/// `*.gguf` в каталоге, отсортированные по имени (порт `sorted(d.glob("*.gguf"))`).
fn gguf_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("gguf"))
                        .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    v
}

/// `{"file": …}` из манифеста модели (UTF-8/BOM); `None` — нет/битый.
fn manifest_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(text).ok()?;
    v.get("file")
        .and_then(|f| f.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Порт пары `(host, port)` роли: `llm_server.host` (127.0.0.1) + `llm_server.<role>.port`.
pub fn role_addr(cfg: &Config, role: &str) -> (String, u16) {
    let host = dig(cfg, "llm_server.host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1")
        .to_string();
    let default = match role {
        "chat" => 8010,
        "embedding" => 8011,
        "rerank" => 8012,
        _ => 8010,
    };
    let port = dig(cfg, &format!("llm_server.{role}.port"))
        .and_then(|v| v.as_u64())
        .unwrap_or(default) as u16;
    (host, port)
}

/// Итог пробы роли (как `STATE_*` в `hds/llama_server.py`).
pub enum Probe {
    /// Живой сервер с ожидаемой моделью (наши `/health` + `/props.total_slots`).
    Llama(Value),
    /// Порт занят, но это не ожидаемый сервер (`/props` без `total_slots`/другая модель).
    Foreign,
    /// Никто не слушает.
    Down,
}

/// Порт `llama_server.probe`: `/health`, затем `/props` (сверка `total_slots` и `model_path`).
pub fn probe_role(cfg: &Config, role: &str, timeout: Duration) -> Probe {
    let (host, port) = role_addr(cfg, role);
    let healthy = http::request(&host, port, "GET", "/health", &[], None, timeout)
        .map(|r| (200..300).contains(&r.status))
        .unwrap_or(false);
    if !healthy {
        return Probe::Down;
    }
    let props = match http::request(&host, port, "GET", "/props", &[], None, timeout) {
        Ok(r) if (200..300).contains(&r.status) => r.json().unwrap_or(Value::Null),
        _ => return Probe::Foreign,
    };
    let slots = props
        .get("total_slots")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if slots < 1 {
        return Probe::Foreign;
    }
    let actual = props
        .get("model_path")
        .or_else(|| props.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !actual.is_empty() {
        let expected = resolve_model(cfg, role);
        if norm_path(actual) != norm_path(&expected.to_string_lossy()) {
            return Probe::Foreign;
        }
    }
    Probe::Llama(props)
}

/// Фактический контекст инстанса из `/props` (`props_context`).
pub fn props_context(props: &Value) -> Option<i64> {
    props
        .get("default_generation_settings")
        .and_then(|d| d.get("n_ctx"))
        .and_then(|v| v.as_i64())
        .or_else(|| props.get("n_ctx").and_then(|v| v.as_i64()))
}

/// Нормализация пути для сравнения (`normcase(normpath())`: общий разделитель + нижний регистр).
pub fn norm_path(s: &str) -> String {
    s.replace('\\', "/").to_lowercase()
}

/// Поиск исполняемого файла в `PATH` (порт `shutil.which` для одного имени).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// `tesseract` доступен: путь из `index.ocr_tesseract_cmd` или `PATH` (как `_tesseract_ready`).
pub fn tesseract_ready(cfg: &Config) -> bool {
    if let Some(cmd) = dig(cfg, "index.ocr_tesseract_cmd").and_then(|v| v.as_str()) {
        if !cmd.trim().is_empty() && Path::new(cmd.trim()).is_file() {
            return true;
        }
    }
    which("tesseract.exe").is_some() || which("tesseract").is_some()
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

/// Дни с 1970-01-01 (Howard Hinnant `days_from_civil`).
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
