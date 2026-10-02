//! Чистые тесты веб-интерфейса: страница, 404, валидация, query-параметры
//! (без БД/сети/файловых операций).

use hds_ui::{qp, route, ReqHeaders};

fn h() -> ReqHeaders {
    ReqHeaders {
        origin: None,
        content_type: Some("application/json".into()),
        x_hds_ui: Some("1".into()),
    }
}

#[test]
fn page_is_served() {
    let (status, ctype, body) = route("GET", "/", "", &ReqHeaders::default(), "");
    assert_eq!(status, 200);
    assert!(ctype.starts_with("text/html"));
    assert!(body.contains("Hermes Disk Search"));
    assert!(body.contains("/api/status"));
}

#[test]
fn unknown_path_is_404() {
    assert_eq!(route("GET", "/nope", "", &ReqHeaders::default(), "").0, 404);
    assert_eq!(route("POST", "/api/unknown", "", &h(), "").0, 404);
}

#[test]
fn search_and_ask_require_q() {
    assert_eq!(
        route("GET", "/api/search", "", &ReqHeaders::default(), "").0,
        400
    );
    assert_eq!(
        route("GET", "/api/ask", "", &ReqHeaders::default(), "").0,
        400
    );
}

#[test]
fn csrf_blocks_cross_origin_post() {
    let bad = ReqHeaders {
        origin: Some("http://evil.example".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: None,
    };
    assert_eq!(route("POST", "/api/index/stop", "", &bad, "").0, 403);
    assert_eq!(
        route("POST", "/api/index/stop", "", &ReqHeaders::default(), "").0,
        403
    );
}

#[test]
fn query_params_decode() {
    assert_eq!(qp("q=hello%20world&limit=5", "q").unwrap(), "hello world");
    assert_eq!(qp("q=a+b", "q").unwrap(), "a b");
    assert_eq!(qp("q=%D1%82%D0%B5%D1%81%D1%82", "q").unwrap(), "тест");
    assert_eq!(qp("limit=5", "limit").unwrap(), "5");
    assert_eq!(qp("q=1", "x"), None);
}
