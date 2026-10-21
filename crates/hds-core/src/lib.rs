//! `hds-core` — общий слой ядра индексации (`MIGRATION_PLAN_RUST.md` §2.4, §3).
//!
//! Состав на шаг B4:
//! * [`config`] — порт `hds/config.py`: `PROJECT_ROOT`, `APP_NAME`, `EMB_CONTEXT`,
//!   `config_path`, `load`, `dig`, `db_abs_path`, `replace_file`;
//! * [`db`] — порт `hds/db.py`: схема `index.db` **байт-в-байт**, `PRAGMA`
//!   (`journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`,
//!   `busy_timeout=30000`), подключение `sqlite-vec` (vec0) через
//!   `sqlite3_auto_extension`, проба `meta.vec_dim`, бэкфилл `indexed_at`,
//!   CRUD `files`/`chunks`/`chunks_fts`/`chunks_vec`;
//! * [`access`] — идентичность и доступ (M1, `PLAN_M1_ACCESS_PROJECTS.md`
//!   §3.2/§3.3): `data/access.yaml`, пароли Argon2id, opaque-токены;
//! * [`roots`] — корни индексации → проекты (M1, §3.1);
//! * [`service`] — управление OS-службами (M7 T1, `PLAN_M7_UI_CONTROL.md` §3.1):
//!   роли/имена, `sc_args`, LaunchDaemon-плейст, чтение состояния SCM/launchd,
//!   управление через `sc.exe`/`launchctl` (общий код CLI ↔ UI).
//!
//! Правило волны W2: Python-версия (`hds/db.py`, `hds/config.py`) — источник
//! истины по поведению (`PLAN_W2_LLM_HOST.md` §5); расхождения допустимы только
//! осознанные и описанные в doc-комментарии и в `tools/parity/W2_REPORT.md`.
//!
//! Отклонение от §2.4: `chunker` оставлен в `hds-index` (задача B3 уже принята),
//! чтобы не двигать проверенный код; перенос в `hds-core` — отдельная задача.

pub mod access;
pub mod audit;
pub mod client;
pub mod cline;
pub mod config;
pub mod db;
pub mod error;
pub mod http;
pub mod net;
pub mod queue;
pub mod ratelimit;
pub mod roots;
pub mod service;
pub mod tools;

pub use access::{
    access_path, authenticate, hash_password, load as load_access, new_token, resolve_token,
    revoke_token, revoke_tokens_of_user, save as save_access, token_sha256_hex, token_ttl_days,
    tokens_of_user, user_by_login, verify_password, Access, AuthCtx, Token, User,
    DEFAULT_TOKEN_TTL_DAYS,
};
pub use audit::{Audit, AuditEvent};
pub use cline::{cline_dir, mcp_entry, mcp_url, role_ctx_tokens, sync, Report, Step};
pub use config::{
    config_path, db_abs_path, dig, load, load_from, project_root, replace_file, Config, APP_NAME,
    EMB_CONTEXT,
};
pub use db::{connect, has_vec, vec_ok, DbError, FileRow, Stats};
pub use error::{CoreError, Result};
pub use net::{real_client_ip, request_scheme, trusted_proxy};
pub use queue::{HeavyQueue, QueueConfig, QueuePolicy, QueueReject, QueueStats, QueueTicket};
pub use roots::{default_project, project_for_path, roots_from_cfg, roots_paths, Root};
pub use tools::{find_ffmpeg, which};
