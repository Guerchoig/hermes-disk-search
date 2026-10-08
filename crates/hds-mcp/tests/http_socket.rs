//! Сокет-тесты hardening M3 T4 / F4 (T0 M4): `413` (тело сверх лимита) и `503`
//! (сверх `http.max_conns`) должны **доходить до клиента**, а не превращаться в
//! `connection reset` из-за закрытия сокета с непрочитанным запросом.
//!
//! Проверка идёт через реальный сокет (`run_http_listener` + `TcpStream`):
//! чистые функции (`route`) этот дефект не ловят.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// Тесты трогают env (`HDS_CONFIG`) — идут последовательно (гонка env).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmp_cfg(tag: &str, body: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("hds-mcpsock-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.yaml");
    std::fs::write(&p, body).unwrap();
    p
}

/// Поднять сервер на порту `0` и вернуть адрес.
fn spawn_server(cfg: &std::path::Path) -> std::net::SocketAddr {
    std::env::set_var("HDS_CONFIG", cfg);
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        let _ = hds_mcp::run_http_listener(l, "/mcp");
    });
    addr
}

/// Сырой запрос: пишем всё, затем читаем ответ (до закрытия).
fn http_raw(addr: std::net::SocketAddr, req: &[u8]) -> String {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(req).expect("write");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp
}

/// F4 (T0): тело сверх `http.max_body_kb` → клиент получает `413`, не RST.
#[test]
fn too_large_body_reaches_client_with_413() {
    let _env = ENV_LOCK.lock().unwrap();
    let cfg = tmp_cfg("413", "http:\n  max_body_kb: 1\n  max_conns: 0\n");
    let addr = spawn_server(&cfg);

    // Двухфазная отправка: сначала заголовки, затем (после паузы) большое тело.
    // Так тело реально остаётся непрочитанным в буфере сервера — без дочитывания
    // закрытие даёт RST и ответа не будет.
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let clen = 200_000usize;
    let head = format!(
        "POST /mcp HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\nContent-Length: {clen}\r\nConnection: close\r\n\r\n"
    );
    s.write_all(head.as_bytes()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let body = vec![b'x'; clen];
    let _ = s.write_all(&body); // при RST может оборваться — это и проверяем
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    assert!(
        resp.starts_with("HTTP/1.1 413"),
        "ожидался 413, получено: {:.120}",
        resp
    );

    std::env::remove_var("HDS_CONFIG");
}

/// F4 (T0): сверх `http.max_conns` → клиент получает `503`, не RST.
#[test]
fn over_conn_limit_reaches_client_with_503() {
    let _env = ENV_LOCK.lock().unwrap();
    let cfg = tmp_cfg("503", "http:\n  max_body_kb: 512\n  max_conns: 1\n");
    let addr = spawn_server(&cfg);

    // Первое соединение держим открытым с НЕПОЛНЫМ запросом — слот занят.
    let mut a = TcpStream::connect(addr).unwrap();
    a.write_all(b"GET /health HTTP/1.1\r\nHost: t\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(300));

    // Второе — полный запрос: сверх лимита соединений → 503.
    let resp = http_raw(
        addr,
        b"GET /health HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    );
    assert!(
        resp.starts_with("HTTP/1.1 503"),
        "ожидался 503, получено: {:.120}",
        resp
    );

    drop(a);
    std::env::remove_var("HDS_CONFIG");
}
