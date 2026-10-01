//! `hds-core` — общий слой ядра индексации (`MIGRATION_PLAN_RUST.md` §2.4, §3).
//!
//! Состав на шаг B4:
//! * [`config`] — порт `hds/config.py`: `PROJECT_ROOT`, `APP_NAME`, `EMB_CONTEXT`,
//!   `config_path`, `load`, `dig`, `db_abs_path`, `replace_file`;
//! * [`db`] — порт `hds/db.py`: схема `index.db` **байт-в-байт**, `PRAGMA`
//!   (`journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`,
//!   `busy_timeout=30000`), подключение `sqlite-vec` (vec0) через
//!   `sqlite3_auto_extension`, проба `meta.vec_dim`, бэкфилл `indexed_at`,
//!   CRUD `files`/`chunks`/`chunks_fts`/`chunks_vec`.
//!
//! Правило волны W2: Python-версия (`hds/db.py`, `hds/config.py`) — источник
//! истины по поведению (`PLAN_W2_LLM_HOST.md` §5); расхождения допустимы только
//! осознанные и описанные в doc-комментарии и в `tools/parity/W2_REPORT.md`.
//!
//! Отклонение от §2.4: `chunker` оставлен в `hds-index` (задача B3 уже принята),
//! чтобы не двигать проверенный код; перенос в `hds-core` — отдельная задача.

pub mod config;
pub mod db;
pub mod error;
pub mod http;

pub use config::{
    config_path, db_abs_path, dig, load, load_from, project_root, replace_file, Config,
    APP_NAME, EMB_CONTEXT,
};
pub use db::{connect, has_vec, vec_ok, DbError, FileRow, Stats};
pub use error::{CoreError, Result};
