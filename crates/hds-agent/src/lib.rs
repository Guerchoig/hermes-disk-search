//! Анонимная зона агента — фаза A3 (`PLAN_AGENT_PRIVACY_ZONE.md` §5.3, §5.5, §5.6).
//!
//! Устройство:
//!
//! * [`worker`] — процесс `node sidecar/hds_agent/dist/worker.mjs` с очищенным
//!   окружением (TR-3.2) и NDJSON-транспортом §4.3: `hello`, `start`, `send`,
//!   `abort`, `stop`, `shutdown` вниз, события вверх, `host_call` вверх с
//!   префиксом `h` (egress_check, office_*, ask_user);
//! * [`filter`] — словарь исходящего на Aho-Corasick (TR-6.2): формы сущностей
//!   проекта, `others`, системные строки, секреты; точки `model`, `mcp_out`,
//!   `mcp_in`; fail-closed `privacy_filter_unavailable` (TR-6.5);
//! * [`deanon`] — деанонимизация событий: метки → значения по снимку vault
//!   (`hds_core::labels`), пути workspace → пути проекта (TR-5.3);
//! * [`gateway`] — шлюз реплик (TR-5.1…5.2), «Очистить историю» (TR-6.6);
//! * [`registry`] — реестр живых процессов (TR-3.9): `max_live_projects`,
//!   выгрузка по простою, переиспользование процесса;
//! * [`projects`] — `projects.json` (TR-8.1);
//! * [`remove`] — удаление проекта: зона, реестр, секреты, запись (A6.5);
//! * [`mcp`] — `cline_mcp_settings.json` (TR-3.6) с подстановкой токена;
//! * [`rules`] — `privacy.md` и `1c-domain.md` (TR-3.8, TR-3.8a);
//! * [`paths`], [`env`] — матрица опасных путей (TR-3.4/TR-3.10а) и окружение
//!   дочернего процесса (TR-3.2).
//!
//! Исходные значения живут только в памяти процесса hds и в `anonymize.db`
//! (I1); в журналы, события и конфиги они не попадают.

pub mod deanon;
pub mod env;
pub mod factory;
pub mod filter;
pub mod gateway;
pub mod history;
pub mod labels;
pub mod mcp;
pub mod mirror;
pub mod paths;
pub mod projects;
pub mod protocol;
pub mod registry;
pub mod registry_db;
pub mod remove;
pub mod results;
pub mod rules;
pub mod run;
pub mod secrets;
pub mod settings;
pub mod validator;
pub mod worker;

pub use filter::{Filter, FilterError, FilterOutcome, Point, Source};
pub use gateway::{Gateway, GatewayEvent};
pub use projects::{Project, Projects};
pub use registry::AgentRegistry;
pub use registry_db::{Artifact, ArtifactRegistry, RegistryError};
pub use remove::{remove_project, RemoveError, RemoveOptions, RemoveReport};
pub use results::{
    ConflictChoice, Decision, DecisionReport, Message, MessageKind, RebuiltImage, RefreshReport,
    ResultsError, BACKUP_SUBDIR, CONFLICTS_DIR, MESSAGES_FILE,
};
pub use settings::{AgentSettings, SyncSettings, UnknownPolicy};
pub use worker::{HostHandler, Worker, WorkerEvent, WorkerSpec};
