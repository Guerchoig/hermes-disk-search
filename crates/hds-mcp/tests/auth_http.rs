//! Тесты авторизации HTTP-MCP (M1 T4, приёмка §5): /health публичный,
//! без/с неверным токеном — 401, /auth/login выдаёт токен, десктоп не ломаем.

use hds_core::access::{hash_password, load_at, save_at, Access, User};
use hds_mcp::auth::AuthEnv;
use hds_mcp::http::route;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-auth-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Включённая авторизация с хранилищем во временном каталоге.
fn env(tag: &str) -> AuthEnv {
    AuthEnv {
        enabled: true,
        access_path: tmpdir(tag).join("access.yaml"),
        ttl_days: 90,
        audit: None,
    }
}

fn add_user(env: &AuthEnv, login: &str, name: &str, admin: bool, projects: &[&str]) {
    let mut acc: Access = load_at(&env.access_path).unwrap();
    acc.users.push(User {
        name: name.into(),
        login: login.into(),
        password_hash: hash_password("pw").unwrap(),
        admin,
        projects: projects.iter().map(|s| s.to_string()).collect(),
        disabled: false,
    });
    save_at(&env.access_path, &acc).unwrap();
}

#[test]
fn health_is_public_without_token() {
    let e = env("health");
    let (status, _, _) = route("GET", "/health", "", "/mcp", &e, None, "127.0.0.1");
    assert_eq!(status, 200);
}

#[test]
fn mcp_without_token_is_401() {
    let e = env("no-token");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let (status, _, _) = route("POST", "/mcp", body, "/mcp", &e, None, "127.0.0.1");
    assert_eq!(status, 401);
    // Мусорный токен — тоже 401.
    let (status, _, _) = route(
        "POST",
        "/mcp",
        body,
        "/mcp",
        &e,
        Some("Bearer garbage"),
        "127.0.0.1",
    );
    assert_eq!(status, 401);
}

#[test]
fn login_issues_token_and_token_works() {
    let e = env("login");
    add_user(&e, "alice", "Alice", false, &["Sales"]);
    // Неверный пароль → 401.
    let (status, _, _) = route(
        "POST",
        "/auth/login",
        r#"{"login":"alice","password":"wrong"}"#,
        "/mcp",
        &e,
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 401);
    // Верный → 200 {token, expires, user, projects}.
    let (status, _, body) = route(
        "POST",
        "/auth/login",
        r#"{"login":"alice","password":"pw"}"#,
        "/mcp",
        &e,
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["user"], "alice");
    assert_eq!(v["projects"][0], "Sales");
    assert!(v["token"].as_str().unwrap().len() == 64, "opaque 256 бит");
    assert!(v["expires"].as_i64().unwrap() > 0);
    // Токен проходит Bearer-проверку (ping).
    let hdr = format!("Bearer {}", v["token"].as_str().unwrap());
    let (status, _, body) = route(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "/mcp",
        &e,
        Some(&hdr),
        "127.0.0.1",
    );
    assert_eq!(status, 200);
    let r: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(r["result"].is_object(), "{body}");
}

#[test]
fn disabled_env_keeps_desktop_behavior() {
    let e = AuthEnv::disabled();
    let (status, _, _) = route(
        "POST",
        "/auth/login",
        r#"{"login":"a","password":"b"}"#,
        "/mcp",
        &e,
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 404);
    // /mcp без токена работает (десктоп не ломаем).
    let (status, _, _) = route(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "/mcp",
        &e,
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 200);
}
