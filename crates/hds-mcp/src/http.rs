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
                                    token: None,
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
                            token: None,
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
    let audit_ev = |event: &str, user: Option<&str>, token: Option<&str>, status: u16| {
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
                    token: token.map(|s| s.into()),
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
            audit_ev("login_fail", None, None, 400);
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
            // F1 (T0): фиксируем выдачу токена — `token_issue` с отпечатком
            // (префикс sha256; сам токен не логируется).
            let fp = hds_core::access::token_sha256_hex(&token);
            let fp = &fp[..12.min(fp.len())];
            audit_ev("login_ok", Some(&ctx.user), None, 200);
            audit_ev("token_issue", Some(&ctx.user), Some(fp), 200);
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
            audit_ev("login_fail", Some(login), None, 401);
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

/// Исход чтения запроса (M3 T4): тело сверх лимита — `413` без чтения тела.
#[allow(clippy::large_enum_variant)]
enum ReadOutcome {
    Ok(String, String, String, ReqHead),
    /// Content-Length выше `http.max_body_kb` — ответить 413.
    TooLarge,
    /// Соединение закрыто/ошибка чтения — молча.
    Closed,
}

/// Дочитать (и отбросить) тело запроса после заголовков — перед отказом.
///
/// F4 (T0): если ответить и закрыть сокет, не прочитав тело, ОС шлёт RST
/// (в приёмном буфере остались непрочитанные данные) — клиент видит
/// «connection reset» вместо кода. Читаем до `content_length`, но не более
/// `CAP` (защита от бесконечного «дочитывания» у враждебного клиента).
fn drain_body(stream: &mut TcpStream, already: usize, content_length: usize) {
    const CAP: usize = 8 * 1024 * 1024;
    let mut remaining = content_length.min(CAP).saturating_sub(already);
    let mut tmp = [0u8; 8192];
    while remaining > 0 {
        let want = remaining.min(tmp.len());
        match stream.read(&mut tmp[..want]) {
            Ok(0) | Err(_) => break,
            Ok(n) => remaining -= n,
        }
    }
}

/// Прочитать один HTTP/1.1-запрос.
fn read_request(stream: &mut TcpStream, max_body: usize) -> ReadOutcome {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    // читаем до конца заголовков
    let header_end = loop {
        let n = match stream.read(&mut tmp) {
            Ok(n) => n,
            Err(_) => return ReadOutcome::Closed,
        };
        if n == 0 {
            return ReadOutcome::Closed;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 1 << 20 {
            return ReadOutcome::Closed;
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
    // M3 T4 (F8): тело сверх лимита — сразу 413. F4 (T0): дочитываем тело
    // (в пределах cap), иначе закрытие с непрочитанным телом → RST.
    if content_length > max_body {
        let already = buf.len().saturating_sub(header_end + 4);
        drain_body(stream, already, content_length);
        return ReadOutcome::TooLarge;
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = match stream.read(&mut tmp) {
            Ok(n) => n,
            Err(_) => return ReadOutcome::Closed,
        };
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    ReadOutcome::Ok(method, path, String::from_utf8_lossy(&body).into_owned(), h)
}

fn write_reply(stream: &mut TcpStream, reply: Reply) {
    write_reply_with(stream, reply, &[]);
}

/// `write_reply` с дополнительными заголовками (Retry-After и т.п.).
fn write_reply_with(
    stream: &mut TcpStream,
    (status, ctype, body): Reply,
    extra: &[(&str, String)],
) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let mut resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Connection: close\r\n",
        body.len()
    );
    for (k, v) in extra {
        resp.push_str(&format!("{k}: {v}\r\n"));
    }
    resp.push_str("\r\n");
    resp.push_str(&body);
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// Ответ `429 Too Many Requests` (M3 T3).
pub fn too_many(inflight: bool, retry: u64) -> Reply {
    (
        429,
        "application/json",
        serde_json::json!({
            "error": "rate limited",
            "hint": if inflight {
                "too many concurrent heavy requests".to_string()
            } else {
                format!("retry after {retry}s")
            }
        })
        .to_string(),
    )
}

/// Заголовки `Retry-After`/`X-RateLimit-Limit` для ответа 429.
pub fn rate_limit_headers(
    limits: &hds_core::ratelimit::Limiter,
    retry: u64,
) -> Vec<(&'static str, String)> {
    vec![
        ("Retry-After", retry.to_string()),
        (
            "X-RateLimit-Limit",
            limits.cfg().requests_per_min.to_string(),
        ),
    ]
}

/// Запустить HTTP-MCP сервер (блокирующе). `path` — endpoint (например `/mcp`).
///
/// Параметры авторизации читаются из `config.yaml` (`access.enabled`,
/// `access.token_ttl_days`); хранилище — `data/access.yaml` (M1 T4).
pub fn run_http(host: &str, port: u16, path: &str) -> Result<(), String> {
    let listener =
        TcpListener::bind((host, port)).map_err(|e| format!("bind {host}:{port}: {e}"))?;
    run_http_listener(listener, path)
}

/// Служить на готовом слушателе (отдельно — для сокет-тестов: тест биндит
/// порт `0` и сам узнаёт адрес).
pub fn run_http_listener(listener: TcpListener, path: &str) -> Result<(), String> {
    let cfg = hds_core::config::load().map_err(|e| e.message())?;
    let env = AuthEnv::from_config(&cfg);
    // T1: доверие forwarded-заголовкам — ключ конфига `trusted_proxy` (раз
    // на процесс; живой конфиг тут не перечитываем, как и `access.enabled`).
    let trusted = hds_core::net::trusted_proxy(&cfg);
    // T3: rate-limit (None — выключен, desktop по умолчанию).
    let limits: Option<std::sync::Arc<hds_core::ratelimit::Limiter>> =
        hds_core::ratelimit::Limiter::from_config(&cfg).map(std::sync::Arc::new);
    // T4: лимит тела и предел соединений (`http.max_body_kb`/`http.max_conns`).
    let max_body = hds_core::net::max_body_bytes(&cfg);
    let max_conns = hds_core::net::max_conns(&cfg);
    let conns = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mcp_path = path.to_string();
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                // T4: сверх предела — короткий 503 без спавна потока.
                if max_conns > 0 && conns.load(std::sync::atomic::Ordering::Relaxed) >= max_conns {
                    // F4 (T0): сначала дочитываем запрос (короткий таймаут), иначе
                    // закрытие сокета с непрочитанными данными даёт RST — клиент
                    // не получает 503.
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                    let _ = read_request(&mut stream, max_body);
                    write_reply_with(
                        &mut stream,
                        (
                            503,
                            "application/json",
                            json!({ "error": "overloaded", "hint": "too many connections, retry later" })
                                .to_string(),
                        ),
                        &[("Retry-After", "2".to_string())],
                    );
                    continue;
                }
                let peer = stream
                    .peer_addr()
                    .map(|a| a.ip().to_string())
                    .unwrap_or_else(|_| "unknown".to_string());
                let mp = mcp_path.clone();
                let env = env.clone();
                let limits = limits.clone();
                let conns = std::sync::Arc::clone(&conns);
                conns.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                std::thread::spawn(move || {
                    let _dec = ConnGuard(&conns);
                    match read_request(&mut stream, max_body) {
                        ReadOutcome::TooLarge => {
                            // T4: тело больше лимита — 413 (F8).
                            write_reply(
                                &mut stream,
                                (
                                    413,
                                    "application/json",
                                    json!({ "error": "payload too large",
                                            "hint": format!("limit is {} KiB (http.max_body_kb)",
                                                max_body / 1024) })
                                    .to_string(),
                                ),
                            );
                        }
                        ReadOutcome::Ok(m, p, b, head) => {
                            let ctx = ServeCtx {
                                mp: &mp,
                                env: &env,
                                trusted,
                                limits: limits.as_deref(),
                            };
                            serve_request(&mut stream, &m, &p, &b, head, &peer, &ctx);
                        }
                        ReadOutcome::Closed => {}
                    }
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

/// RAII-декремент счётчика соединений (на выходе потока).
struct ConnGuard<'a>(&'a std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Drop for ConnGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Контекст сервера для обработки запроса (чтобы не таскать 10 аргументов).
struct ServeCtx<'a> {
    mp: &'a str,
    env: &'a AuthEnv,
    trusted: bool,
    limits: Option<&'a hds_core::ratelimit::Limiter>,
}

/// Обработать прочитанный запрос: rate-limit → audit → route (чистая часть —
/// в [`route`], сокетная — здесь).
fn serve_request(
    stream: &mut TcpStream,
    m: &str,
    p: &str,
    b: &str,
    head: ReqHead,
    peer: &str,
    ctx: &ServeCtx<'_>,
) {
    let ServeCtx {
        mp,
        env,
        trusted,
        limits,
    } = *ctx;
    let ip = hds_core::net::real_client_ip(peer, head.x_forwarded_for.as_deref(), trusted);
    // T3: ключ лимитов — токен, иначе IP.
    let token = auth::bearer_token(head.authorization.as_deref());
    let key = token.map(|s| s.to_string()).unwrap_or_else(|| ip.clone());
    let is_login = p.split('?').next().unwrap_or("") == "/auth/login";
    // Лимит частоты: /auth/login — жёсткий, по IP логина.
    let over = match limits {
        Some(l) if is_login => l.check_login(&ip),
        Some(l) => l.check(&key),
        None => None,
    };
    if let (Some(l), Some(retry)) = (limits, over) {
        let r = too_many(false, retry);
        write_reply_with(stream, r, &rate_limit_headers(l, retry));
        return;
    }
    // Тяжёлый RAG-инструмент — под лимит параллелизма (слот освобождается
    // после route; лимитер — не Arc здесь, поэтому слот снимаем вручную).
    let heavy = b.contains("tools/call") && b.contains("ask_my_files");
    if heavy {
        let acquired = limits.map(|l| l.inflight_acquire(&key)).unwrap_or(true);
        if !acquired {
            if let Some(l) = limits {
                let r = too_many(true, 1);
                write_reply_with(stream, r, &rate_limit_headers(l, 1));
            }
            return;
        }
        let reply = route(m, p, b, mp, env, head.authorization.as_deref(), &ip);
        if let Some(l) = limits {
            l.inflight_release(&key);
        }
        write_reply(stream, reply);
        return;
    }
    write_reply(
        stream,
        route(m, p, b, mp, env, head.authorization.as_deref(), &ip),
    );
}
