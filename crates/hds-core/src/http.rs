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
    request_stream(
        &Endpoint {
            https: false,
            host,
            port,
        },
        method,
        path,
        headers,
        body,
        timeout,
    )
}

/// M5 T2 (§3.3): разбор полного URL `http(s)://host[:port]/path` — схема
/// (`true` = https), хост, порт (дефолт 80/443) и путь (пустой → `/`).
pub fn parse_url(url: &str) -> Result<(bool, String, u16, String)> {
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(CoreError::Other(format!("не HTTP(S)-адрес: {url}")));
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // hostport может быть IPv6-литералом в скобках ([::1]:8443).
    let (host, port) = if let Some(stripped) = hostport.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((h, rest_p)) => {
                let p = rest_p
                    .strip_prefix(':')
                    .map(|v| v.parse::<u16>())
                    .transpose()
                    .map_err(|e| CoreError::Other(format!("порт в {url}: {e}")))?
                    .unwrap_or(if https { 443 } else { 80 });
                (h.to_string(), p)
            }
            None => return Err(CoreError::Other(format!("плохой IPv6-адрес: {url}"))),
        }
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>()
                    .map_err(|e| CoreError::Other(format!("порт в {url}: {e}")))?,
            ),
            None => (hostport.to_string(), if https { 443 } else { 80 }),
        }
    };
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };
    Ok((https, host, port, path))
}

/// Один HTTP/1.1-запрос по ПОЛНОМУ URL (M5 T2, §3.3): `https` — соединение
/// через TLS (rustls, корни webpki), `http` — как раньше (plain TCP).
pub fn request_url(
    url: &str,
    method: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
    timeout: Duration,
) -> Result<Response> {
    let (https, host, port, path) = parse_url(url)?;
    request_stream(
        &Endpoint {
            https,
            host: &host,
            port,
        },
        method,
        &path,
        headers,
        body,
        timeout,
    )
}

/// Адрес назначения (схема + хост + порт) — чтобы не таскать 8 аргументов.
struct Endpoint<'a> {
    https: bool,
    host: &'a str,
    port: u16,
}

/// Один HTTP/1.1-запрос по частям (схема задаёт TLS/plain).
fn request_stream(
    ep: &Endpoint<'_>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
    timeout: Duration,
) -> Result<Response> {
    let (host, port) = (ep.host, ep.port);
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

    let mut raw = Vec::new();
    if ep.https {
        let mut tls = tls_stream(host, port, stream)?;
        tls.write_all(req.as_bytes())
            .map_err(|e| CoreError::Other(format!("tls write: {e}")))?;
        tls.flush().ok();
        tls.read_to_end(&mut raw)
            .map_err(|e| CoreError::Other(format!("tls read: {e}")))?;
    } else {
        stream
            .write_all(req.as_bytes())
            .map_err(|e| CoreError::Other(format!("write: {e}")))?;
        stream.flush().ok();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| CoreError::Other(format!("read: {e}")))?;
    }
    parse_response(&raw)
}

/// TLS-поток (rustls, корни webpki-roots): только исходящие запросы клиента
/// (M5 §3.3); серверная сторона остаётся без TLS (периметр = reverse-proxy).
fn tls_stream(
    host: &str,
    port: u16,
    tcp: TcpStream,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>> {
    let roots = {
        let mut rs = rustls::RootCertStore::empty();
        rs.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        rs
    };
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| CoreError::Other(format!("имя сервера {host}: {e}")))?;
    let conn = rustls::ClientConnection::new(std::sync::Arc::new(config), name)
        .map_err(|e| CoreError::Other(format!("tls {host}:{port}: {e}")))?;
    Ok(rustls::StreamOwned::new(conn, tcp))
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

#[cfg(test)]
mod tests {
    use super::{parse_url, request_url};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    /// M5 T2 (§3.3): разбор URL — схема, дефолтные порты 80/443, путь, IPv6.
    #[test]
    fn parse_url_scheme_host_port_path() {
        let (https, host, port, path) = parse_url("https://api.example.com/v1").expect("https URL");
        assert!(https);
        assert_eq!(host, "api.example.com");
        assert_eq!(port, 443, "дефолт-порт https");
        assert_eq!(path, "/v1");

        let (https, host, port, path) = parse_url("http://127.0.0.1:8010/v1/").expect("http URL");
        assert!(!https);
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 8010);
        assert_eq!(path, "/v1/");

        let (https, host, port, path) = parse_url("https://h:8443").expect("порт явный");
        assert!(https);
        assert_eq!(host, "h");
        assert_eq!(port, 8443);
        assert_eq!(path, "/", "пустой путь → /");

        let (https, host, port, path) = parse_url("http://[::1]:8443/x").expect("IPv6");
        assert!(!https);
        assert_eq!(host, "::1");
        assert_eq!(port, 8443);
        assert_eq!(path, "/x");

        assert!(parse_url("ftp://x/").is_err(), "не HTTP(S) — ошибка");
    }

    /// M5 T2 (§3.2): `request_url` на живом фейк-сервере OpenAI-формата —
    /// тело доходит, ответ разбирается (как RAG ходит в remote).
    #[test]
    fn request_url_posts_json_and_parses_reply() {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let auth_ok = req.contains("Authorization: Bearer sk-fake");
            let body_ok = req.contains("\"messages\"");
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Connection: close\r\nContent-Length: {}\r\n\r\n{}",
                r#"{"choices":[{"message":{"content":"готово"}}]}"#.len(),
                r#"{"choices":[{"message":{"content":"готово"}}]}"#
            );
            let _ = s.write_all(reply.as_bytes());
            assert!(auth_ok, "Bearer-заголовок доходит: {req}");
            assert!(body_ok, "тело JSON доходит: {req}");
        });
        let url = format!("http://127.0.0.1:{}/v1/chat/completions", addr.port());
        let resp = request_url(
            &url,
            "POST",
            &[
                ("Content-Type", "application/json"),
                ("Authorization", "Bearer sk-fake"),
            ],
            Some(r#"{"model":"m","messages":[]}"#),
            Duration::from_secs(5),
        )
        .expect("запрос к фейк-серверу");
        assert_eq!(resp.status, 200);
        let v = resp.json().expect("JSON ответа");
        assert_eq!(v["choices"][0]["message"]["content"], "готово");
    }
}
