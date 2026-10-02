//! Свой мини-HTTP-клиент (без внешних зависимостей): `crates.io` на машине
//! заказчика недоступен (`tools/parity/README.md` §3, п. 13), поэтому HTTP-слой
//! пишем сами — как в `hds-llama::http`.
//!
//! Область применения B4: POST JSON к фасаду эмбеддингов `:8011/v1/embeddings`.
//! Соединение — на один запрос (`Connection: close`); keep-alive и полная
//! унификация с `hds-llama::http` — задача W1 (после переезда поиска).
//!
//! Поддерживается ответ `Content-Length` и `Transfer-Encoding: chunked`
//! (llama-server/фасад могут ответить обоими способами).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::error::{CoreError, Result};

/// Ответ HTTP: статус и тело (UTF-8, ошибки декодирования — replacement).
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    /// Разбор тела как JSON.
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_str(&self.body)
            .map_err(|e| CoreError::Other(format!("не JSON в ответе: {e}")))
    }
}

/// Один HTTP/1.1-запрос. `path` — с ведущим `/` (например `/v1/embeddings`).
pub fn request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
    timeout: Duration,
) -> Result<Response> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| CoreError::Other(format!("адрес {host}:{port}: {e}")))?
        .next()
        .ok_or_else(|| CoreError::Other(format!("адрес {host}:{port} не разрешился")))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| CoreError::Other(format!("connect {host}:{port}: {e}")))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("Connection: close\r\n\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    stream
        .write_all(req.as_bytes())
        .map_err(|e| CoreError::Other(format!("write: {e}")))?;
    stream.flush().ok();

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| CoreError::Other(format!("read: {e}")))?;
    parse_response(&raw)
}

/// Разбор сырого ответа: строка статуса, заголовки, тело (CL или chunked).
pub fn parse_response(raw: &[u8]) -> Result<Response> {
    let sep = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| CoreError::Other("нет конца заголовков в ответе".into()))?;
    let head = String::from_utf8_lossy(&raw[..sep]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| CoreError::Other(format!("плохая строка статуса: {status_line}")))?;
    let mut chunked = false;
    for l in lines {
        let low = l.to_lowercase();
        if low.starts_with("transfer-encoding:") && low.contains("chunked") {
            chunked = true;
        }
    }
    let body_bytes = &raw[sep + 4..];
    let body = if chunked {
        decode_chunked(body_bytes)
    } else {
        body_bytes.to_vec()
    };
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// Декодирование `Transfer-Encoding: chunked` (без trailers).
fn decode_chunked(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(nl) = data.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&data[..nl]);
        let size = match usize::from_str_radix(size_str.trim().split(';').next().unwrap_or(""), 16)
        {
            Ok(s) => s,
            Err(_) => break,
        };
        if size == 0 {
            break;
        }
        let start = nl + 2;
        if start + size > data.len() {
            out.extend_from_slice(&data[start..]);
            break;
        }
        out.extend_from_slice(&data[start..start + size]);
        data = &data[start + size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    out
}
