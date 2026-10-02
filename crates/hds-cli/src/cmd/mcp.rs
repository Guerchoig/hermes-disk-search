//! `hds mcp` — MCP-сервер (порт `python -m hds.mcp_server`).
//!
//! По умолчанию — stdio (каждый клиент держит свой процесс). С `--http` — один
//! streamable-http инстанс на машину (`/health` + `/mcp`), см. `hds mcp-http`.

/// `cmd_mcp`: stdio-цикл или HTTP-сервер; возвращает код выхода.
pub fn cmd_mcp(
    http: bool,
    host: Option<String>,
    port: Option<u16>,
    path: Option<String>,
) -> i32 {
    if !http {
        return hds_mcp::run_stdio();
    }
    let host = host.unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.unwrap_or(8787);
    let path = path.unwrap_or_else(|| "/mcp".to_string());
    match hds_mcp::run_http(&host, port, &path) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

