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
/// `auth_header` — значение заголовка `Authorization` (если был); `client_ip` —
/// реальный клиентский IP (T1): `remote_addr`, а за доверенным прокси —
/// `X-Forwarded-For` (`hds_core::net::real_client_ip`).
pub fn route(
    method: &str,
    req_path: &str,
    body: &str,
    mcp_path: &str,
    env: &AuthEnv,
    auth_header: Option<&str>,
    client_ip: &str,
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
                "transport": "streamable-http",
                // T1: как сервер видит клиента — живая диагностика прокси
                // (без trusted_proxy здесь будет IP прокси, не клиента).
                "client_ip": client_ip
            })
            .to_string(),
        ),
        ("POST", "/auth/login") if env.enabled => {
            handle_login(body, env, client_ip, std::time::Instant::now())
        }
        ("POST", mp) if mp == mcp_path.trim_end_matches('/') => {
            let started = std::time::Instant::now();
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
                    None => {
                        // M3 T2: аудит неавторизованного доступа.
                        if let Some(a) = &env.audit {
                            a.record(
                                &hds_core::audit::AuditEvent {
                                    ts: 0,
                                    event: "unauthorized".into(),
                                    user: None,
                                    ip: client_ip.into(),
                                    method: method.into(),
                                    path: req_path.into(),
                                    tool: None,
                                    projects: None,
                                    status: 401,
                                    ms: started.elapsed().as_millis() as u64,
                                }
                                .now(),
                            );
                        }
                        return unauthorized();
                    }
                }
            } else {
                None
            };
            // M3 T2: аудит вызовов инструментов (кто/какой инструмент/проекты).
            let tool = (env.enabled
                && msg.get("method").and_then(|m| m.as_str()) == Some("tools/call"))
            .then(|| {
                msg.get("params")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string()
            });
            let reply = match server::handle(&msg, ctx.as_ref()) {
                Some(resp) => (200, "application/json", resp.to_string()),
                None => (202, "application/json", String::new()), // notification
            };
            if let (Some(a), Some(tool)) = (&env.audit, tool) {
                if !tool.is_empty() {
                    a.record(
                        &hds_core::audit::AuditEvent {
                            ts: 0,
                            event: "tool_call".into(),
                            user: ctx.as_ref().map(|c| c.user.clone()),
                            ip: client_ip.into(),
                            method: method.into(),
                            path: req_path.into(),
                            tool: Some(tool),
                            projects: ctx.as_ref().map(|c| c.projects.clone()),
                            status: reply.0,
                            ms: started.elapsed().as_millis() as u64,
                        }
                        .now(),
                    );
                }
            }
            reply
        }
        _ => (
            404,
            "application/json",
            json!({ "error": "not found", "path": req_path }).to_string(),
        ),
    }
}

/// `POST /auth/login {login,password}` → `{token,expires,user,projects}` | 401.
///
/// Аудит (M3 T2): `login_ok` (успех — токен выдан) / `login_fail` (неверные
/// данные или битое тело); тела запроса и токены не логируются.
fn handle_login(body: &str, env: &AuthEnv, client_ip: &str, started: std::time::Instant) -> Reply {
    let audit_ev = |event: &str, user: Option<&str>, status: u16| {
        env.audit.as_ref().map(|a| {
            a.record(
                &hds_core::audit::AuditEvent {
                    ts: 0,
                    event: event.into(),
                    user: user.map(|s| s.into()),
                    ip: client_ip.into(),
                    method: "POST".into(),
                    path: "/auth/login".into(),
                    tool: None,
                    projects: None,
                    status,
                    ms: started.elapsed().as_millis() as u64,
                }
                .now(),
            )
        })
    };
    let v: Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            audit_ev("login_fail", None, 400);
            return (
                400,
                "application/json",
                json!({ "error": format!("bad json: {e}") }).to_string(),
            );
        }
    };
    let login = v.get("login").and_then(|x| x.as_str()).unwrap_or("");
    let password = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    match auth::login(env, login, password) {
        Ok((token, expires, ctx)) => {
            audit_ev("login_ok", Some(&ctx.user), 200);
            (
                200,
                "application/json",
                json!({
                    "token": token,
                    "expires": expires,
                    "user": ctx.user,
                    "projects": ctx.projects,
                })
                .to_string(),
            )
        }
        Err(_) => {
            audit_ev("login_fail", Some(login), 401);
            (
                401,
                "application/json",
                json!({ "error": "неверный логин или пароль" }).to_string(),
            )
        }
    }
}

/// Заголовки запроса, значимые для периметра (M3 T1): авторизация и
/// forwarded-заголовки reverse-proxy (доверие решает `trusted_proxy`).
#[derive(Debug, Default)]
struct ReqHead {
    authorization: Option<String>,
    x_forwarded_for: Option<String>,
    x_forwarded_proto: Option<String>,
}

/// Прочитать один HTTP/1.1-запрос: `(метод, путь, тело, заголовки)`.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String, ReqHead)> {
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
    let mut h = ReqHead::default();
    for l in lines {
        let low = l.to_ascii_lowercase();
        if let Some(v) = low.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = low.strip_prefix("authorization:") {
            h.authorization = Some(v.trim().to_string());
        }
        if let Some(v) = low.strip_prefix("x-forwarded-for:") {
            h.x_forwarded_for = Some(v.trim().to_string());
        }
        if let Some(v) = low.strip_prefix("x-forwarded-proto:") {
            h.x_forwarded_proto = Some(v.trim().to_string());
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
    Some((method, path, String::from_utf8_lossy(&body).into_owned(), h))
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
    // T1: доверие forwarded-заголовкам — ключ конфига `trusted_proxy` (раз
    // на процесс; живой конфиг тут не перечитываем, как и `access.enabled`).
    let trusted = hds_core::net::trusted_proxy(&cfg);
    let listener =
        TcpListener::bind((host, port)).map_err(|e| format!("bind {host}:{port}: {e}"))?;
    let mcp_path = path.to_string();
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                let peer = stream
                    .peer_addr()
                    .map(|a| a.ip().to_string())
                    .unwrap_or_else(|_| "unknown".to_string());
                let mp = mcp_path.clone();
                let env = env.clone();
                std::thread::spawn(move || {
                    if let Some((m, p, b, head)) = read_request(&mut stream) {
                        let ip = hds_core::net::real_client_ip(
                            &peer,
                            head.x_forwarded_for.as_deref(),
                            trusted,
                        );
                        write_reply(
                            &mut stream,
                            route(&m, &p, &b, &mp, &env, head.authorization.as_deref(), &ip),
                        );
                    }
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}
