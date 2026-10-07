//! Аудит MCP-событий (M3 T2, `PLAN_M3_NETWORK_PERIMETER.md` §3.2):
//! `login_ok`/`login_fail`, `unauthorized`, `tool_call` (кто/что/проекты).
//! Десктоп (audit: None) — файл не создаётся.

use hds_core::access::{hash_password, load_at, save_at, Access, User};
use hds_mcp::auth::AuthEnv;
use hds_mcp::http::route;
use std::path::PathBuf;

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-audit-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn env(tag: &str) -> AuthEnv {
    env_audit(tag, true)
}

/// Включённая авторизация + аудит в явный файл (`audit_path=false` — без аудита).
fn env_audit(tag: &str, audit: bool) -> AuthEnv {
    let dir = tmpdir(tag);
    AuthEnv {
        enabled: true,
        access_path: dir.join("access.yaml"),
        ttl_days: 90,
        audit: audit.then(|| hds_core::audit::Audit::at(dir.join("audit.jsonl"), 50)),
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

fn events(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn login_ok_and_fail_are_audited() {
    let e = env("login");
    add_user(&e, "alice", false, &["Sales"]);
    let path = e.audit.as_ref().unwrap().path.clone();
    // Успешный логин.
    route(
        "POST",
        "/auth/login",
        r#"{"login":"alice","password":"pw"}"#,
        "/mcp",
        &e,
        None,
        "10.1.2.3",
    );
    // Неверный пароль.
    route(
        "POST",
        "/auth/login",
        r#"{"login":"alice","password":"wrong"}"#,
        "/mcp",
        &e,
        None,
        "10.1.2.3",
    );
    let evs = events(&path);
    assert_eq!(evs.len(), 2, "{evs:?}");
    assert_eq!(evs[0]["event"], "login_ok");
    assert_eq!(evs[0]["user"], "alice");
    assert_eq!(evs[0]["ip"], "10.1.2.3");
    assert_eq!(evs[0]["status"], 200);
    assert!(
        evs[0].get("projects").is_none(),
        "токены/тела не логируются"
    );
    assert_eq!(evs[1]["event"], "login_fail");
    assert_eq!(evs[1]["status"], 401);
}

#[test]
fn unauthorized_and_tool_call_are_audited() {
    let e = env("gate");
    add_user(&e, "boss", true, &[]);
    add_user(&e, "alice", false, &["Sales"]);
    let path = e.audit.as_ref().unwrap().path.clone();

    // Без токена → 401 + unauthorized.
    let (status, _, _) = route(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "/mcp",
        &e,
        None,
        "10.1.2.3",
    );
    assert_eq!(status, 401);

    // tool_call от обычного пользователя → событие с его проектами.
    let (status, _, body) = route(
        "POST",
        "/auth/login",
        r#"{"login":"alice","password":"pw"}"#,
        "/mcp",
        &e,
        None,
        "10.1.2.3",
    );
    assert_eq!(status, 200, "{body}");
    let tok: serde_json::Value = serde_json::from_str(&body).unwrap();
    let hdr = format!("Bearer {}", tok["token"].as_str().unwrap());
    route(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_local_files","arguments":{"query":"x"}}}"#,
        "/mcp",
        &e,
        Some(&hdr),
        "10.1.2.3",
    );

    let evs = events(&path);
    let unauthorized = evs
        .iter()
        .find(|v| v["event"] == "unauthorized")
        .expect("unauthorized");
    assert_eq!(unauthorized["status"], 401);
    assert_eq!(unauthorized["ip"], "10.1.2.3");
    let call = evs
        .iter()
        .find(|v| v["event"] == "tool_call")
        .expect("tool_call");
    assert_eq!(call["tool"], "search_local_files");
    assert_eq!(call["user"], "alice");
    assert_eq!(call["projects"][0], "Sales");
    assert_eq!(call["status"], 200);
}

/// Десктоп (audit: None) — событий нет, файл не создаётся.
#[test]
fn no_audit_when_disabled() {
    let e = env_audit("off", false);
    add_user(&e, "alice", false, &[]);
    route(
        "POST",
        "/auth/login",
        r#"{"login":"alice","password":"pw"}"#,
        "/mcp",
        &e,
        None,
        "127.0.0.1",
    );
    route(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
        "/mcp",
        &e,
        None,
        "127.0.0.1",
    );
    // Файл audit.jsonl не появился рядом с access.yaml.
    assert!(
        !e.access_path.parent().unwrap().join("audit.jsonl").exists(),
        "выключенный аудит не пишет файл"
    );
}
