//! `hds-extract` — клиент Python-воркера извлечения/нормализации
//! (`MIGRATION_PLAN_RUST.md` §5, задача B6).
//!
//! Воркер (`sidecar/hds_extract/worker.py`) остаётся в Python: извлечение
//! PDF/DOCX/XLSX/PPTX/OCR и лемматизация `pymorphy3` — по §2.5 плана. Общение —
//! **stdio + JSON-RPC 2.0** (NDJSON), родитель владеет процессом (§5.1).
//!
//! Состав:
//! * [`protocol`] — кадрирование/разбор ответов, `Capabilities`, `ExtractResult`,
//!   структурированная ошибка `RpcError`;
//! * [`worker`] — [`Worker`]: поиск интерпретатора, ленивый запуск, `hello`,
//!   `extract`/`normalize`/`clip_image`, перезапуск (после N запросов/падения/
//!   таймаута), `shutdown` (закрытие stdin → штатный выход), `pid` для замера RSS.

pub mod protocol;
pub mod worker;

pub use protocol::{Capabilities, ExtractResult, RpcError, Segment, PROTOCOL_VERSION};
pub use worker::{discover_python, Worker, WorkerConfig};
