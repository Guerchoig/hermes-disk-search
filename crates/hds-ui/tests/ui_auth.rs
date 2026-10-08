//! Тесты гейта UI (M1 T5): десктоп — без токена; server-режим (access.enabled)
//! — только с админ-токеном; API доступа работает под админ-токеном.
//!
//! HDS_CONFIG — единственная переменная окружения, поэтому все шаги в ОДНОМ
//! тесте (последовательно), чтобы не было гонки env между тестами.

use hds_core::access::{hash_password, load_at, save_at, Access, User};
use hds_ui::route_with_access;
use hds_ui::ReqHeaders;

/// Общий замок на env-переменные (HDS_CONFIG/HDS_ROOT персистентны в процессе):
/// тесты одного бинаря идут параллельно в потоках — гонка env недопустима.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-uiauth-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn headers(auth: Option<&str>) -> ReqHeaders {
    ReqHeaders {
        origin: Some("http://127.0.0.1:8765".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: Some("1".into()),
        authorization: auth.map(|a| a.to_string()),
        ..ReqHeaders::default()
    }
}

fn write_cfg(path: &std::path::Path, enabled: bool) {
    let body = if enabled {
        "access:\n  enabled: true\n".to_string()
    } else {
        String::new()
    };
    std::fs::write(path, body).unwrap();
}

fn add_user(acc_path: &std::path::Path, login: &str, admin: bool) {
    let mut acc: Access = load_at(acc_path).unwrap();
    acc.users.push(User {
        name: login.into(),
        login: login.into(),
        password_hash: hash_password("pw").unwrap(),
        admin,
        projects: vec![],
        disabled: false,
    });
    save_at(acc_path, &acc).unwrap();
}

/// Токен напрямую через access API (как после /auth/login).
fn issue_token(acc_path: &std::path::Path, login: &str) -> String {
    let mut acc: Access = load_at(acc_path).unwrap();
    let (plain, tok) = hds_core::access::new_token(
        login,
        90,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
    )
    .unwrap();
    acc.tokens.push(tok);
    save_at(acc_path, &acc).unwrap();
    plain
}

#[test]
fn ui_auth_gate_and_access_api() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("gate");
    let cfg_desktop = dir.join("config-desktop.yaml");
    let cfg_server = dir.join("config-server.yaml");
    let acc_path = dir.join("access.yaml");
    write_cfg(&cfg_desktop, false);
    write_cfg(&cfg_server, true);

    // --- Десктоп: без токена страница открывается ---
    std::env::set_var("HDS_CONFIG", &cfg_desktop);
    let h = headers(None);
    let (status, ctype, _) = route_with_access("GET", "/", "", &h, "", Some(&acc_path));
    assert_eq!(status, 200, "десктоп: страница без токена");
    assert!(ctype.starts_with("text/html"));

    // --- Server: без токена — 401, страница и API ---
    std::env::set_var("HDS_CONFIG", &cfg_server);
    add_user(&acc_path, "boss", true);
    let (status, _, _) = route_with_access("GET", "/", "", &headers(None), "", Some(&acc_path));
    assert_eq!(status, 401, "server: страница без токена — 401");
    let (status, _, _) = route_with_access(
        "GET",
        "/api/status",
        "",
        &headers(None),
        "",
        Some(&acc_path),
    );
    assert_eq!(status, 401, "server: API без токена — 401");
    // Токен обычного пользователя — тоже 401 (UI только для админа).
    add_user(&acc_path, "alice", false);
    let alice_plain = issue_token(&acc_path, "alice");
    let (status, _, _) = route_with_access(
        "GET",
        "/",
        "",
        &headers(Some(&format!("Bearer {alice_plain}"))),
        "",
        Some(&acc_path),
    );
    assert_eq!(status, 401, "server: не-админ — 401");
    // Админ-токен — 200.
    let boss_plain = issue_token(&acc_path, "boss");
    let h_admin = headers(Some(&format!("Bearer {boss_plain}")));
    let (status, _, _) = route_with_access("GET", "/", "", &h_admin, "", Some(&acc_path));
    assert_eq!(status, 200, "server: админ — 200");
    let (status, _, _) = route_with_access("GET", "/api/status", "", &h_admin, "", Some(&acc_path));
    assert_eq!(status, 200);

    // --- API доступа под админ-токеном: создать пользователя ---
    let body = r#"{"login":"carl","password":"pw2","projects":["Sales"]}"#;
    let (status, _, resp) = route_with_access(
        "POST",
        "/api/access/user/add",
        "",
        &h_admin,
        body,
        Some(&acc_path),
    );
    assert_eq!(status, 200, "resp={resp}");
    let acc: Access = load_at(&acc_path).unwrap();
    let carl = acc
        .users
        .iter()
        .find(|u| u.login == "carl")
        .expect("carl создан");
    assert_eq!(carl.projects, vec!["Sales"]);
    assert!(carl.password_hash.starts_with("$argon2id$"));
    // grant — поменять проекты
    let (status, _, resp) = route_with_access(
        "POST",
        "/api/access/user/grant",
        "",
        &h_admin,
        r#"{"login":"carl","projects":["Docs","Ops"]}"#,
        Some(&acc_path),
    );
    assert_eq!(status, 200, "resp={resp}");
    let acc: Access = load_at(&acc_path).unwrap();
    let carl = acc.users.iter().find(|u| u.login == "carl").unwrap();
    assert_eq!(carl.projects, vec!["Docs", "Ops"]);
    // GET /api/access/users — список без хэшей
    let (status, _, resp) = route_with_access(
        "GET",
        "/api/access/users",
        "",
        &h_admin,
        "",
        Some(&acc_path),
    );
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert!(v["users"].as_array().unwrap().len() >= 3);
    assert!(
        serde_json::to_string(&v)
            .unwrap()
            .find("argon2id")
            .is_none(),
        "хэши не отдаются"
    );

    std::env::remove_var("HDS_CONFIG");
}

