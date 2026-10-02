//! Тесты кадрирования/разбора JSON-RPC 2.0 (контракт §5) — без процесса воркера.

use hds_extract::protocol::{
    capabilities_from, extract_from, lemmas_from, parse_response, request, PROTOCOL_VERSION,
};
use serde_json::json;

#[test]
fn request_is_single_line_jsonrpc() {
    let s = request(7, "hello", json!({"protocol": PROTOCOL_VERSION}));
    assert!(!s.contains('\n'), "NDJSON: строка без переводов строк");
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], 7);
    assert_eq!(v["method"], "hello");
    assert_eq!(v["params"]["protocol"], 1);
}

#[test]
fn parse_response_ok_and_id_mismatch() {
    let line = r#"{"jsonrpc":"2.0","id":3,"result":{"a":1}}"#;
    let v = parse_response(line, 3).unwrap();
    assert_eq!(v["a"], 1);
    let err = parse_response(line, 4).unwrap_err();
    assert!(
        err.message().contains("неожиданный id"),
        "{}",
        err.message()
    );
}

#[test]
fn parse_response_error_carries_message_and_hint() {
    let line = r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32001,"message":"pdf повреждён: cannot open","hint":"файл пропущен"}}"#;
    let err = parse_response(line, 3).unwrap_err();
    let m = err.message();
    assert!(m.contains("pdf повреждён"), "{m}");
    assert!(m.contains("файл пропущен"), "{m}");
    assert!(m.contains("-32001"), "{m}");
}

#[test]
fn capabilities_parsing() {
    let caps = capabilities_from(&json!({
        "protocol": 1, "python": "3.12.14", "pid": 42,
        "capabilities": ["text", "pdf", "normalize"]
    }));
    assert_eq!(caps.protocol, 1);
    assert_eq!(caps.python, "3.12.14");
    assert_eq!(caps.pid, 42);
    assert!(caps.has("normalize"));
    assert!(!caps.has("ocr"));
}

#[test]
fn extract_and_lemmas_parsing() {
    let r = extract_from(&json!({
        "kind": "pdf",
        "segments": [
            {"text": "Итоги", "page": 3},
            {"text": "таблица", "head": "# Раздел / ## Таблица"}
        ],
        "warnings": ["страница 7: пустой текст"],
        "elapsed_ms": 12.5
    }));
    assert_eq!(r.kind, "pdf");
    assert_eq!(r.segments.len(), 2);
    assert_eq!(r.segments[0].page, Some(3));
    assert_eq!(r.segments[0].t_start, None);
    assert_eq!(r.segments[1].head.as_deref(), Some("# Раздел / ## Таблица"));
    assert_eq!(r.warnings, vec!["страница 7: пустой текст"]);
    assert_eq!(r.elapsed_ms, 12.5);

    assert_eq!(
        lemmas_from(&json!({"lemmas": ["настройка", "скрипт"]})),
        vec!["настройка", "скрипт"]
    );
    assert!(lemmas_from(&json!({})).is_empty());
}
