//! A5 — HTTP-часть фасада: реальные сокеты, поддельный backend (без движка).
//!
//! Проверяем то, что не проверить «на глаз»: коды и формы ответов, keep-alive,
//! `Expect: 100-continue`, отказ от chunked-тела, 404/400/503 и то, что фасад
//! **не** ест чужие порты (в тесте порт эфемерный).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hds_llama::facade::{handle, Backend, ChatRequest, ServerConfig, Usage};
use hds_llama::http;
use serde_json::Value;

/// Подделка инференса: отвечает по контракту, ничего не считая.
struct Fake {
    /// Роль, которую «подняли» (для `/props`).
    up: Vec<String>,
}

impl Backend for Fake {
    fn chat(&self, req: &ChatRequest) -> hds_llama::Result<(String, Usage)> {
        let lt = '\u{3c}';
        let gt = '\u{3e}';
        let text = format!(
            "{lt}think{gt}секрет{lt}/think{gt}Ответ на: {}",
            req.prompt.chars().take(40).collect::<String>()
        );
        Ok((
            text,
            Usage {
                prompt_tokens: 56,
                completion_tokens: 2,
            },
        ))
    }

    fn embeddings(&self, body_json: &str) -> hds_llama::Result<String> {
        let input = serde_json::from_str::<Value>(body_json)
            .ok()
            .and_then(|v| v.get("input").map(|i| i.to_string()))
            .unwrap_or_default();
        Ok(format!(
            r#"{{"data":[{{"embedding":[0.1,0.2,0.3],"index":0}}],"model":"embedding","echo":{input}}}"#
        ))
    }

    fn rerank(&self, body_json: &str) -> hds_llama::Result<String> {
        let _ = body_json;
        Ok(r#"{"results":[{"index":0,"relevance_score":0.9}]}"#.to_string())
    }

    fn props(&self, role: &str) -> Option<Value> {
        if self.up.iter().any(|r| r == role) {
            Some(serde_json::json!({
                "model_path": format!("C:\\llama-runtime\\models\\{role}\\m.gguf"),
                "n_ctx": 32768,
                "total_slots": 1,
            }))
        } else {
            None
        }
    }
}

/// Поднять фасад на эфемерном порту; вернуть `(порт, stop)`.
fn start(cfg: ServerConfig, backend: Arc<Fake>) -> (u16, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("эфемерный порт");
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    std::thread::spawn(move || {
        let handler = move |r: &http::Request| handle(r, &cfg, &*backend);
        let _ = http::serve(listener, stop_thread, handler);
    });
    // ждём готовности (первое соединение может прийти раньше accept-цикла)
    std::thread::sleep(Duration::from_millis(150));
    (port, stop)
}

/// Отправить сырой HTTP-запрос и вернуть `(код, тело)`.
fn send(port: u16, raw: &[u8], read_continue: bool) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(raw).expect("write");
    stream.flush().expect("flush");
    let mut reader = BufReader::new(stream);
    read_response(&mut reader, read_continue)
}

/// Прочитать один HTTP-ответ из уже созданного читателя.
///
/// Читатель **один на соединение**: `BufReader` забуферизует вперёд, поэтому новый
/// reader на каждый ответ «съедал» бы следующий ответ в keep-alive (грабля теста).
fn read_response(reader: &mut BufReader<TcpStream>, read_continue: bool) -> (u16, String) {
    let mut status = 0u16;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return (status, String::new());
        }
        if status == 0 {
            status = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    if status == 100 && read_continue {
        // промежуточный ответ — читаем финальный
        return read_response(reader, false);
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).unwrap();
    }
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Тестовый конфиг: одна роль «chat» на эфемерном порту.
fn cfg_for(port: u16) -> ServerConfig {
    ServerConfig {
        ports: vec![("chat".to_string(), port)],
        ..ServerConfig::default()
    }
}

