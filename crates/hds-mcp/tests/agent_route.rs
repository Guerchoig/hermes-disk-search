//! Сокет-тесты маршрута агента `POST <mcp>/agent/<P>` (TR-4.3, TR-4.4, TR-4.7).
//!
//! Проверяется то, что нельзя проверить чистой функцией: живой HTTP-сервер,
//! проектный токен из `data/agent/secrets.json`, отсутствие `ask_my_files` и
//! управления индексацией в `tools/list`, отказ чужому проекту (I5) и статус
//! индекса по проекту с подсчётом «в очереди» по ФС.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Тесты трогают env (`HDS_CONFIG`, `HDS_ROOT`, `HDS_AGENT_SECRETS`) — идут
/// последовательно (общая среда процесса).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-agent-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(p: &Path, text: &str) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, text).unwrap();
}

/// Стенд: агентская зона с двумя проектами, индекс с двумя файлами и токены.
struct Stand {
    dir: PathBuf,
    token_p1: String,
    token_p2: String,
}

impl Stand {
    fn new(tag: &str) -> Stand {
        let dir = tmpdir(tag);
        let anon = dir.join("anon");
        touch(&anon.join("P0001/workspace/a.md"), "Поручение [PERSON_3].");
        touch(
            &anon.join("P0001/workspace/c.md"),
            "Ещё не проиндексированный образ.",
        );
        touch(
            &anon.join("P0002/workspace/b.md"),
            "Секрет второго проекта.",
        );
        touch(&anon.join("P0001/agent-config/keys.json"), "{}");

        let db = dir.join("index.db");
        {
            let conn = hds_core::db::connect(&db, 4).unwrap();
            let a = hds_core::db::upsert_file(
                &conn,
                &anon.join("P0001/workspace/a.md").to_string_lossy(),
                ".md",
                "text",
                10,
                1.0,
                Some("hash-a"),
                Some("P0001"),
            )
            .unwrap();
            hds_core::db::add_chunk(
                &conn,
                a,
                0,
                Some(1),
                None,
                None,
                "поручение подготовить отчёт xperson3x",
                "поручение подготовить отчёт xperson3x",
            )
            .unwrap();
            hds_core::db::finish_file(&conn, a, "indexed", None, Some(1), 1.0).unwrap();
            let b = hds_core::db::upsert_file(
                &conn,
                &anon.join("P0002/workspace/b.md").to_string_lossy(),
                ".md",
                "text",
                10,
                1.0,
                Some("hash-b"),
                Some("P0002"),
            )
            .unwrap();
            hds_core::db::add_chunk(
                &conn,
                b,
                0,
                None,
                None,
                None,
                "секрет второго проекта xorg2x",
                "секрет второго проекта xorg2x",
            )
            .unwrap();
            hds_core::db::finish_file(&conn, b, "indexed", None, Some(1), 1.0).unwrap();
        }

        let cfg = format!(
            "agent:\n  enabled: true\n  root: '{}'\n\
             db_path: '{}'\n\
             embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n  batch_size: 1\n\
             index:\n  clip: false\n\
             http:\n  max_conns: 0\n",
            anon.to_string_lossy().replace('\\', "/"),
            db.to_string_lossy().replace('\\', "/")
        );
        touch(&dir.join("config.yaml"), &cfg);

        // Токены проектов: в файл — хэш, plain держим в стенде.
        let token_p1 = "aaaa1111".repeat(8);
        let token_p2 = "bbbb2222".repeat(8);
        let mut secrets = hds_mcp::AgentSecrets::default();
        for (p, plain) in [("P0001", &token_p1), ("P0002", &token_p2)] {
            secrets.projects.insert(
                p.to_string(),
                hds_mcp::AgentToken {
                    token_sha256: hds_core::access::token_sha256_hex(plain),
                    expires: 0,
                    token: Some(plain.clone()),
                },
            );
        }
        let secrets_path = dir.join("secrets.json");
        hds_mcp::agent::save_at(&secrets_path, &secrets).unwrap();

        std::env::set_var("HDS_CONFIG", dir.join("config.yaml"));
        std::env::set_var("HDS_ROOT", &dir);
        std::env::set_var("HDS_AGENT_SECRETS", &secrets_path);
        Stand {
            dir,
            token_p1,
            token_p2,
        }
    }

