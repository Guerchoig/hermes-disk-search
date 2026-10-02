//! MCP streamable-http (W1): минимальный HTTP/1.1-сервер с маршрутами `/health`
//! (опознание «наш» инстанс для менеджера `hds mcp-http`) и `<path>` (JSON-RPC 2.0,
//! ответ — `application/json`). Порт поведения `hds/mcp_server.py`/`mcp_http.py`.
//!
//! Свой сервер (не `hds-llama::http`): `hds-mcp` не должен тянуть движок/NVML.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{json, Value};

use crate::server;

/// Ответ маршрута: `(статус, content-type, тело)`.
pub type Reply = (u16, &'static str, String);

/// Обработать HTTP-запрос (чистая функция — тестируется без сокетов).
pub fn route(method: &str, req_path: &str, body: &str, mcp_path: &str) -> Reply {
    let p = req_path
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    match (method, p) {
        ("GET", "/health") => (
            200,
            "application/json",
            json!({
                "app": hds_core::config::APP_NAME,
                "version": env!("CARGO_PKG_VERSION"),
                "transport": "streamable-http"
            })
            .to_string(),
        ),
        ("POST", mp) if mp == mcp_path.trim_end_matches('/') => {
            let msg: Value = match serde_json::from_str(body.trim()) {
                Ok(v) => v,
                Err(e) => {
                    return (
                        200,
                        "application/json",
                        json!({ "jsonrpc": "2.0", "id": Value::Null,
                                "error": { "code": -32700, "message": format!("Parse error: {e}") } })
                        .to_string(),
                    )
                }
            };
            match server::handle(&msg) {
                Some(resp) => (200, "application/json", resp.to_string()),
                None => (202, "application/json", String::new()), // notification
            }
        }
        _ => (
            404,
            "application/json",
            json!({ "error": "not found", "path": req_path }).to_string(),
        ),
    }
}

/// Прочитать один HTTP/1.1-запрос: `(метод, путь, тело)`.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    // читаем до конца заголовков
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 1 << 20 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut content_length = 0usize;
    for l in lines {
        let low = l.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Some((method, path, String::from_utf8_lossy(&body).into_owned()))
}

fn write_reply(stream: &mut TcpStream, (status, ctype, body): Reply) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        _ => "OK",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// Запустить HTTP-MCP сервер (блокирующе). `path` — endpoint (например `/mcp`).
pub fn run_http(host: &str, port: u16, path: &str) -> Result<(), String> {
    let listener =
        TcpListener::bind((host, port)).map_err(|e| format!("bind {host}:{port}: {e}"))?;
    let mcp_path = path.to_string();
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                let mp = mcp_path.clone();
                std::thread::spawn(move || {
                    if let Some((m, p, b)) = read_request(&mut stream) {
                        write_reply(&mut stream, route(&m, &p, &b, &mp));
                    }
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}
