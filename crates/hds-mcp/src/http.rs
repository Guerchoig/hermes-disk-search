//! MCP streamable-http (W1): минимальный HTTP/1.1-сервер с маршрутами `/health`
//! (опознание «наш» инстанс для менеджера `hds mcp-http`) и `<path>` (JSON-RPC 2.0,
//! ответ — `application/json`). Порт поведения `hds/mcp_server.py`/`mcp_http.py`.
//!
//! M1 T4 (`PLAN_M1_ACCESS_PROJECTS.md` §3.3): при `access.enabled=true` все вызовы,
//! кроме `/health` и `POST /auth/login`, требуют `Authorization: Bearer <token>`;
//! контекст токена (проекты/admin) идёт в [`server::handle`]. Без `access.enabled`
//! поведение прежнее (десктоп не ломаем).
//!
//! Свой сервер (не `hds-llama::http`): `hds-mcp` не должен тянуть движок/NVML.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{json, Value};

use crate::auth::{self, AuthEnv};
use crate::server;

/// Ответ маршрута: `(статус, content-type, тело)`.
pub type Reply = (u16, &'static str, String);

/// Ответ 401 (единый формат).
fn unauthorized() -> Reply {
    (
        401,
        "application/json",
        json!({ "error": "unauthorized", "hint": "POST /auth/login" }).to_string(),
    )
}

/// Обработать HTTP-запрос (чистая функция — тестируется без сокетов).
///
/// `auth_header` — значение заголовка `Authorization` (если был).
pub fn route(
    method: &str,
    req_path: &str,
    body: &str,
    mcp_path: &str,
    env: &AuthEnv,
    auth_header: Option<&str>,
) -> Reply {
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
        ("POST", "/auth/login") if env.enabled => handle_login(body, env),
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
            // Авторизация (M1 T4): без токена — 401; с токеном — контекст в handle.
            let ctx = if env.enabled {
                match auth::resolve_bearer(env, auth_header) {
                    Some(c) => Some(c),
                    None => return unauthorized(),
                }
            } else {
                None
            };
            match server::handle(&msg, ctx.as_ref()) {
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

/// `POST /auth/login {login,password}` → `{token,expires,user,projects}` | 401.
fn handle_login(body: &str, env: &AuthEnv) -> Reply {
    let v: Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            return (
                400,
                "application/json",
                json!({ "error": format!("bad json: {e}") }).to_string(),
            )
        }
    };
    let login = v.get("login").and_then(|x| x.as_str()).unwrap_or("");
    let password = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    match auth::login(env, login, password) {
        Ok((token, expires, ctx)) => (
            200,
            "application/json",
            json!({
                "token": token,
                "expires": expires,
                "user": ctx.user,
                "projects": ctx.projects,
            })
            .to_string(),
        ),
        Err(_) => (
            401,
            "application/json",
            json!({ "error": "неверный логин или пароль" }).to_string(),
        ),
    }
}

/// Прочитать один HTTP/1.1-запрос: `(метод, путь, тело, заголовок Authorization)`.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String, Option<String>)> {
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
    let mut authorization: Option<String> = None;
    for l in lines {
        let low = l.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = low.strip_prefix("authorization:") {
            authorization = Some(v.trim().to_string());
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
    Some((
        method,
        path,
        String::from_utf8_lossy(&body).into_owned(),
        authorization,
    ))
}

fn write_reply(stream: &mut TcpStream, (status, ctype, body): Reply) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
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
///
/// Параметры авторизации читаются из `config.yaml` (`access.enabled`,
/// `access.token_ttl_days`); хранилище — `data/access.yaml` (M1 T4).
pub fn run_http(host: &str, port: u16, path: &str) -> Result<(), String> {
    let cfg = hds_core::config::load().map_err(|e| e.message())?;
    let env = AuthEnv::from_config(&cfg);
    let listener =
        TcpListener::bind((host, port)).map_err(|e| format!("bind {host}:{port}: {e}"))?;
    let mcp_path = path.to_string();
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                let mp = mcp_path.clone();
                let env = env.clone();
                std::thread::spawn(move || {
                    if let Some((m, p, b, ah)) = read_request(&mut stream) {
                        write_reply(&mut stream, route(&m, &p, &b, &mp, &env, ah.as_deref()));
                    }
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}
