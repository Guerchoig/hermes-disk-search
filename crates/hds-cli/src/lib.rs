//! `hds-cli` — CLI ядра индексации (`MIGRATION_PLAN_RUST.md` §2.4, §3.3; задача B7).
//!
//! Подкоманды: `db-move`, `status`, `check`, `reindex`, `reindex-fts`, `forget`,
//! `stop`, `clip-index`, `index`. Python-версия (`hds/cli.py`, `hds/dbops.py`,
//! `hds/diag.py`) — **источник истины** по поведению (`PLAN_W2_LLM_HOST.md` §5);
//! расхождения допустимы только осознанные и описанные в doc-комментарии и в
//! `tools/parity/W2_REPORT.md`.
//!
//! Крейт — библиотека ([`cmd`], [`support`]) + тонкий `[[bin]] hds` (`src/main.rs`):
//! библиотека нужна, чтобы интеграционные тесты вызывали команды с явными
//! путями (temp-конфиг/temp-БД), не трогая боевые `config.yaml`/`index.db`.
//!
//! Зависимостей на `hds-llama` **нет**: фасад `:8010–8012` — только по HTTP
//! (`support::probe_role`), роли не поднимаем (владелец портов — `llm-host`).

pub mod cmd;
pub mod support;
