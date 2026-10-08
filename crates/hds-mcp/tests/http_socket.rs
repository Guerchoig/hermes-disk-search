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

/// F7 (T0): `/health` вне rate-limit — проба менеджера не должна получать 429
/// (иначе живой свой инстанс принимался за «чужой сервис», STATE_FOREIGN).
#[test]
fn health_is_exempt_from_rate_limit() {
    let _env = ENV_LOCK.lock().unwrap();
    let cfg = tmp_cfg(
        "health",
        "http:\n  max_body_kb: 512\n  max_conns: 0\nrate_limit:\n  enabled: true\n  requests_per_min: 60\n  burst: 1\n  login_per_min: 10\n  max_inflight: 4\n",
    );
    let addr = spawn_server(&cfg);

    // Три запроса /health подряд — всегда 200 (под лимит не попадают).
    for _ in 0..3 {
        let resp = http_raw(
            addr,
            b"GET /health HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 200"), "health: {:.90}", resp);
    }
    // Не-health путь под лимитом: burst 1 → первый 404, второй 429.
    let r1 = http_raw(
        addr,
        b"GET /x HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    );
    assert!(r1.starts_with("HTTP/1.1 404"), "{:.90}", r1);
    let r2 = http_raw(
        addr,
        b"GET /x HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    );
    assert!(r2.starts_with("HTTP/1.1 429"), "rate limited: {:.90}", r2);
    // F10.2 (T0): заголовки лимита на 429.
    assert!(r2.contains("Retry-After:"), "{r2}");
    assert!(r2.contains("X-RateLimit-Reset:"), "{r2}");
    assert!(r2.contains("X-RateLimit-Remaining: 0"), "{r2}");

    std::env::remove_var("HDS_CONFIG");
}

/// Тело ответа (после пустой строки заголовков).
fn body_of(resp: &str) -> String {
    resp.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
}

/// F5 (T0): `client_ip` в `/health` — из XFF только при `trusted_proxy: true`
/// (без доверия заголовок подделать IP не даёт).
#[test]
fn health_reports_client_ip_from_xff_only_when_trusted() {
    let _env = ENV_LOCK.lock().unwrap();

    // trusted_proxy: false → XFF игнорируется, client_ip = адрес соединения.
    let cfg1 = tmp_cfg("xff-off", "http:\n  max_conns: 0\ntrusted_proxy: false\n");
    let a1 = spawn_server(&cfg1);
    let r1 = http_raw(
        a1,
        b"GET /health HTTP/1.1\r\nHost: t\r\nX-Forwarded-For: 1.2.3.4\r\nConnection: close\r\n\r\n",
    );
    let v1: serde_json::Value = serde_json::from_str(body_of(&r1).trim()).unwrap();
    assert_eq!(v1["client_ip"], "127.0.0.1", "без доверия XFF: {r1}");

    // trusted_proxy: true → правая (нашим прокси дописанная) запись XFF.
    let cfg2 = tmp_cfg("xff-on", "http:\n  max_conns: 0\ntrusted_proxy: true\n");
    let a2 = spawn_server(&cfg2);
    let r2 = http_raw(
        a2,
        b"GET /health HTTP/1.1\r\nHost: t\r\nX-Forwarded-For: 9.9.9.9, 1.2.3.4\r\nConnection: close\r\n\r\n",
    );
    let v2: serde_json::Value = serde_json::from_str(body_of(&r2).trim()).unwrap();
    assert_eq!(v2["client_ip"], "1.2.3.4", "правая запись XFF: {r2}");

    std::env::remove_var("HDS_CONFIG");
}
