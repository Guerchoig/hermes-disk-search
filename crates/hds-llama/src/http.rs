//! Минимальный HTTP/1.1-сервер для локального фасада `:8010–8012`.
//!
//! Почему свой, а не крейт: `crates.io` на машине заказчика **недоступен**
//! (проверено 30.09.2026: `Could not resolve host: index.crates.io`), а зависимостей
//! «из воздуха» в проекте нет. Фасад локальный (один пользователь), поэтому полный
//! HTTP-стек не нужен — достаточно аккуратно разобрать запрос и ответить JSON.
//!
//! Поддерживается: `GET`/`POST`, заголовки, `Content-Length`, `Expect: 100-continue`
//! (его шлют некоторые клиенты), keep-alive (HTTP/1.1 по умолчанию) и `Connection: close`.
//! **Не** поддерживается (явно): `Transfer-Encoding: chunked` в запросе — отвечаем
//! `411 Length Required` с понятным текстом (стандартные клиенты шлют Content-Length).
//! TLS нет: фасад слушает только localhost.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::error::{EngineError, Result};

/// Максимальный размер тела запроса (RAG-контекст с запасом).
pub const MAX_BODY: usize = 32 * 1024 * 1024;

/// Разобранный HTTP-запрос.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: String,
    /// Путь без query-строки.
    pub path: String,
    /// Query-строка (без `?`), если была.
    pub query: String,
    pub body: String,
    pub keep_alive: bool,
}

/// Ответ: статус + JSON-тело либо готовое SSE-тело (`text/event-stream`).
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub json: Value,
    /// Если задано — тело отдаётся как `text/event-stream` (например, стрим чата).
    pub sse: Option<String>,
}

impl Response {
    pub fn ok(json: Value) -> Response {
        Response {
            status: 200,
            json,
            sse: None,
        }
    }

    /// Ответ-поток SSE (`Content-Type: text/event-stream`).
    pub fn sse(body: String) -> Response {
        Response {
            status: 200,
            json: Value::Null,
            sse: Some(body),
        }
    }

    pub fn error(status: u16, message: &str, kind: &str) -> Response {
        Response {
            status,
            json: serde_json::json!({
                "error": { "message": message, "type": kind, "code": Value::Null }
            }),
            sse: None,
        }
    }

    /// Текст статуса HTTP.
    pub fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            411 => "Length Required",
            413 => "Payload Too Large",
            500 => "Internal Server Error",
            501 => "Not Implemented",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            _ => "Error",
        }
    }
}

