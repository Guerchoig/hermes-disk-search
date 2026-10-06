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

// --- T4: закладка «Транскрибация» (PLAN_AUTO_TRANSCRIBE §8) ---

/// Страница содержит закладки и элементы страницы транскрибации.
#[test]
fn page_has_transcribe_tab() {
    let (_, _, body) = route("GET", "/", "", &ReqHeaders::default(), "");
    for needle in [
        "tab-search",
        "tab-transcribe",
        "showTab(",
        "/api/transcribe/list",
        "/api/transcribe/file",
        "/api/transcribe/speakers",
        "/api/transcribe/apply",
        "/api/transcribe/daemon",
        "Присвоить имена спикерам",
        "Перезапустить задание",
        "Сохранить папки",
        "/api/config/transcribe-dirs",
        "Запустить",
        "Остановить",
        "trdaemon",
        "trmodal",
    ] {
        assert!(body.contains(needle), "в странице нет {needle}");
    }
}

/// Статус демона доступен UI (`running`/`enabled`/папки) — без запуска процесса.
#[test]
fn transcribe_daemon_status_shape() {
    let (status, _, body) = route(
        "GET",
        "/api/transcribe/daemon",
        "",
        &ReqHeaders::default(),
        "",
    );
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    for key in [
        "running",
        "stale",
        "enabled",
        "inbox_dir",
        "out_dir",
        "lock",
    ] {
        assert!(v.get(key).is_some(), "нет поля {key}: {v}");
    }
    assert!(v["running"].is_boolean(), "{v}");
}

/// Управление демоном — POST под CSRF (в тестах только отказ: запускать процесс нельзя).
#[test]
fn transcribe_daemon_post_requires_csrf() {
    let bad = ReqHeaders {
        origin: Some("http://evil.example".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: None,
    };
    assert_eq!(
        route(
            "POST",
            "/api/transcribe/daemon",
            "",
            &bad,
            r#"{"action":"start"}"#
        )
        .0,
        403
    );
    assert_eq!(
        route(
            "POST",
            "/api/transcribe/daemon",
            "",
            &ReqHeaders::default(),
            r#"{"action":"start"}"#
        )
        .0,
        403
    );
}

// --- Демон индексации (`hds watch`): блок на закладке «Поиск» (симметрично) ---

/// На странице есть блок «Демон индексации» со своими кнопками и маршрутом.
#[test]
fn page_has_watch_daemon_block() {
    let (_, _, body) = route("GET", "/", "", &ReqHeaders::default(), "");
    for needle in [
        "Демон индексации",
        "/api/watch/daemon",
        "wdaemon",
        "watchAct(",
        "loadWatchDaemon(",
    ] {
        assert!(body.contains(needle), "в странице нет {needle}");
    }
}

/// Статус демона индексации: `running`/`stale`/корни/сигналы — без запуска процесса.
#[test]
fn watch_daemon_status_shape() {
    let (status, _, body) = route("GET", "/api/watch/daemon", "", &ReqHeaders::default(), "");
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    for key in [
        "running",
        "stale",
        "paused",
        "stop_requested",
        "indexing",
        "roots",
        "lock",
    ] {
        assert!(v.get(key).is_some(), "нет поля {key}: {v}");
    }
    assert!(v["running"].is_boolean() && v["roots"].is_array(), "{v}");
}

/// Управление демоном индексации — POST под CSRF (в тестах только отказ).
#[test]
fn watch_daemon_post_requires_csrf() {
    let bad = ReqHeaders {
        origin: Some("http://evil.example".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: None,
    };
    assert_eq!(
        route(
            "POST",
            "/api/watch/daemon",
            "",
            &bad,
            r#"{"action":"start"}"#
        )
        .0,
        403
    );
    assert_eq!(
        route(
            "POST",
            "/api/watch/daemon",
            "",
            &ReqHeaders::default(),
            r#"{"action":"start"}"#
        )
        .0,
        403
    );
}

/// T5: маршрут «Перезапустить задание» — без `name` 400, с `name` и корректными
/// заголовками отвечает JSON-ошибкой (файла нет), но ничего не пишет.
#[test]
fn transcribe_apply_requires_name() {
    assert_eq!(
        route("POST", "/api/transcribe/apply", "", &h(), "{}").0,
        400,
        "без name"
    );
    let (status, _, body) = route(
        "POST",
        "/api/transcribe/apply",
        "",
        &h(),
        r#"{"name":"нет-такого.md"}"#,
    );
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], false, "{body}");
}