fn post(port: u16, path: &str, body: &str) -> (u16, String) {
    let raw = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.as_bytes().len()
    );
    send(port, raw.as_bytes(), false)
}

fn get(port: u16, path: &str) -> (u16, String) {
    let raw = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    send(port, raw.as_bytes(), false)
}

/// Основные эндпоинты фасада: живой чат, эмбеддинги, реранк, /health, /props, /v1/models.
#[test]
fn facade_serves_llama_server_compatible_endpoints() {
    let backend = Arc::new(Fake {
        up: vec!["chat".to_string()],
    });
    let (port, stop) = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("порт");
        let p = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let cfg = cfg_for(p);
        let b = Arc::clone(&backend);
        let st = Arc::clone(&stop);
        std::thread::spawn(move || {
            let handler = move |r: &http::Request| handle(r, &cfg, &*b);
            let _ = http::serve(listener, st, handler);
        });
        std::thread::sleep(Duration::from_millis(150));
        (p, stop)
    };

    // /health — 2xx (Python-версия по нему понимает, что порт живой)
    let (status, body) = get(port, "/health");
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["status"], "ok");

    // /props — total_slots >= 1 и model_path (иначе probe вернёт FOREIGN)
    let (status, body) = get(port, "/props");
    assert_eq!(status, 200, "{body}");
    let props: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(props["total_slots"], 1);
    assert!(props["model_path"].as_str().unwrap().ends_with("m.gguf"));
    assert_eq!(props["default_generation_settings"]["n_ctx"], 32768);

    // /v1/models — алиасы
    let (status, body) = get(port, "/v1/models");
    assert_eq!(status, 200);
    let models: Value = serde_json::from_str(&body).unwrap();
    let ids: Vec<String> = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&"chat".to_string()) && ids.contains(&"chat-think".to_string()));

    // чат (мышление выключено по умолчанию) — размышления вырезаны из content
    let (status, body) = post(
        port,
        "/v1/chat/completions",
        r#"{"model":"chat","messages":[{"role":"system","content":"S"},{"role":"user","content":"2+2?"}]}"#,
    );
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["object"], "chat.completion");
    let content = v["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.starts_with("Ответ на: S"), "в промпт попал system: {content}");
    assert!(!content.contains("секрет"), "размышления вырезаны: {content}");
    assert_eq!(v["usage"]["total_tokens"], 58);

    // чат с `chat-think` — размышления видны
    let (status, body) = post(
        port,
        "/v1/chat/completions",
        r#"{"model":"chat-think","messages":[{"role":"user","content":"2+2?"}]}"#,
    );
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["choices"][0]["message"]["reasoning_content"]
        .as_str()
        .unwrap()
        .contains("секрет"));

    // эмбеддинги и реранк — как у llama-server
    let (status, body) = post(port, "/v1/embeddings", r#"{"input":["a","b"],"model":"embedding"}"#);
    assert_eq!(status, 200, "{body}");
    let v: Value = serde_json::from_str(&body).unwrap();
    assert!(v["data"][0]["embedding"].is_array());

    let (status, body) = post(port, "/v1/rerank", r#"{"query":"q","documents":["a"]}"#);
    assert_eq!(status, 200, "{body}");
    assert!(serde_json::from_str::<Value>(&body).unwrap()["results"].is_array());

    stop.store(true, Ordering::Relaxed);
}

/// Ошибки: неизвестный путь (404), пустое/битое тело (400), роль не поднята (503).
#[test]
fn facade_reports_errors_with_clear_messages() {
    let backend = Arc::new(Fake { up: vec![] }); // ни одна роль не поднята
    let (port, stop) = start(cfg_for(0), Arc::clone(&backend));

    // неизвестный путь
    let (status, body) = get(port, "/v1/unknown");
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("/v1/chat/completions"), "подсказка путей: {body}");

    // пустое тело у POST
    let (status, body) = post(port, "/v1/chat/completions", "");
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("пустое"), "{body}");

    // битый JSON
    let (status, body) = post(port, "/v1/chat/completions", "{не json");
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("JSON"), "{body}");

    // нет messages
    let (status, body) = post(port, "/v1/chat/completions", r#"{"model":"chat"}"#);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("messages"), "{body}");

    // stream не поддерживаем — честная ошибка вместо пустого потока
    let (status, body) = post(
        port,
        "/v1/chat/completions",
        r#"{"messages":[{"role":"user","content":"x"}],"stream":true}"#,
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("stream"), "{body}");

    // роль не поднята → /props честно отвечает 503
    let (status, body) = get(port, "/props");
    assert_eq!(status, 503, "{body}");
    assert!(body.contains("инстанс"), "{body}");

    stop.store(true, Ordering::Relaxed);
}

