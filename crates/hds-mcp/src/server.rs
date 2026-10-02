//! MCP-транспорт stdio: NDJSON JSON-RPC 2.0 (`initialize`, `tools/list`,
//! `tools/call`, `ping`). Порт поведения `hds/mcp_server.py` (`mcp.run(stdio)`).

use serde_json::{json, Value};

use crate::schema;

/// Версия протокола MCP, которую объявляем при `initialize`.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Обработать один JSON-RPC объект; `None` — ответ не нужен (notification).
pub fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let is_notification = id.is_none();

    let response = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "disk-search", "version": env!("CARGO_PKG_VERSION") }
        }),
        "tools/list" => json!({ "tools": schema::tool_list() }),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match schema::call(name, &args) {
                Ok(text) => json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }),
                Err(e) => json!({
                    "content": [{ "type": "text", "text": e }],
                    "isError": true
                }),
            }
        }
        "ping" => json!({}),
        "notifications/initialized" | "notifications/cancelled" => return None,
        _ => {
            if is_notification {
                return None;
            }
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }));
        }
    };

    if is_notification {
        return None;
    }
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": response }))
}

/// Обработать строку (для тестов): `None` — ответа нет; `Some` — JSON-строка ответа.
pub fn handle_line(line: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(line.trim()).ok()?;
    handle(&msg).map(|v| v.to_string())
}

/// Запустить MCP-сервер на stdio (блокирующе, до EOF на stdin).
pub fn run_stdio() -> i32 {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line.trim()) {
            Ok(msg) => handle(&msg).map(|v| v.to_string()),
            Err(e) => Some(
                json!({ "jsonrpc": "2.0", "id": Value::Null,
                        "error": { "code": -32700, "message": format!("Parse error: {e}") } })
                .to_string(),
            ),
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
    0
}