/// Страница: карточки «Модель чата» и «MCP-сервер»; групп запросов (поиск/ask)
/// больше нет — UI только для администрирования (запросы: агенты/MCP/CLI).
#[test]
fn page_admin_sections() {
    let (_, _, body) = route("GET", "/", "", &ReqHeaders::default(), "");
    for needle in [
        "Модель чата",
        "applyChatModel()",
        "/api/chat-model",
        "MCP-сервер disk-search",
        "mcpAct(",
        "/api/mcp-http",
        "/api/llm-host/job",
        "pollJob(",
        "Главная",
    ] {
        assert!(body.contains(needle), "в странице нет {needle}");
    }
    assert!(
        !body.contains("doSearch"),
        "карточки «Поиск» быть не должно"
    );
    assert!(!body.contains("doAsk"), "карточки «Вопрос» быть не должно");
}

/// Статус MCP — структура `{state, url, pid, version}` (без процессов в тесте).
#[test]
fn mcp_status_shape() {
    let (status, _, body) = route("GET", "/api/mcp-http", "", &ReqHeaders::default(), "");
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    for key in ["state", "url", "pid", "version"] {
        assert!(v.get(key).is_some(), "нет поля {key}: {v}");
    }
    assert!(v["state"].as_str().is_some(), "{v}");
    assert!(
        matches!(
            v["state"].as_str(),
            Some("mcp") | Some("foreign") | Some("down")
        ),
        "{v}"
    );
}

/// Неизвестное действие MCP отклоняется без запуска процессов.
#[test]
fn mcp_action_validated() {
    let (status, _, body) = route(
        "POST",
        "/api/mcp-http",
        "",
        &h(),
        r#"{ "action": "bogus" }"#,
    );
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["ok"], false, "{body}");
    assert!(
        v["msg"]
            .as_str()
            .unwrap_or("")
            .contains("start|stop|restart"),
        "{body}"
    );
}

/// Фоновое задание llm-host: эндпоинт хода отдаёт полную структуру
/// (`running/stage/detail/elapsed_sec/result`) — карточка не выглядит зависшей.
#[test]
fn llm_job_status_shape() {
    let (status, _, body) = route("GET", "/api/llm-host/job", "", &ReqHeaders::default(), "");
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    for key in [
        "running",
        "kind",
        "stage",
        "detail",
        "elapsed_sec",
        "result",
    ] {
        assert!(v.get(key).is_some(), "нет поля {key}: {v}");
    }
    assert!(v["running"].is_boolean(), "{v}");
    assert!(v["elapsed_sec"].is_u64(), "{v}");
    assert!(v["stage"].as_str().is_some(), "{v}");
}