/// Временная диагностика keep-alive — оставлена как «ручной» тест (по умолчанию `#[ignore]`).
///
/// Именно она нашла граблю Windows: `accept()` от неблокирующего слушателя отдаёт
/// неблокирующий сокет, и соединение закрывалось сразу после первого ответа.
#[test]
#[ignore = "диагностика: запускать с --ignored --nocapture, печатает сырые байты"]
fn debug_keepalive_raw() {
    let backend = Arc::new(Fake {
        up: vec!["chat".to_string()],
    });
    let listener = TcpListener::bind("127.0.0.1:0").expect("порт");
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let cfg = cfg_for(port);
    let b = Arc::clone(&backend);
    let st = Arc::clone(&stop);
    std::thread::spawn(move || {
        let handler = move |r: &http::Request| handle(r, &cfg, &*b);
        let _ = http::serve(listener, st, handler);
    });
    std::thread::sleep(Duration::from_millis(150));

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let dump = |stream: &mut TcpStream| -> String {
        let mut out = String::new();
        for _ in 0..8 {
            let mut buf = [0u8; 4096];
            match stream.read(&mut buf) {
                Ok(0) => {
                    out.push_str("\n<EOF>");
                    break;
                }
                Ok(n) => out.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(e) => {
                    out.push_str(&format!("\n<{e:?}>"));
                    break;
                }
            }
        }
        out
    };
    eprintln!("=== первый ответ:{}", dump(&mut stream));
    stream
        .write_all(b"GET /v1/models HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    eprintln!("=== второй ответ:{}", dump(&mut stream));
    stop.store(true, Ordering::Relaxed);
}

#[test]
fn http_transport_handles_keep_alive_continue_and_chunked() {
    let backend = Arc::new(Fake {
        up: vec!["chat".to_string()],
    });
    let listener = TcpListener::bind("127.0.0.1:0").expect("порт");
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let cfg = cfg_for(port);
    let b = Arc::clone(&backend);
    let st = Arc::clone(&stop);
    std::thread::spawn(move || {
        let handler = move |r: &http::Request| handle(r, &cfg, &*b);
        let _ = http::serve(listener, st, handler);
    });
    std::thread::sleep(Duration::from_millis(150));

    // keep-alive: два GET в одном соединении (читатель — один на соединение)
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let (s1, b1) = read_response(&mut reader, false);
    stream
        .write_all(b"GET /v1/models HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let (s2, b2) = read_response(&mut reader, false);
    assert_eq!((s1, s2), (200, 200), "{b1} / {b2}");

    // Expect: 100-continue
    let body = r#"{"messages":[{"role":"user","content":"привет"}]}"#;
    let raw = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\
         Expect: 100-continue\r\n\r\n{body}",
        body.len()
    );
    let (status, text) = send(port, raw.as_bytes(), true);
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("chat.completion"), "{text}");

    // chunked-тело: явный 411 с понятным текстом
    let raw = b"POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
    let (status, text) = send(port, raw, false);
    assert_eq!(status, 411, "{text}");
    assert!(text.contains("Content-Length"), "{text}");

    stop.store(true, Ordering::Relaxed);
}

