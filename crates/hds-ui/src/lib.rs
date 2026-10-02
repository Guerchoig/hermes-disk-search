//! `hds-ui` — минимальный веб-интерфейс на Rust (`MIGRATION_PLAN_RUST.md` §4.1 W1):
//! статус (индекс + роли `llm-host`), поиск, RAG-вопрос, управление индексацией.
//!
//! Это **перепроектированный** UI под Rust-стек (не 1:1-порт `hds/ui_server.py`:
//! тот обслуживал Python-операционку — `llama_server`, скачивание/смену моделей,
//! правку конфига; после перехода на `llm-host` это неактуально). Роли читаются по
//! HTTP из фасада `llm-host` (`/internal/status`).

#![forbid(unsafe_code)]

pub mod page;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};

use hds_core::config::{db_abs_path, dig, load, project_root, Config};
use hds_core::{db, http};
use hds_index::heartbeat::{self, HeartbeatFile};
use hds_index::{Embedder, Sidecar};
use serde_json::{json, Value};

/// Запущена ли фоновая индексация этим UI-процессом.
static INDEXING: AtomicBool = AtomicBool::new(false);

/// Ответ маршрута: `(статус, content-type, тело)`.
pub type Reply = (u16, &'static str, String);

fn json_ok(v: Value) -> Reply {
    (200, "application/json", v.to_string())
}

/// Простое декодирование `%XX` и `+` (для query-параметров).
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Значение query-параметра (`?a=b&c=d`).
pub fn qp(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(pct_decode(v));
            }
        } else if pair == key {
            return Some(String::new());
        }
    }
    None
}

fn connect(cfg: &Config) -> Result<rusqlite::Connection, String> {
    let dim = dig(cfg, "embedding.dim").and_then(|v| v.as_i64()).unwrap_or(1024);
    db::connect(&db_abs_path(cfg), dim).map_err(|e| e.message())
}

fn sidecar() -> Result<Sidecar, String> {
    let root = project_root();
    let py = hds_extract::discover_python(&root)
        .ok_or_else(|| format!("не найден интерпретатор воркера в {}", root.display()))?;
    Sidecar::spawn(&py, &root, false).map_err(|e| e.message())
}

/// Индексация сейчас идёт? (heartbeat или фоновой прогон этого процесса).
pub fn index_running() -> bool {
    INDEXING.load(Ordering::Relaxed)
        || heartbeat::index_running(&HeartbeatFile::new(&project_root()), 30.0)
}

/// `/api/status`: состояние индекса + роли `llm-host` (из фасада `/internal/status`).
pub fn status_json() -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "error": e.message() }),
    };
    let mut index =
        json!({ "running": index_running(), "paused": project_root().join("index.pause").exists() });
    if let Ok(conn) = connect(&cfg) {
        if let Ok(st) = db::stats(&conn) {
            index["chunks"] = json!(st.chunks);
            index["statuses"] = json!(st
                .by_status
                .iter()
                .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_default(), n))
                .collect::<Vec<_>>()
                .join(", "));
            index["kinds"] = json!(st
                .by_kind
                .iter()
                .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_default(), n))
                .collect::<Vec<_>>()
                .join(", "));
            index["errors"] = json!(st.errors.len());
        }
    }
    json!({ "index": index, "llm_host": llm_host_status(&cfg) })
}

/// Роли `llm-host` из фасада (`GET <chat.base_url host:port>/internal/status`).
fn llm_host_status(cfg: &Config) -> Value {
    let base = dig(cfg, "chat.base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://127.0.0.1:8010/v1");
    let (host, port, _) = match hds_index::embed::split_base(base) {
        Ok(x) => x,
        Err(_) => return json!({ "up": false }),
    };
    match http::request(
        &host,
        port,
        "GET",
        "/internal/status",
        &[],
        None,
        std::time::Duration::from_secs(3),
    ) {
        Ok(r) if r.status == 200 => {
            let v = r.json().unwrap_or_else(|_| json!({}));
            let roles = v
                .get("lines")
                .and_then(|l| l.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .filter(|s| {
                            s.contains("chat") || s.contains("embedding") || s.contains("rerank")
                        })
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default();
            json!({
                "up": true,
                "mode": v.get("mode").cloned().unwrap_or(Value::Null),
                "pid": v.get("pid").cloned().unwrap_or(Value::Null),
                "roles": roles,
            })
        }
        _ => json!({ "up": false }),
    }
}

/// `/api/search?q=&limit=&kinds=`.
pub fn search_json(query: &str, limit: i64, kinds: &str) -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "error": e.message() }),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return json!({ "error": e }),
    };
    let emb = Embedder::from_config(&cfg);
    let side = match sidecar() {
        Ok(s) => s,
        Err(e) => return json!({ "error": e }),
    };
    let kinds_v: Option<Vec<String>> = {
        let v: Vec<String> = kinds
            .split(',')
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    };
    let lim = limit.clamp(1, 30) as usize;
    let res = hds_search::search(&conn, Some(&emb), &side, &cfg, query, kinds_v.as_deref(), lim);
    side.shutdown();
    Value::Array(
        res.iter()
            .map(|r| {
                let mut v = r.to_json();
                v["location"] = json!(r.location());
                v
            })
            .collect(),
    )
}