/// Обслуживать соединения listener'а, пока `stop` не выставлен.
///
/// Обработчик вызывается в потоке соединения; паника в нём не роняет сервер
/// (поток завершается, соединение закрывается — для резидентного процесса это важнее).
pub fn serve<H>(listener: TcpListener, stop: Arc<AtomicBool>, handler: H) -> Result<()>
where
    H: Fn(&Request) -> Response + Send + Sync + 'static,
{
    listener
        .set_nonblocking(true)
        .map_err(|e| EngineError::Other(format!("listener: {e}")))?;
    let handler = Arc::new(handler);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let h = Arc::clone(&handler);
                std::thread::spawn(move || {
                    let _ = handle_connection(stream, &*h);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                // одна ошибка accept не должна ронять фасад
                eprintln!("[facade] accept: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    Ok(())
}

/// Обработать соединение (keep-alive: несколько запросов подряд).
///
/// Грабля Windows (поймана тестом `facade_http`): `accept()` от неблокирующего
/// слушателя отдаёт **неблокирующий** сокет, поэтому вторую строку запроса читать
/// было нельзя — соединение закрывалось сразу после первого ответа. Возвращаем
/// сокет в блокирующий режим и опираемся на таймауты.
fn handle_connection<H>(stream: TcpStream, handler: &H) -> std::io::Result<()>
where
    H: Fn(&Request) -> Response,
{
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(300)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    loop {
        match read_request(&mut reader, &mut writer) {
            Ok(None) => return Ok(()), // соединение закрыто клиентом
            Ok(Some(req)) => {
                let keep_alive = req.keep_alive;
                // CORS-префлайт (OPTIONS): браузерные клиенты (вебвью Cline Desktop)
                // запрашивают каталог моделей `GET /v1/models` с заголовком
                // `Authorization` — такой запрос требует предварительный OPTIONS.
                // Раньше OPTIONS падал 404 (нет маршрута) и реальный GET не делался:
                // список моделей в Cline не обновлялся (проверено 05.10.2026).
                if req.method.eq_ignore_ascii_case("OPTIONS") {
                    write_preflight(&mut writer, keep_alive)?;
                    if !keep_alive {
                        return Ok(());
                    }
                    continue;
                }
                let resp = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&req)))
                    .unwrap_or_else(|_| {
                        Response::error(
                            500,
                            "внутренняя ошибка фасада (паника обработчика)",
                            "server_error",
                        )
                    });
                write_response(&mut writer, &resp, keep_alive)?;
                if !keep_alive {
                    return Ok(());
                }
            }
            // простой дольше таймаута (keep-alive без запросов) — закрываем соединение
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }
}

/// Разобрать тело запроса как JSON (для POST-маршрутов).
pub fn parse_body(req: &Request) -> Result<Value> {
    if req.body.trim().is_empty() {
        return Err(EngineError::Other("тело запроса пустое".to_string()));
    }
    serde_json::from_str(&req.body)
        .map_err(|e| EngineError::Other(format!("тело запроса не JSON: {e}")))
}

/// Прочитать один запрос; `None` — соединение закрыто клиентом.
fn read_request<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    // пустые строки перед запросом игнорируем
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_ascii_uppercase();
    let raw_path = parts.next().unwrap_or("/").to_string();
    let (path, query) = match raw_path.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (raw_path, String::new()),
    };

    let mut content_length: Option<usize> = None;
    let mut keep_alive = true; // HTTP/1.1 по умолчанию
    let mut expect_continue = false;
    let mut chunked = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let l = line.trim();
        if l.is_empty() {
            break;
        }
        let (name, value) = match l.split_once(':') {
            Some((n, v)) => (n.trim().to_ascii_lowercase(), v.trim().to_string()),
            None => continue,
        };
        match name.as_str() {
            "content-length" => content_length = value.parse::<usize>().ok(),
            "connection" => {
                if value.to_ascii_lowercase().contains("close") {
                    keep_alive = false;
                }
            }
            "expect" => {
                if value.to_ascii_lowercase().contains("100-continue") {
                    expect_continue = true;
                }
            }
            "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => {
                chunked = true;
            }
            _ => {}
        }
    }
    if chunked {
        // явный отказ: отвечаем и закрываем соединение
        let resp = Response::error(
            411,
            "фасад не поддерживает Transfer-Encoding: chunked — отправьте тело с Content-Length",
            "invalid_request_error",
        );
        write_response(writer, &resp, false)?;
        return Ok(None);
    }
    if expect_continue {
        writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        writer.flush()?;
    }
    let mut body = String::new();
    if let Some(n) = content_length {
        if n > MAX_BODY {
            let resp = Response::error(
                413,
                &format!("тело запроса больше {MAX_BODY} байт"),
                "invalid_request_error",
            );
            write_response(writer, &resp, false)?;
            return Ok(None);
        }
        if n > 0 {
            let mut buf = vec![0u8; n];
            reader.read_exact(&mut buf)?;
            body = String::from_utf8_lossy(&buf).into_owned();
        }
    }
    Ok(Some(Request {
        method,
        path,
        query,
        body,
        keep_alive,
    }))
}