/// Сырой HTTP GET через реальный сокет; возвращает `(код, всё тело ответа)`.
fn http_get(addr: std::net::SocketAddr, auth: Option<&str>) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(addr).expect("connect");
    let auth_line = match auth {
        Some(a) => format!("Authorization: {a}\r\n"),
        None => String::new(),
    };
    let req = format!("GET / HTTP/1.1\r\nHost: test\r\n{auth_line}Connection: close\r\n\r\n");
    use std::io::{Read, Write};
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let code: u16 = resp
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (code, resp)
}

/// T0 (M2, код-ревью 07.10.2026): `read_request` не парсил заголовок
/// `Authorization` → `ui_auth_gate` в server-режиме отдавал 401 даже с валидным
/// админ-токеном. Чистые функции (`route`) дефект не ловят — они получают
/// `ReqHeaders`, сконструированный руками. Проверка через **реальный сокет**:
/// `run_http_listener` + сырой HTTP-запрос из `TcpStream`.
#[test]
fn ui_auth_socket_parses_authorization() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("socket");
    let cfg_path = dir.join("config.yaml");
    std::fs::write(&cfg_path, "access:\n  enabled: true\n").unwrap();
    // HDS_ROOT — временный корень: access_path() = <root>/data/access.yaml
    // (живой data/ теста не касается).
    std::env::set_var("HDS_ROOT", &dir);
    std::env::set_var("HDS_CONFIG", &cfg_path);

    let acc_path = dir.join("data").join("access.yaml");
    std::fs::create_dir_all(acc_path.parent().unwrap()).unwrap();
    add_user(&acc_path, "boss", true);
    let token = issue_token(&acc_path, "boss");

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let _ = hds_ui::run_http_listener(listener);
    });

    // Без токена — 401 (гейт включён).
    let (code, _) = http_get(addr, None);
    assert_eq!(code, 401, "без токена страница — 401");

    // С валидным админ-токеном — 200: заголовок Authorization должен дойти
    // из потока до ui_auth_gate (до фикса здесь был 401).
    let (code, resp) = http_get(addr, Some(&format!("Bearer {token}")));
    assert_eq!(
        code, 200,
        "токен из реального сокета должен проходить: {resp:.200}"
    );

    // Заголовок в другом регистре — тоже должен парситься (HTTP-ключи case-insensitive).
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    use std::io::{Read, Write};
    let req = format!(
        "GET / HTTP/1.1\r\nHost: test\r\nAUTHORIZATION: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "верхний регистр ключа: {resp:.200}"
    );

    std::env::remove_var("HDS_ROOT");
    std::env::remove_var("HDS_CONFIG");
}

/// T0 F2/F3 (код-ревью M3): UI-аудит обязан писать `admin_op` с ФАКТИЧЕСКИМ
/// статусом ответа (после маршрута, а не всегда 200) и `unauthorized` с
/// реальными `method`/`path` (а не `-`).
#[test]
fn ui_audit_admin_op_status_and_unauthorized_fields() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("audit");
    let audit_path = dir.join("audit.jsonl");
    let audit_yaml = audit_path.to_string_lossy().replace('\\', "/");
    let cfg_path = dir.join("config.yaml");
    std::fs::write(
        &cfg_path,
        format!("access:\n  enabled: true\naudit:\n  enabled: true\n  path: \"{audit_yaml}\"\n"),
    )
    .unwrap();
    std::env::set_var("HDS_CONFIG", &cfg_path);

    let acc_path = dir.join("access.yaml");
    add_user(&acc_path, "boss", true);
    let token = issue_token(&acc_path, "boss");
    let h = headers(Some(&format!("Bearer {token}")));

    // POST на несуществующий /api/... под админ-токеном → 404.
    // Аудит admin_op должен записать 404 (раньше всегда 200 и до маршрута).
    let (status, _, _) =
        route_with_access("POST", "/api/does-not-exist", "", &h, "{}", Some(&acc_path));
    assert_eq!(status, 404);

    // GET без токена → 401 + unauthorized с реальными method/path.
    let (status, _, _) = route_with_access(
        "GET",
        "/api/status",
        "",
        &headers(None),
        "",
        Some(&acc_path),
    );
    assert_eq!(status, 401);

    let text = std::fs::read_to_string(&audit_path).unwrap();
    let evs: Vec<serde_json::Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let admin = evs
        .iter()
        .find(|v| v["event"] == "admin_op")
        .expect("admin_op");
    assert_eq!(admin["status"], 404, "фактический статус, не 200: {admin}");
    assert_eq!(admin["method"], "POST");
    assert_eq!(admin["path"], "/api/does-not-exist");
    assert_eq!(admin["user"], "boss");

    let un = evs
        .iter()
        .find(|v| v["event"] == "unauthorized")
        .expect("unauthorized");
    assert_eq!(un["method"], "GET", "реальный метод, не '-': {un}");
    assert_eq!(un["path"], "/api/status", "реальный путь, не '-': {un}");
    assert_eq!(un["status"], 401);

    std::env::remove_var("HDS_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}
