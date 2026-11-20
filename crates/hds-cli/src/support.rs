//! Общие помощники CLI: конфиг/БД, адресация ролей фасада, резолвинг `shared:<role>`,
//! поиск бинарей в `PATH`, формат даты.
//!
//! Всё здесь — тонкие обёртки над `hds-core`/`hds-index`, сохраняющие поведение
//! Python-версии (`hds/cli.py`, `hds/llama_server.py`, `hds/llama_runtime.py`).
//! Функции принимают конфиг/пути явно (не читают `HDS_CONFIG` сами) — так их
//! проверяют тесты без глобального состояния.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_core::config::{self, dig, Config};
use hds_core::db;
use hds_core::error::{CoreError, Result};
use hds_index::{Embedder, Sidecar};

// Дата-время переехала в `hds_core::timefmt` (A4.4 — одна реализация на
// workspace); реэкспорт сохраняет прежний путь `hds_cli::support::…`.
pub use hds_core::timefmt::{fmt_local_datetime, now_epoch};

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

/// Подключение **писателя** к индексной БД (`open_rw` + `db_abs_path`).
///
/// Писатели: `index`, `reindex`, `reindex-fts`, `forget`, `clip-index`, `watch`.
/// `busy_timeout` — из `db.busy_timeout_ms` (M4 T1).
pub fn open_conn(cfg: &Config) -> Result<rusqlite::Connection> {
    let opts = hds_core::db::DbOptions {
        busy_timeout_ms: config::db_busy_timeout_ms(cfg),
        journal_mode: config::db_journal_mode(cfg),
    };
    db::open_rw(&config::db_abs_path(cfg), dim_of(cfg), &opts)
}

/// Подключение **читателя** к индексной БД: при `db.read_only_conns` (server-дефолт)
/// — read-only соединение без DDL; иначе прежнее rw-поведение (desktop, M4 T1).
///
/// Читатели: `status`, `search`, `ask`.
pub fn open_conn_ro(cfg: &Config) -> Result<rusqlite::Connection> {
    if config::db_read_only_conns(cfg) {
        let opts = hds_core::db::DbOptions {
            busy_timeout_ms: config::db_busy_timeout_ms(cfg),
            journal_mode: config::db_journal_mode(cfg),
        };
        db::open_ro(&config::db_abs_path(cfg), &opts)
    } else {
        open_conn(cfg)
    }
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
