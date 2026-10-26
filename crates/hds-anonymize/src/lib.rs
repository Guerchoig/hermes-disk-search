//! `hds-anonymize` — типизированный клиент Python-воркера анонимизации
//! (`PLAN_ANONYMIZER_SIDECAR.md` §2.3).
//!
//! Состав:
//! * [`protocol`] — типы ответов (`HelloInfo`, `AnonymizeResult`, …), схема
//!   выходных имён `<stem>.anonymized.<ext>` / `<stem>.deanon.<ext>`;
//! * [`error`] — машинные коды (`model_missing`, `file_busy`, …);
//! * [`settings`] — чтение `anonymize.*` → аргументы воркера (§2.6);
//! * [`folder`] — пофайловый обход папки (форматы — из `hello.formats`);
//! * [`client`] — запуск воркера и типизированные вызовы RPC.
//!
//! Транспорт переиспользуется из `hds-extract` (`Worker`): progress-нотификации
//! без `id` пропускаются там же (аддитивная доработка §2.3).
//!
//! [`lease`] — P3: GPU-аренда у `llm_host` (`/internal/anon/enter|exit`);
//! `anonymize.gpu: auto` — lease → spawn → акт → shutdown → exit, fallback CPU
//! при недоступности хоста; `off` — всегда CPU.

pub mod client;
pub mod error;
pub mod folder;
pub mod lease;
pub mod protocol;
pub mod settings;

pub use client::{discover_python, Client, ClientConfig};
pub use error::{AnonymizeError, ErrorKind};
pub use folder::list_files;
pub use lease::{LeaseError, LeaseGrant, LeaseGuard};
pub use protocol::{
    AnonymizeResult, DeanonymizeResult, HelloInfo, InspectResult, Mode, Placeholder, Resolved,
    TokenCount, WarmupResult,
};
pub use settings::Settings;