/// Записать ответ на CORS-префлайт (OPTIONS) — до вызова маршрутов.
///
/// Разрешаем любой origin (`*`, как и в обычных ответах `write_response`) и любые
/// заголовки запроса (`*` поддерживают все современные Chromium-вебвью; запросы
/// клиентов фасада не credentialed). Тело пустое: префлайт отвечает только заголовками.
fn write_preflight<W: Write>(w: &mut W, keep_alive: bool) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Access-Control-Allow-Headers: *\r\n\
         Access-Control-Max-Age: 86400\r\n\r\n",
        if keep_alive { "keep-alive" } else { "close" }
    );
    w.write_all(head.as_bytes())?;
    w.flush()
}

/// Записать ответ (JSON либо SSE + `Content-Length`, keep-alive по запросу).
fn write_response<W: Write>(w: &mut W, resp: &Response, keep_alive: bool) -> std::io::Result<()> {
    let (content_type, body) = match &resp.sse {
        Some(s) => ("text/event-stream; charset=utf-8", s.clone()),
        None => (
            "application/json; charset=utf-8",
            serde_json::to_string(&resp.json).unwrap_or_else(|_| "{}".to_string()),
        ),
    };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\n\
         Content-Length: {}\r\nConnection: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        resp.status,
        resp.reason(),
        content_type,
        body.len(),
        if keep_alive { "keep-alive" } else { "close" }
    );
    w.write_all(head.as_bytes())?;
    w.write_all(body.as_bytes())?;
    w.flush()
}

/// Разобрать `http://host:port/path` (`Result` — только `http://`, см. ниже).
fn split_url(url: &str) -> Result<(String, u16, String)> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        EngineError::Other(format!(
            "поддерживается только http:// (TLS у фасада нет) — получено {url}"
        ))
    })?;
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| EngineError::Other(format!("не разобран порт в URL: {url}")))?,
        ),
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return Err(EngineError::Other(format!("в URL нет хоста: {url}")));
    }
    Ok((host, port, path))
}

/// Мини-клиент HTTP/1.1 (без зависимостей): внутренний CLI (`/internal/*`) и
/// режим `llm_server.mode: facade`.
///
/// Возвращает `(статус, JSON)` — в т.ч. для не-2xx (тело ошибки разбирает
/// вызывающий: `facade::proxy` отдаёт текст апстрима как есть). `chunked`-ответ
/// апстрима не поддержан: наши серверы всегда отвечают с `Content-Length`, а
/// клиенты движка — не наш случай (честная ошибка вместо «пустого JSON»).
pub fn client_json(
    method: &str,
    url: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<(u16, Value)> {
    use std::io::Read;
    use std::net::ToSocketAddrs;

    let (host, port, path) = split_url(url)?;
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| EngineError::Other(format!("не разобран адрес {host}:{port}: {e}")))?
        .next()
        .ok_or_else(|| EngineError::Other(format!("адрес {host}:{port} не разрешился")))?;
    let mut sock = TcpStream::connect_timeout(&addr, timeout.min(Duration::from_secs(10)))
        .map_err(|e| EngineError::Other(format!("не подключиться к {host}:{port}: {e}")))?;
    sock.set_read_timeout(Some(timeout)).ok();
    sock.set_write_timeout(Some(Duration::from_secs(30))).ok();

    let payload = body.unwrap_or("");
    let request = format!(
        "{} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        method.to_ascii_uppercase(),
        payload.len()
    );
    sock.write_all(request.as_bytes())
        .map_err(|e| EngineError::Other(format!("не отправить запрос в {url}: {e}")))?;
    sock.flush().ok();

    let mut buf = Vec::new();
    sock.read_to_end(&mut buf)
        .map_err(|e| EngineError::Other(format!("не прочитать ответ {url}: {e}")))?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body_text) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| EngineError::Other(format!("ответ {url} не HTTP: {:?}", head.trim())))?;
    if head.to_ascii_lowercase().contains("transfer-encoding") {
        return Err(EngineError::Other(format!(
            "{url} ответил chunked-телом — этот клиент его не разбирает"
        )));
    }
    let json = if body_text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(body_text)
            .map_err(|e| EngineError::Other(format!("ответ {url} не JSON: {e}")))?
    };
    Ok((status, json))
}