/// `/api/ask?q=`.
pub fn ask_json(question: &str) -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "error": e.message() }),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return json!({ "error": e }),
    };
    let emb = Embedder::from_config(&cfg);
    let side = match sidecar() {
        Ok(s) => s,
        Err(e) => return json!({ "error": e }),
    };
    let out = hds_search::ask(&conn, Some(&emb), &side, &cfg, question, 8);
    side.shutdown();
    out.to_json()
}

/// Управление индексацией (`start`/`stop`/`pause`/`resume`).
pub fn index_action(action: &str, full: bool) -> Value {
    let root = project_root();
    match action {
        "stop" => {
            let _ = std::fs::write(root.join("index.stop"), b"");
            json!({ "ok": true, "msg": "сигнал остановки отправлен" })
        }
        "pause" => {
            let _ = std::fs::write(root.join("index.pause"), b"");
            json!({ "ok": true, "msg": "индексация на паузе" })
        }
        "resume" => {
            let _ = std::fs::remove_file(root.join("index.pause"));
            json!({ "ok": true, "msg": "пауза снята" })
        }
        "start" => {
            if index_running() {
                return json!({ "ok": false, "msg": "индексация уже идёт" });
            }
            INDEXING.store(true, Ordering::Relaxed);
            std::thread::spawn(move || {
                let _ = run_index_bg(full);
                INDEXING.store(false, Ordering::Relaxed);
            });
            json!({ "ok": true, "msg": if full { "запущена полная индексация" } else { "запущена инкрементальная индексация" } })
        }
        other => json!({ "ok": false, "msg": format!("неизвестное действие: {other}") }),
    }
}

fn run_index_bg(full: bool) -> Result<(), String> {
    let cfg = load().map_err(|e| e.message())?;
    let conn = connect(&cfg)?;
    let emb = Embedder::from_config(&cfg);
    let stop = project_root().join("index.stop");
    if stop.exists() {
        let _ = std::fs::remove_file(&stop);
    }
    let side = sidecar()?;
    let media = hds_index::transcribe::MediaRouter::new(&side, &cfg);
    let args = hds_index::RunIndexArgs {
        full,
        prune: true,
        quiet: true,
        ..Default::default()
    };
    let r = hds_index::pipeline::run_index(&conn, &cfg, &emb, &media, &side, &args);
    side.shutdown();
    r.map(|_| ()).map_err(|e| e.message())
}


/// Маршрутизация (чистая функция — тестируется без сокетов).
///
/// `path` — без query; `query` — часть после `?` (может быть пустой).
pub fn route(method: &str, path: &str, query: &str) -> Reply {
    let p = path.trim_end_matches('/');
    match (method, p) {
        ("GET", "") | ("GET", "/index.html") => (200, "text/html; charset=utf-8", page::PAGE.to_string()),
        ("GET", "/api/status") => json_ok(status_json()),
        ("GET", "/api/search") => {
            let q = qp(query, "q").unwrap_or_default();
            if q.trim().is_empty() {
                return (400, "application/json", json!({ "error": "нет параметра q" }).to_string());
            }
            let limit = qp(query, "limit").and_then(|v| v.parse().ok()).unwrap_or(8);
            let kinds = qp(query, "kinds").unwrap_or_default();
            json_ok(search_json(&q, limit, &kinds))
        }
        ("GET", "/api/ask") => {
            let q = qp(query, "q").unwrap_or_default();
            if q.trim().is_empty() {
                return (400, "application/json", json!({ "error": "нет параметра q" }).to_string());
            }
            json_ok(ask_json(&q))
        }
        ("POST", "/api/index/start") => {
            let full = qp(query, "full").map(|v| v == "1" || v == "true").unwrap_or(false);
            json_ok(index_action("start", full))
        }
        ("POST", "/api/index/stop") => json_ok(index_action("stop", false)),
        ("POST", "/api/index/pause") => json_ok(index_action("pause", false)),
        ("POST", "/api/index/resume") => json_ok(index_action("resume", false)),
        _ => (
            404,
            "application/json",
            json!({ "error": "not found", "path": path }).to_string(),
        ),
    }
}

/// Читать один HTTP/1.1-запрос: `(метод, путь, query, тело)`.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
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
    let mut parts = lines.next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
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
    Some((method, path, query, String::from_utf8_lossy(&body).into_owned()))
}

fn write_reply(stream: &mut TcpStream, (status, ctype, body): Reply) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
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

/// Запустить веб-интерфейс (блокирующе).
pub fn run_http(host: &str, port: u16) -> Result<(), String> {
    let listener =
        TcpListener::bind((host, port)).map_err(|e| format!("bind {host}:{port}: {e}"))?;
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                std::thread::spawn(move || {
                    if let Some((m, p, q, b)) = read_request(&mut stream) {
                        let _ = b; // тело UI-эндпоинтов не используется
                        write_reply(&mut stream, route(&m, &p, &q));
                    }
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

