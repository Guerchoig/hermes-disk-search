//! `hds mcp` — MCP-сервер на stdio (порт `python -m hds.mcp_server`).
//!
//! Блокирующий цикл JSON-RPC 2.0; клиент (Cline/Hermes) держит процесс.

/// `cmd_mcp`: запускает stdio-цикл, возвращает код выхода.
pub fn cmd_mcp() -> i32 {
    hds_mcp::run_stdio()
}
