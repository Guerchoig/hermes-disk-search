//! Бинарь MCP-сервера disk-search (stdio) — клиенты (Cline/Hermes) запускают его
//! и общаются по JSON-RPC 2.0 (NDJSON). Эквивалент `python -m hds.mcp_server`.

fn main() {
    std::process::exit(hds_mcp::run_stdio());
}