    fn spawn(&self) -> std::net::SocketAddr {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let _ = hds_mcp::run_http_listener(l, "/mcp");
        });
        addr
    }

    fn cleanup(&self) {
        std::env::remove_var("HDS_CONFIG");
        std::env::remove_var("HDS_ROOT");
        std::env::remove_var("HDS_AGENT_SECRETS");
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Сырой POST на путь `path`; `token` — Bearer (None — без заголовка).
fn post(addr: std::net::SocketAddr, path: &str, token: Option<&str>, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\n{auth}\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let status = resp
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = resp
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn rpc(method: &str, params: serde_json::Value) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string()
}

fn tool_call(name: &str, args: serde_json::Value) -> String {
    rpc(
        "tools/call",
        serde_json::json!({ "name": name, "arguments": args }),
    )
}

#[test]
fn agent_route_requires_project_token_and_hides_tools() {
    let _env = ENV_LOCK.lock().unwrap();
    let stand = Stand::new("route");
    let addr = stand.spawn();
    let path = "/mcp/agent/P0001";

    // 1. Без токена, с чужим токеном, с мусором — 401.
    assert_eq!(
        post(addr, path, None, &rpc("ping", serde_json::json!({}))).0,
        401
    );
    assert_eq!(
        post(
            addr,
            path,
            Some(&stand.token_p2),
            &rpc("initialize", serde_json::json!({}))
        )
        .0,
        401,
        "токен чужого проекта обязан давать 401"
    );
    assert_eq!(
        post(
            addr,
            path,
            Some("deadbeef"),
            &rpc("initialize", serde_json::json!({}))
        )
        .0,
        401
    );

    // 2. Свой токен — работает; инструкции маршрута агента.
    let (st, body) = post(
        addr,
        path,
        Some(&stand.token_p1),
        &rpc("initialize", serde_json::json!({})),
    );
    assert_eq!(st, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["result"]["instructions"]
        .as_str()
        .unwrap_or("")
        .contains("анонимной зоне"));

    // 3. tools/list — ровно поиск и статус, без ask_my_files и управления.
    let (st, body) = post(
        addr,
        path,
        Some(&stand.token_p1),
        &rpc("tools/list", serde_json::json!({})),
    );
    assert_eq!(st, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let names: Vec<String> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        names,
        vec!["search_local_files", "index_status"],
        "{names:?}"
    );
    for banned in [
        "ask_my_files",
        "start_indexing",
        "stop_indexing",
        "reindex_path",
    ] {
        assert!(
            !names.iter().any(|n| n == banned),
            "{banned} не должен публиковаться"
        );
    }

    // 4. Вызов скрытого инструмента — ошибка, а не выполнение.
    let (_, body) = post(
        addr,
        path,
        Some(&stand.token_p1),
        &tool_call("ask_my_files", serde_json::json!({"question":"x"})),
    );
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["isError"], true, "{v}");
    assert!(v["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("")
        .contains("unknown tool"));
    let (_, body) = post(
        addr,
        path,
        Some(&stand.token_p1),
        &tool_call("start_indexing", serde_json::json!({})),
    );
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["isError"], true);

    // 5. Поиск не выдаёт чужой проект (I5): пути P0002 и b.md не появляются.
    let (st, body) = post(
        addr,
        path,
        Some(&stand.token_p1),
        &tool_call(
            "search_local_files",
            serde_json::json!({"query":"секрет проекта","limit":10}),
        ),
    );
    assert_eq!(st, 200);
    assert!(!body.contains("P0002"), "утечка чужого проекта: {body}");
    assert!(
        !body.contains("b.md"),
        "утечка файла чужого проекта: {body}"
    );

    stand.cleanup();
}

#[test]
fn agent_index_status_counts_by_project_and_reports_pending() {
    let _env = ENV_LOCK.lock().unwrap();
    let stand = Stand::new("lag");
    let addr = stand.spawn();
    let path = "/mcp/agent/P0001";

    let (st, body) = post(
        addr,
        path,
        Some(&stand.token_p1),
        &tool_call("index_status", serde_json::json!({})),
    );
    assert_eq!(st, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let text = v["result"]["content"][0]["text"].as_str().unwrap_or("");
    let st_json: serde_json::Value = serde_json::from_str(text).expect("index_status — JSON");
    assert_eq!(st_json["project"], "P0001");
    // a.md есть в индексе, c.md — только на диске: оба в total, c.md в pending.
    assert_eq!(st_json["total"], 2, "{st_json}");
    assert!(st_json["pending"].as_u64().unwrap() >= 1, "{st_json}");
    assert!(!text.contains("P0002"), "статус чужого проекта: {text}");
    assert!(!text.contains("b.md"), "{text}");

    stand.cleanup();
}