/// Не-первый фрагмент шардированной GGUF отклоняется с подсказкой (до любых
/// файловых операций).
#[test]
fn chat_model_set_rejects_non_first_shard() {
    for (bad, hint) in [
        ("m-00002-of-00003.gguf", "m-00001-of-00003.gguf"),
        ("m-00003-of-00003.gguf", "m-00001-of-00003.gguf"),
    ] {
        let req = serde_json::json!({ "file": bad }).to_string();
        let (status, _, body) = route("POST", "/api/chat-model/set", "", &h(), &req);
        assert_eq!(status, 200, "{bad}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false, "{bad}: {body}");
        let msg = v["msg"].as_str().unwrap_or("");
        assert!(msg.contains("ПЕРВЫЙ"), "{bad}: {msg}");
        assert!(msg.contains(hint), "{bad}: {msg}");
    }
    // первый фрагмент проходит валидацию имени (дальше — честная проверка файла)
    let req = serde_json::json!({ "file": "m-00001-of-00003.gguf" }).to_string();
    let (_, _, body) = route("POST", "/api/chat-model/set", "", &h(), &req);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let msg = v["msg"].as_str().unwrap_or("");
    assert!(
        !msg.contains("ПЕРВЫЙ"),
        "первый фрагмент не должен режется: {msg}"
    );
}

/// Смена модели — под CSRF; обход пути/пустое имя/не-.gguf отклоняются до любых
/// файловых операций (перезапуска хоста в тестах нет).
#[test]
fn chat_model_set_rejects_bad_file() {
    for bad in [
        "",
        "  ",
        "..",
        "../x.gguf",
        "a\\b.gguf",
        "a/b.gguf",
        "x.onnx",
    ] {
        let req = serde_json::json!({ "file": bad }).to_string();
        let (status, _, body) = route("POST", "/api/chat-model/set", "", &h(), &req);
        assert_eq!(status, 200, "{bad}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false, "{bad}: {body}");
        assert!(!v["msg"].as_str().unwrap_or("").is_empty(), "{bad}: {body}");
    }
}

/// Список моделей роли chat — структура `{dir, current, models, host_up}`.
#[test]
fn chat_model_shape() {
    let (status, _, body) = route("GET", "/api/chat-model", "", &ReqHeaders::default(), "");
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    if v.get("error").is_some() {
        return; // в окружении без общего рантайма достаточно понятной ошибки
    }
    assert!(v.get("models").and_then(|m| m.as_array()).is_some(), "{v}");
    assert!(v.get("dir").is_some(), "{v}");
    assert!(v["host_up"].is_boolean(), "{v}");
    for m in v["models"].as_array().unwrap() {
        assert!(m["file"].as_str().is_some(), "{m}");
        assert!(m["current"].is_boolean(), "{m}");
    }
}

/// POST-маршруты новых карточек защищены той же CSRF-проверкой.
#[test]
fn new_post_routes_require_csrf() {
    let bad = ReqHeaders {
        origin: Some("http://evil.example".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: None,
    };
    for path in ["/api/chat-model/set", "/api/mcp-http"] {
        assert_eq!(route("POST", path, "", &bad, "{}").0, 403, "{path}");
    }
}

// --- T5/§8.4: запись папок конвейера — под CSRF (в тестах БЕЗ корректных заголовков,
/// чтобы не править боевой `config.yaml`).
#[test]
fn transcribe_dirs_post_requires_csrf() {
    let bad = ReqHeaders {
        origin: Some("http://evil.example".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: None,
    };
    assert_eq!(
        route("POST", "/api/config/transcribe-dirs", "", &bad, "{}").0,
        403
    );
    assert_eq!(
        route(
            "POST",
            "/api/config/transcribe-dirs",
            "",
            &ReqHeaders::default(),
            "{}"
        )
        .0,
        403
    );
}

/// Список файлов отдаёт структуру `{dir, files:[…]}` даже без настроенного `out_dir`.
#[test]
fn transcribe_list_returns_shape() {
    let (status, ctype, body) = route(
        "GET",
        "/api/transcribe/list",
        "",
        &ReqHeaders::default(),
        "",
    );
    assert_eq!(status, 200);
    assert!(ctype.starts_with("application/json"));
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v.get("files").and_then(|f| f.as_array()).is_some(), "{v}");
    assert!(v.get("dir").is_some(), "{v}");
}

/// Без `name` чтение файла — 400 (никакого «первого попавшегося» файла).
#[test]
fn transcribe_file_requires_name() {
    assert_eq!(
        route(
            "GET",
            "/api/transcribe/file",
            "",
            &ReqHeaders::default(),
            ""
        )
        .0,
        400
    );
}

/// Path-safety (§8.3): `..`, разделители и абсолютные пути не проходят даже через маршрут.
#[test]
fn transcribe_file_rejects_traversal() {
    for bad in [
        "..%2Fsecret.md",
        "sub%2Ffile.md",
        "C%3A%5CWindows%5Cx.md",
        "..",
    ] {
        let (status, _, body) = route(
            "GET",
            "/api/transcribe/file",
            &format!("name={bad}"),
            &ReqHeaders::default(),
            "",
        );
        assert_eq!(status, 200, "обработчик отвечает JSON-ошибкой: {bad}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], false, "должно быть отклонено: {bad} -> {body}");
        assert!(
            v["error"].as_str().unwrap_or("").contains("недопустимое")
                || v["error"].as_str().unwrap_or("").contains("не найден"),
            "{bad}: {body}"
        );
    }
}

/// POST-маршруты транскрибации защищены той же CSRF-проверкой, что и остальные (§8.3).
#[test]
fn transcribe_post_requires_csrf() {
    let bad = ReqHeaders {
        origin: Some("http://evil.example".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: None,
    };
    assert_eq!(
        route("POST", "/api/transcribe/speakers", "", &bad, "{}").0,
        403
    );
    assert_eq!(
        route(
            "POST",
            "/api/transcribe/speakers",
            "",
            &ReqHeaders::default(),
            "{}"
        )
        .0,
        403
    );
    // с корректными заголовками, но без поля name — 400
    assert_eq!(
        route("POST", "/api/transcribe/speakers", "", &h(), "{}").0,
        400
    );
}
