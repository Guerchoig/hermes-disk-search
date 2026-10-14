//! Тесты пользовательского UI (R4, `PLAN_TRANSCRIBE_SERVICE.md`): вход по
//! логину/паролю, разделение гейтов (user vs admin), `403` не-админу на
//! админском API, `401` без токена, rate-limit логина, правка `.md` не-админом
//! (`.bak` + path-safety) и состав страницы `/ui/user/`.
//!
//! `HDS_CONFIG` — переменная окружения процесса, поэтому каждый тест берёт общий
//! замок (тесты одного бинаря идут параллельно в потоках).

use hds_core::access::{hash_password, load_at, save_at, Access, User};
use hds_ui::{page, route_full, ReqHeaders};
use serde_json::{json, Value};

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-uiuser-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Заголовки как от браузера за прокси: origin **не** loopback (TLS-домен) —
/// проверяем, что CSRF-послабление server-режима работает.
fn headers(auth: Option<&str>) -> ReqHeaders {
    ReqHeaders {
        origin: Some("https://hds.example.com".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: Some("1".into()),
        authorization: auth.map(|a| a.to_string()),
        client_ip: Some("10.0.0.9".into()),
        ..ReqHeaders::default()
    }
}

/// Мини-конфиг теста: server-профиль + включённая авторизация (+ строки теста).
fn write_cfg(path: &std::path::Path, extra: &str) {
    std::fs::write(
        path,
        format!("profile: server\naccess:\n  enabled: true\n{extra}"),
    )
    .unwrap();
}

/// Учётка с паролем `pw` и проектом `P1`.
fn add_user(acc: &std::path::Path, login: &str, admin: bool) {
    let mut a: Access = load_at(acc).unwrap();
    a.users.push(User {
        name: login.into(),
        login: login.into(),
        password_hash: hash_password("pw").unwrap(),
        admin,
        projects: vec!["P1".into()],
        disabled: false,
    });
    save_at(acc, &a).unwrap();
}

/// Вход через маршрут (как `hds login`): `POST /api/auth/login` → `(код, JSON)`.
fn login(acc: &std::path::Path, who: &str) -> (u16, Value) {
    let body = json!({ "login": who, "password": "pw" }).to_string();
    let (status, _, text) = route_full(
        "POST",
        "/api/auth/login",
        "",
        &headers(None),
        &body,
        Some(acc),
        None,
    );
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// `Authorization: Bearer <token>`.
fn bearer(token: &str) -> ReqHeaders {
    headers(Some(&format!("Bearer {token}")))
}

/// R4.1/R4.7: вход выдаёт токен; user-гейт пускает страницу и «безопасные» API,
/// админ-гейт отвечает не-админу `403`, а без токена — `401`.
#[test]
fn login_grants_user_routes_and_denies_admin_ones() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("gates");
    let cfg = dir.join("config.yaml");
    let acc = dir.join("access.yaml");
    write_cfg(&cfg, "");
    std::env::set_var("HDS_CONFIG", &cfg);
    add_user(&acc, "boss", true);
    add_user(&acc, "anna", false);

    // Вход не-админа — 200, токен (plain) и его проекты.
    let (status, v) = login(&acc, "anna");
    assert_eq!(status, 200, "вход: {v}");
    assert_eq!(v["user"], "anna");
    assert_eq!(v["admin"], false);
    assert_eq!(v["projects"][0], "P1");
    let anna = v["token"].as_str().expect("токен выдан").to_string();
    assert!(!anna.is_empty());

    // user-гейт: страница (в т.ч. с префиксом /ui от прокси) и безопасные API.
    let anna_h = bearer(&anna);
    for path in [
        "/ui/user/",
        "/api/user/me",
        "/api/user/client",
        "/api/transcribe/list",
    ] {
        let (status, ctype, body) = route_full("GET", path, "", &anna_h, "", Some(&acc), None);
        assert_eq!(status, 200, "{path}: {body}");
        if path == "/ui/user/" {
            assert!(ctype.starts_with("text/html"), "{path}: {ctype}");
        }
    }
    let (_, _, me) = route_full("GET", "/api/user/me", "", &anna_h, "", Some(&acc), None);
    let me: Value = serde_json::from_str(&me).unwrap();
    assert_eq!(me["auth"], true);
    assert_eq!(me["user"], "anna");

    // Админские маршруты — 403 (валидный токен без прав), а не 401.
    for path in ["/", "/api/config", "/api/transcribe/daemon"] {
        let (status, _, body) = route_full("GET", path, "", &anna_h, "", Some(&acc), None);
        assert_eq!(status, 403, "{path}: {body}");
    }
    let body = json!({ "paths": [] }).to_string();
    let (status, _, _) = route_full(
        "POST",
        "/api/config/excludes",
        "",
        &anna_h,
        &body,
        Some(&acc),
        None,
    );
    assert_eq!(status, 403, "правка конфига не-админом");

    // Админ — те же маршруты 200.
    let (status, v) = login(&acc, "boss");
    assert_eq!(status, 200);
    let boss_h = bearer(v["token"].as_str().unwrap());
    for path in ["/", "/api/config", "/api/transcribe/daemon"] {
        let (status, _, body) = route_full("GET", path, "", &boss_h, "", Some(&acc), None);
        assert_eq!(status, 200, "{path}: {body}");
    }

    // Без токена — 401 (и страницы, и API); невалидный токен — тоже 401.
    for path in ["/", "/ui/user/", "/api/user/me"] {
        let (status, _, _) = route_full("GET", path, "", &headers(None), "", Some(&acc), None);
        assert_eq!(status, 401, "{path}");
    }
    let (status, _, _) = route_full(
        "GET",
        "/api/user/me",
        "",
        &bearer("deadbeef"),
        "",
        Some(&acc),
        None,
    );
    assert_eq!(status, 401, "невалидный токен");

    std::env::remove_var("HDS_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}

/// R4.1: ошибки входа — те же, что у MCP/CLI: `401` неверные данные, `400`
/// битый JSON; при `access.enabled: false` вход не нужен (десктоповый доступ).
#[test]
fn login_errors_and_desktop_mode() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("login-errors");
    let cfg = dir.join("config.yaml");
    let acc = dir.join("access.yaml");
    write_cfg(&cfg, "");
    std::env::set_var("HDS_CONFIG", &cfg);
    add_user(&acc, "anna", false);

    let body = json!({ "login": "anna", "password": "no" }).to_string();
    let (status, _, text) = route_full(
        "POST",
        "/api/auth/login",
        "",
        &headers(None),
        &body,
        Some(&acc),
        None,
    );
    assert_eq!(status, 401, "{text}");

    let (status, _, text) = route_full(
        "POST",
        "/api/auth/login",
        "",
        &headers(None),
        "{не json",
        Some(&acc),
        None,
    );
    assert_eq!(status, 400, "{text}");

    // Десктоп: авторизация выключена — вход отвечает 400 с подсказкой, страница
    // и user-API открыты без токена (прежнее поведение).
    std::fs::write(&cfg, "profile: desktop\naccess:\n  enabled: false\n").unwrap();
    let body = json!({ "login": "anna", "password": "pw" }).to_string();
    let (status, _, text) = route_full(
        "POST",
        "/api/auth/login",
        "",
        &headers(None),
        &body,
        Some(&acc),
        None,
    );
    assert_eq!(status, 400, "{text}");
    let (status, _, _) = route_full("GET", "/ui/user/", "", &headers(None), "", Some(&acc), None);
    assert_eq!(status, 200, "desktop: страница открыта без токена");
    let (_, _, me) = route_full(
        "GET",
        "/api/user/me",
        "",
        &headers(None),
        "",
        Some(&acc),
        None,
    );
    let me: Value = serde_json::from_str(&me).unwrap();
    assert_eq!(me["auth"], false, "desktop: локальный доступ");

    std::env::remove_var("HDS_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}

/// R4.1: антибрутфорс логина — `login_per_min` по IP (ёмкость bucket 5):
/// шестая попытка подряд получает `429`, а не `401`/`200`.
#[test]
fn login_rate_limit_returns_429() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("login-limit");
    let cfg = dir.join("config.yaml");
    let acc = dir.join("access.yaml");
    write_cfg(&cfg, "rate_limit:\n  enabled: true\n  login_per_min: 1\n");
    std::env::set_var("HDS_CONFIG", &cfg);
    add_user(&acc, "anna", false);
    let cfg_v = hds_core::config::load_from(&cfg).unwrap();
    let limiter = hds_core::ratelimit::Limiter::from_config(&cfg_v).expect("лимитер включён");

    let body = json!({ "login": "anna", "password": "pw" }).to_string();
    let mut codes = Vec::new();
    for _ in 0..6 {
        let (status, _, _) = route_full(
            "POST",
            "/api/auth/login",
            "",
            &headers(None),
            &body,
            Some(&acc),
            Some(&limiter),
        );
        codes.push(status);
    }
    assert_eq!(
        &codes[..5],
        &[200, 200, 200, 200, 200],
        "попытки: {codes:?}"
    );
    assert_eq!(codes[5], 429, "антибрутфорс: {codes:?}");

    std::env::remove_var("HDS_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}

/// R4.5 (решение §8 №5): авторизованный не-админ **правит `.md`** — замена имён
/// в заголовках реплик, `.bak` в каталоге сервисных файлов, sidecar обновлён;
/// имена с путём (`..`, абсолютные, `:`) отклоняются (path-safety).
#[test]
fn non_admin_edits_markdown_with_backup_and_path_safety() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("md");
    let out = dir.join("out");
    let svc = dir.join("svc");
    std::fs::create_dir_all(&out).unwrap();
    let cfg = dir.join("config.yaml");
    let acc = dir.join("access.yaml");
    let q = |p: &std::path::Path| p.display().to_string().replace('\\', "/");
    write_cfg(
        &cfg,
        &format!(
            "auto_transcribe:\n  enabled: true\n  out_dir: '{}'\n  state_dir: '{}'\n",
            q(&out),
            q(&svc)
        ),
    );
    std::env::set_var("HDS_CONFIG", &cfg);
    add_user(&acc, "anna", false);
    let md = out.join("клип.md");
    std::fs::write(&md, "### SPEAKER_00 [00:00:01 - 00:00:02]\nПривет\n").unwrap();
    let outside = dir.join("evil.md");
    std::fs::write(&outside, "### SPEAKER_00 [00:00:01 - 00:00:02]\nчужое\n").unwrap();

    let (status, v) = login(&acc, "anna");
    assert_eq!(status, 200);
    let h = bearer(v["token"].as_str().unwrap());

    // Подстановка имён не-админом: файл меняется, `.bak` и sidecar — в svc.
    let body = json!({ "name": "клип.md", "names": { "SPEAKER_00": "Иван" } }).to_string();
    let (status, _, text) = route_full(
        "POST",
        "/api/transcribe/speakers",
        "",
        &h,
        &body,
        Some(&acc),
        None,
    );
    assert_eq!(status, 200, "{text}");
    let r: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(r["ok"], true, "{r}");
    let after = std::fs::read_to_string(&md).unwrap();
    assert!(
        after.contains("### Иван [00:00:01"),
        "имя подставлено: {after}"
    );
    assert!(
        svc.join("клип.md.bak").is_file(),
        "копия .bak в service_dir"
    );
    assert!(
        svc.join("клип.speakers.json").is_file(),
        "sidecar имён обновлён"
    );

    // Path-safety: имя с путём — отказ (`ok: false`), чужой файл не тронут.
    for bad in ["../evil.md", "..\\evil.md", "/etc/passwd", "C:\\evil.md"] {
        let body = json!({ "name": bad, "names": { "SPEAKER_00": "Хакер" } }).to_string();
        let (status, _, text) = route_full(
            "POST",
            "/api/transcribe/speakers",
            "",
            &h,
            &body,
            Some(&acc),
            None,
        );
        assert_eq!(status, 200, "{bad}: маршрут отвечает телом: {text}");
        let r: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(r["ok"], false, "{bad}: {r}");
    }
    assert!(
        std::fs::read_to_string(&outside).unwrap().contains("чужое"),
        "файл вне out_dir не изменён"
    );

    std::env::remove_var("HDS_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}

/// R4.3/R2.5: страница пользователя содержит три блока (поиск, установка
/// клиента, транскрипты/спикеры) и форму входа; админская страница — индикаторы
/// «исключена из индексации» у полей папок.
#[test]
fn user_page_has_three_blocks() {
    for id in ["user-search", "user-install", "user-speakers"] {
        assert!(
            page::PAGE_USER.contains(id),
            "нет блока {id} на странице пользователя"
        );
    }
    assert!(page::PAGE_USER.contains("login-user"), "нет формы входа");
    assert!(
        page::PAGE_USER.contains("/api/auth/login"),
        "нет входа по паролю"
    );
    assert!(
        page::PAGE_USER.contains("/api/user/client") && page::PAGE_USER.contains("install_ps1"),
        "нет блока установки клиента"
    );
    assert!(
        page::PAGE_USER.contains("/api/transcribe/speakers"),
        "нет правки имён спикеров"
    );
    for admin in ["/api/config", "/api/services", "/api/index/"] {
        assert!(
            !page::PAGE_USER.contains(admin),
            "админский маршрут {admin} на странице пользователя"
        );
    }
    assert!(
        page::PAGE.contains("trinboxind") && page::PAGE.contains("troutind"),
        "нет индикаторов «исключена из индексации» на админской странице"
    );
}
