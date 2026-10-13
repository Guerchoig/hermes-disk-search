//! Логистика тонкого клиента (M9/W7): клиентский пакет скачивается **с сервера**
//! (`<mcp-path>/client-package.zip`), а `/health` публикует версию продукта —
//! по ней мастер клиента понимает, что отстал.
//!
//! Проверяем реальным сокетом: бинарное тело (zip) доходит байт-в-байт (строковый
//! путь `Reply` для этого не годится), без токена — `401`, без вложенного пакета —
//! `404` с подсказкой.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Тесты трогают env (`HDS_CONFIG`/`HDS_ROOT`) — идут последовательно.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Песочница: корень поставки (`HDS_ROOT`) с `config.yaml` и каталогом `client/`.
fn sandbox(tag: &str, access_enabled: bool, version: Option<&str>) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hds-clientpkg-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("client")).unwrap();
    let cfg =
        format!("profile: server\naccess:\n  enabled: {access_enabled}\nhttp:\n  max_conns: 0\n");
    std::fs::write(dir.join("config.yaml"), cfg).unwrap();
    if let Some(v) = version {
        std::fs::write(dir.join("version.txt"), format!("{v}\n")).unwrap();
    }
    std::env::set_var("HDS_ROOT", &dir);
    dir
}

/// Поднять MCP-сервер на порту `0` (как в бою: `run_http_listener`).
fn spawn_server(dir: &Path) -> std::net::SocketAddr {
    std::env::set_var("HDS_CONFIG", dir.join("config.yaml"));
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        let _ = hds_mcp::run_http_listener(l, "/mcp");
    });
    addr
}

/// Сырой GET: `(заголовки, тело-байты)` — тело читаем как байты (zip не UTF-8).
fn get_raw(addr: std::net::SocketAddr, path: &str, bearer: Option<&str>) -> (String, Vec<u8>) {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let auth = match bearer {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: t\r\n{auth}Connection: close\r\n\r\n");
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    match raw.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => (
            String::from_utf8_lossy(&raw[..i]).into_owned(),
            raw[i + 4..].to_vec(),
        ),
        None => (String::from_utf8_lossy(&raw).into_owned(), Vec::new()),
    }
}

/// Пакет-«zip» с байтами, которые сломались бы при проходе через строку.
fn fake_zip() -> Vec<u8> {
    let mut b = b"PK\x03\x04".to_vec();
    b.extend_from_slice(&[0x00, 0xff, 0xfe, 0x80, 0x0a, 0x0d, 0x00, 0x01]);
    b.extend(std::iter::repeat(b'\xc3').take(512));
    b.extend_from_slice(&[0xff, 0xff, 0xfe]);
    b
}

/// Сервер отдаёт вложенный клиентский пакет байт-в-байт (200, zip, имя файла).
#[test]
fn package_bytes_reach_client() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = sandbox("bytes", false, Some("0.19.0"));
    let zip = fake_zip();
    std::fs::write(
        dir.join("client").join("hds-client-0.19.0-windows-x64.zip"),
        &zip,
    )
    .unwrap();
    let addr = spawn_server(&dir);

    let (head, body) = get_raw(addr, "/mcp/client-package.zip", None);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.contains("Content-Type: application/zip"), "{head}");
    assert!(
        head.contains(
            "Content-Disposition: attachment; filename=\"hds-client-0.19.0-windows-x64.zip\""
        ),
        "{head}"
    );
    assert!(
        head.contains(&format!("Content-Length: {}", zip.len())),
        "длина тела из метаданных файла: {head}"
    );
    assert_eq!(
        body, zip,
        "байты пакета не должны портиться (не-UTF-8 включительно)"
    );

    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}

/// При включённой авторизации пакет отдаётся только по своему Bearer-токену
/// (тот же гейт, что у `/mcp`): без токена и с мусором — `401`.
#[test]
fn package_download_requires_bearer() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = sandbox("auth", true, None);
    std::fs::write(
        dir.join("client").join("hds-client-0.19.0-windows-x64.zip"),
        b"PK",
    )
    .unwrap();
    let addr = spawn_server(&dir);

    let (head, body) = get_raw(addr, "/mcp/client-package.zip", None);
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");
    assert!(
        String::from_utf8_lossy(&body).contains("unauthorized"),
        "{head}"
    );
    let (head, _) = get_raw(addr, "/mcp/client-package.zip", Some("garbage"));
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");

    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}

/// Пакета нет (dev-сборка) → `404` и подсказка, где его взять; версия — в ответе.
#[test]
fn missing_package_is_404_with_hint() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = sandbox("missing", false, Some("0.19.0"));
    let addr = spawn_server(&dir);

    let (head, body) = get_raw(addr, "/mcp/client-package.zip", None);
    assert!(head.starts_with("HTTP/1.1 404"), "{head}");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(v["error"], "client-package-missing");
    assert!(
        v["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("build_client_release"),
        "{v}"
    );
    assert_eq!(
        v["version"], "0.19.0",
        "версия из version.txt поставки: {v}"
    );

    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}

/// W7: `/health` публикует версию продукта (мастер сверяет её со своей).
#[test]
fn health_reports_product_version() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = sandbox("health", false, Some("0.19.0"));
    let addr = spawn_server(&dir);

    let (head, body) = get_raw(addr, "/health", None);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let v: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(v["app"], "disk-search");
    assert_eq!(v["version"], "0.19.0", "{v}");

    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}
