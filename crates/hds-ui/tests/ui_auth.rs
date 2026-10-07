//! Тесты гейта UI (M1 T5): десктоп — без токена; server-режим (access.enabled)
//! — только с админ-токеном; API доступа работает под админ-токеном.
//!
//! HDS_CONFIG — единственная переменная окружения, поэтому все шаги в ОДНОМ
//! тесте (последовательно), чтобы не было гонки env между тестами.

use hds_core::access::{hash_password, load_at, save_at, Access, User};
use hds_ui::route_with_access;
use hds_ui::ReqHeaders;

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
