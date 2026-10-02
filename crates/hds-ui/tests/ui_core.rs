//! Чистые тесты веб-интерфейса: страница, 404, валидация, query-параметры
//! (без БД/сети/файловых операций).

use hds_ui::{qp, route};

#[test]
fn page_is_served() {
    let (status, ctype, body) = route("GET", "/", "");
    assert_eq!(status, 200);
    assert!(ctype.starts_with("text/html"));
    assert!(body.contains("Hermes Disk Search"));
    assert!(body.contains("/api/status"));
}

#[test]
fn unknown_path_is_404() {
    assert_eq!(route("GET", "/nope", "").0, 404);
    assert_eq!(route("POST", "/api/unknown", "").0, 404);
}

#[test]
fn search_and_ask_require_q() {
    assert_eq!(route("GET", "/api/search", "").0, 400);
    assert_eq!(route("GET", "/api/ask", "").0, 400);
}

#[test]
fn query_params_decode() {
    assert_eq!(qp("q=hello%20world&limit=5", "q").unwrap(), "hello world");
    assert_eq!(qp("q=a+b", "q").unwrap(), "a b");
    assert_eq!(qp("q=%D1%82%D0%B5%D1%81%D1%82", "q").unwrap(), "тест");
    assert_eq!(qp("limit=5", "limit").unwrap(), "5");
    assert_eq!(qp("q=1", "x"), None);
}
