//! Тесты MCP-протокола (stdio JSON-RPC 2.0) без БД/сети: initialize, tools/list,
//! tools/call (валидация аргументов), ошибки, ping, notifications.

use hds_mcp::handle_line;
use serde_json::Value;

fn v(line: &str) -> Value {
    serde_json::from_str(&handle_line(line).expect("ответ")).unwrap()
}

#[test]
fn initialize_reports_server_info() {
    let r = v(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
    assert_eq!(r["result"]["serverInfo"]["name"], "disk-search");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert_eq!(r["id"], 1);
}

#[test]
fn tools_list_has_six_named_tools() {
    let r = v(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let names: Vec<String> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "search_local_files",
            "ask_my_files",
            "index_status",
            "start_indexing",
            "stop_indexing",
            "reindex_path"
        ]
    );
    let search = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "search_local_files")
        .unwrap();
    assert_eq!(search["inputSchema"]["required"][0], "query");
}

#[test]
fn tools_call_missing_arg_is_error_content() {
    let r = v(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_local_files","arguments":{}}}"#,
    );
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("query"));
}

#[test]
fn tools_call_unknown_tool_is_error_content() {
    let r = v(
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
    );
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("unknown tool"));
}

#[test]
fn unknown_method_is_jsonrpc_error() {
    let r = v(r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#);
    assert_eq!(r["error"]["code"], -32601);
}

#[test]
fn ping_and_notification() {
    let r = v(r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#);
    assert_eq!(r["result"], serde_json::json!({}));
    assert!(handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
}
