//! Тесты streamable-http маршрутов (чистая `route()`): /health, /mcp, 404.
//! Десктоп-поведение (access.enabled=false) — как раньше, без токена.

use hds_mcp::auth::AuthEnv;
use hds_mcp::http::route;

fn env() -> AuthEnv {
    AuthEnv::disabled()
}

#[test]
fn health_reports_app() {
    let (status, ctype, body) = route("GET", "/health", "", "/mcp", &env(), None, "10.1.2.3");
    assert_eq!(status, 200);
    assert_eq!(ctype, "application/json");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["app"], "disk-search");
    assert_eq!(v["transport"], "streamable-http");
    // M3 T1: клиентский IP доходит до route (диагностика прокси).
    assert_eq!(v["client_ip"], "10.1.2.3");
}

#[test]
fn mcp_post_returns_jsonrpc_result() {
    let (status, _, body) = route(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        "/mcp",
        &env(),
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["result"]["serverInfo"]["name"], "disk-search");
}

#[test]
fn mcp_notification_is_accepted_without_body() {
    let (status, _, body) = route(
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "/mcp",
        &env(),
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 202);
    assert!(body.is_empty());
}

#[test]
fn unknown_path_is_404() {
    let (status, _, _) = route("GET", "/nope", "", "/mcp", &env(), None, "127.0.0.1");
    assert_eq!(status, 404);
}

#[test]
fn bad_json_is_parse_error() {
    let (status, _, body) = route(
        "POST",
        "/mcp",
        "{not json",
        "/mcp",
        &env(),
        None,
        "127.0.0.1",
    );
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["error"]["code"], -32700);
}

/// F6 (T0): «тяжёлый» запрос определяется разбором JSON-RPC (`tools/call` +
/// `params.name == ask_my_files`), а не подстрокой в теле.
#[test]
fn heavy_request_is_jsonrpc_not_substring() {
    // Настоящий вызов RAG — тяжёлый.
    assert!(hds_mcp::is_heavy_request(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ask_my_files","arguments":{"query":"x"}}}"#
    ));
    // Слово `ask_my_files` в аргументах — НЕ тяжёлый (иначе ложный лимит).
    assert!(!hds_mcp::is_heavy_request(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_local_files","arguments":{"query":"ask_my_files"}}}"#
    ));
    // Другой метод — не тяжёлый.
    assert!(!hds_mcp::is_heavy_request(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#
    ));
    assert!(!hds_mcp::is_heavy_request("{not json"));
}
