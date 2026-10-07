//! Admin-гейтинг и фильтр проектов (M1 T4): индексацией занимается только
//! админ; project_filter — единая точка (admin → без фильтра, пусто → ничего).

use hds_core::access::{authenticate, hash_password, load_at, save_at, Access, AuthCtx, User};
use hds_mcp::auth::{admin_required, project_filter, AuthEnv};
use hds_mcp::http::route;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-gate-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn env(tag: &str) -> AuthEnv {
    AuthEnv {
        enabled: true,
        access_path: tmpdir(tag).join("access.yaml"),
        ttl_days: 90,
        audit: None,
    }
}

fn add_user(env: &AuthEnv, login: &str, admin: bool, projects: &[&str]) {
    let mut acc: Access = load_at(&env.access_path).unwrap();
    acc.users.push(User {
        name: login.into(),
        login: login.into(),
        password_hash: hash_password("pw").unwrap(),
        admin,
        projects: projects.iter().map(|s| s.to_string()).collect(),
        disabled: false,
    });
    save_at(&env.access_path, &acc).unwrap();
}

/// plain-токен через POST /auth/login.
fn token_for(e: &AuthEnv, login: &str) -> String {
    let (status, _, body) = route(
        "POST",
        "/auth/login",
        &format!(r#"{{"login":"{login}","password":"pw"}}"#),
        "/mcp",
        e,
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    v["token"].as_str().unwrap().to_string()
}

/// tools/call с Bearer-токеном пользователя.
fn call(e: &AuthEnv, login: &str, name: &str, args: &str) -> serde_json::Value {
    let hdr = format!("Bearer {}", token_for(e, login));
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#
    );
    let (status, _, body) = route("POST", "/mcp", &body, "/mcp", e, Some(&hdr), "127.0.0.1");
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap_or(serde_json::Value::Null)
}

#[test]
fn indexing_tools_require_admin() {
    let e = env("gate");
    add_user(&e, "alice", false, &["Sales"]);
    // stop_indexing → isError с текстом про админа (БД не нужна).
    let v = call(&e, "alice", "stop_indexing", "{}");
    assert_eq!(v["result"]["isError"], true, "{v}");
    let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("администратора"), "text={text}");
    // Гейтинг ДО валидации аргументов.
    let v = call(&e, "alice", "reindex_path", "{}");
    let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("администратора"), "text={text}");
    // Поиск — НЕ админ-инструмент: для пользователя ошибок админа нет
    // (БД может отсутствовать — ошибка будет про базу).
    let v = call(
        &e,
        "alice",
        "search_local_files",
        r#"{"query":"маркер","limit":1}"#,
    );
    let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(!text.contains("администратора"), "text={text}");
}

#[test]
fn admin_required_helper() {
    let e = env("helper");
    add_user(&e, "alice", false, &[]);
    add_user(&e, "boss", true, &[]);
    let acc = load_at(&e.access_path).unwrap();
    let alice = authenticate(&acc, "alice", "pw");
    let boss = authenticate(&acc, "boss", "pw");
    assert!(admin_required(alice.as_ref()).is_err());
    assert!(admin_required(boss.as_ref()).is_ok());
    assert!(admin_required(None).is_ok(), "desktop/stdio — как раньше");
}

#[test]
fn project_filter_semantics() {
    // Нет авторизации → без фильтра (desktop).
    assert_eq!(project_filter(None), None);
    // Admin → без фильтра (видит всё).
    let admin = AuthCtx {
        user: "admin".into(),
        admin: true,
        projects: vec![],
    };
    assert_eq!(project_filter(Some(&admin)), None);
    // Обычный пользователь → его проекты.
    let alice = AuthCtx {
        user: "alice".into(),
        admin: false,
        projects: vec!["Sales".into()],
    };
    assert_eq!(
        project_filter(Some(&alice)),
        Some(vec!["Sales".to_string()])
    );
    // Пустой список → Some(пусто): поиск не выдаст ничего.
    let nobody = AuthCtx {
        user: "nobody".into(),
        admin: false,
        projects: vec![],
    };
    assert_eq!(project_filter(Some(&nobody)), Some(vec![]));
}
