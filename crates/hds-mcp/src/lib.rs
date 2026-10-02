//! `hds-mcp` — MCP-сервер disk-search (`MIGRATION_PLAN_RUST.md` §4.1 W1, порт
//! `hds/mcp_server.py`): инструменты поиска/RAG/индексации и stdio-транспорт.
//!
//! Инструменты (`tools.rs`) возвращают готовые строки, как Python; описания и схемы
//! (`schema.rs`) совпадают с `mcp_server.py`; транспорт (`server.rs`) — NDJSON
//! JSON-RPC 2.0. Streamable-http (`mcp_http.py`) — следующий шаг W1.

#![forbid(unsafe_code)]

pub mod http;
pub mod schema;
pub mod server;
pub mod tools;

pub use http::run_http;
pub use server::{handle, handle_line, run_stdio};
