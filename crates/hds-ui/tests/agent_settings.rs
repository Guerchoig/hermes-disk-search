//! A5.6: панель «Настройки агента» и «Очистить историю» на живом стеке.
//!
//! Настоящий HTTP-сервер (`run_http_listener`, порт 0), настоящий клиент
//! анонимизации на mock-скрипте `tests/fixtures/mock_anon.py` (NER не нужен) и
//! временный стенд: `config.yaml` **с комментариями**, `projects.json`,
//! `secrets.json` с `mcp.servers`.
//!
//! `HDS_CONFIG`/`HDS_ROOT`/`HDS_ANONYMIZE_*` — глобальные переменные процесса:
//! тесты идут под одним мьютексом, каждый со своим временным каталогом.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use hds_agent::secrets::Secrets;
use hds_agent::{projects, settings, Project};
use hds_core::config::{load_from, Config};
use hds_ui::agent_session::AgentApi;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Mock-воркер анонимизации — общий с A5.2…A5.5 (`fixtures/mock_anon.py`).
const MOCK_ANON: &str = include_str!("fixtures/mock_anon.py");

/// Вымышленный ключ теста: его не должно быть ни в одном ответе API.
const KEY: &str = "sk-a56-socket-secret";
/// Значение из фикстур mock-воркера: в истории лежит открытым, после очистки — метка.
const VALUE: &str = "Иванов Иван";

fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("agent-settings-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Корень поставки без verbatim-префикса.
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let resolved = joined.canonicalize().unwrap_or(joined);
    let text = resolved.to_string_lossy().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => resolved,
    }
}

/// Интерпретатор mock-воркера: `HDS_ANONYMIZE_PYTHON` или venv репозитория.
fn worker_python(root: &Path) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HDS_ANONYMIZE_PYTHON") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let path = root
        .join("sidecar")
        .join("venv-anonymize")
        .join(if cfg!(windows) { "Scripts" } else { "bin" })
        .join(if cfg!(windows) {
            "python.exe"
        } else {
            "python"
        });
    path.is_file().then_some(path)
}

/// Временный стенд: папка проекта, зона, `config.yaml` (с комментариями),
/// `projects.json` и `secrets.json`.
struct Stand {
    dir: PathBuf,
    cfg: Config,
}

impl Stand {
    fn new(tag: &str) -> Option<Stand> {
        Stand::build(tag, None)
    }

    fn with_profile(tag: &str, profile: Option<&str>) -> Option<Stand> {
        Stand::build(tag, profile)
    }

    fn build(tag: &str, profile: Option<&str>) -> Option<Stand> {
        let python = match worker_python(&repo_root()) {
            Some(path) => path,
            None => {
                eprintln!(
                    "A5.6 {tag} пропущен: нет интерпретатора воркера анонимизации \
                     (HDS_ANONYMIZE_PYTHON / sidecar/venv-anonymize)"
                );
                return None;
            }
        };
        let dir = tmpdir(tag);
        let project = dir.join("Проект");
        let zone = dir.join("zone");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&zone).unwrap();
        // Комментарии (в секции и хвостовые) — часть сценария: правка настроек
        // не должна их терять.
        let mut body = String::new();
        body.push_str("agent:                        # A5.6: комментарий секции\n");
        body.push_str("  enabled: true               # зона включена\n");
        body.push_str(&format!("  root: '{}'   # корень зоны\n", slash(&zone)));
        body.push_str("  ask_timeout_sec: 600        # хвостовой комментарий\n");
        body.push_str("  # filter.max_stale_sec: 300  # дефолт (комментарий не трогать)\n");
        body.push('\n');
        body.push_str(&format!("db_path: '{}'\n", slash(&dir.join("index.db"))));
        body.push_str("embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n");
        body.push_str("index:\n  clip: false\n");
        if let Some(profile) = profile {
            body.push_str(&format!("profile: {profile}\n"));
        }
        let cfg_path = dir.join("config.yaml");
        fs::write(&cfg_path, body).unwrap();
        std::env::set_var("HDS_CONFIG", &cfg_path);
        std::env::set_var("HDS_ROOT", &dir);
        std::env::set_var("HDS_ANONYMIZE_PYTHON", &python);
        let script = dir.join("mock_anon.py");
        fs::write(
            &script,
            MOCK_ANON.replace("__SIDECAR__", &slash(&repo_root().join("sidecar"))),
        )
        .unwrap();
        std::env::set_var("HDS_ANONYMIZE_WORKER", &script);
        let cfg = load_from(&cfg_path).expect("config.yaml");
        projects::add(
            &cfg,
            Project {
                id: "P0001".to_string(),
                name: "Пилот".to_string(),
                path: project.to_string_lossy().to_string(),
                provider: "openai-compatible".to_string(),
                model: "mock-model".to_string(),
                created: 0,
            },
        )
        .expect("projects.json");
        Some(Stand { dir, cfg })
    }

    fn config_path(&self) -> PathBuf {
        self.dir.join("config.yaml")
    }

    fn projects_path(&self) -> PathBuf {
        settings::projects_file(&self.cfg)
    }

    fn secrets_path(&self) -> PathBuf {
        settings::secrets_file(&self.cfg)
    }

    fn state_dir(&self) -> PathBuf {
        settings::state_dir_for(&self.cfg, "P0001").expect("agent-state")
    }

    /// `secrets.json` без ключа провайдера, но с `mcp.servers` и незнакомым
    /// блоком: правка ключа не должна их терять.
    fn seed_secrets(&self) {
        let path = self.secrets_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body = json!({
            "providers": { "openai-compatible": { "base_url": "http://127.0.0.1:9/v1" } },
            "mcp": {
                "servers": {
                    "1c-buddy": { "type": "stdio", "command": "C:\\tools\\1c-buddy.exe" }
                }
            },
            "future_block": { "keep": true }
        });
        fs::write(&path, serde_json::to_string_pretty(&body).unwrap()).unwrap();
    }

    /// Файлы истории SDK: манифест и сообщения со значением в открытом виде.
    fn seed_session(&self, id: &str) -> PathBuf {
        let dir = self
            .state_dir()
            .join(".cline")
            .join("data")
            .join("sessions")
            .join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{id}.json")),
            format!(r#"{{"id":"{id}"}}"#),
        )
        .unwrap();
        let messages = dir.join(format!("{id}.messages.json"));
        let body = format!(r#"{{"messages":[{{"role":"user","content":"Контакт: {VALUE}"}}]}}"#);
        fs::write(&messages, body).unwrap();
        messages
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        std::env::remove_var("HDS_CONFIG");
        std::env::remove_var("HDS_ROOT");
        std::env::remove_var("HDS_ANONYMIZE_PYTHON");
        std::env::remove_var("HDS_ANONYMIZE_WORKER");
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Поднять настоящий HTTP-сервер на эфемерном порту.
fn spawn_ui() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("порт 0");
    let addr = listener.local_addr().expect("локальный адрес");
    std::thread::spawn(move || {
        let _ = hds_ui::run_http_listener(listener);
    });
    addr
}

fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn http(addr: SocketAddr, request: String) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("соединение");
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .expect("таймаут чтения");
    stream.write_all(request.as_bytes()).expect("запрос");
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, body)
}

fn get(addr: SocketAddr, path: &str) -> (u16, Value) {
    let (status, body) = http(
        addr,
        format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"),
    );
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

fn post(addr: SocketAddr, path: &str, body: &Value, csrf: bool) -> (u16, Value) {
    let extra = if csrf {
        "Origin: http://127.0.0.1:8765\r\nContent-Type: application/json\r\nX-HDS-UI: 1\r\n"
    } else {
        ""
    };
    let body = body.to_string();
    let (status, text) = http(
        addr,
        format!(
            "POST {path} HTTP/1.1\r\nHost: test\r\n{extra}Content-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Общее состояние вкладки для сокет-тестов (`global` читает именно его).
fn install_global(stand: &Stand) -> Arc<AgentApi> {
    let _ = hds_ui::agent_session::install_global(AgentApi::for_tests_at(repo_root()));
    let api = Arc::clone(hds_ui::agent_session::global(&stand.cfg));
    api.reset_for_tests();
    api
}

fn settings_url(project: &str) -> String {
    format!("/api/agent/settings?project={}", urlencode(project))
}

/// GET без ключа → POST (модель, ключ, порог, политика) → файлы изменились,
/// комментарии `config.yaml` целы, значения ключа в ответах нет.
#[test]
fn settings_get_and_save_keep_files_and_comments() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("settings") else {
        return;
    };
    stand.seed_secrets();
    let api = install_global(&stand);
    let addr = spawn_ui();

    // 1. Ключа нет: только признак, значения в ответе быть не может.
    let (status, body) = get(addr, &settings_url("P0001"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["project"]["id"], "P0001");
    assert_eq!(body["project"]["provider"], "openai-compatible");
    assert_eq!(body["project"]["model"], "mock-model");
    assert_eq!(body["project"]["key"]["set"], false);
    assert_eq!(body["config"]["enabled"], true);
    assert_eq!(body["config"]["ask_timeout_sec"], 600);
    assert_eq!(body["config"]["unknown"], "label");
    assert_eq!(body["config"]["egress_log"], false);
    let providers = body["providers"].as_array().expect("providers");
    assert!(providers
        .iter()
        .any(|p| p["name"] == "openai-compatible" && p["key"]["set"] == false));
    let text = body.to_string();
    assert!(
        !text.contains("api_key") && !text.contains(KEY),
        "в ответе не должно быть ключа: {text}"
    );

    // 2. POST: модель, ключ, порог и политика фильтра.
    let (status, body) = post(
        addr,
        "/api/agent/settings",
        &json!({
            "project": "P0001",
            "model": "mock-model-2",
            "api_key": KEY,
            "ask_timeout_sec": 601,
            "unknown": "block",
            "egress_log": true
        }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["reopened"], true);
    assert_eq!(body["project"]["model"], "mock-model-2");
    assert_eq!(body["project"]["key"]["set"], true);
    let applied: Vec<&str> = body["applied"]
        .as_array()
        .expect("applied")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for field in [
        "model",
        "api_key",
        "ask_timeout_sec",
        "unknown",
        "egress_log",
    ] {
        assert!(applied.contains(&field), "нет {field} в {applied:?}");
    }
    let text = body.to_string();
    assert!(!text.contains(KEY), "ключ попал в ответ POST: {text}");

    // 3. projects.json: модель сменилась, остальные поля проекта целы.
    let registry = projects::load(&stand.cfg).expect("projects.json");
    let rec = registry.get("P0001").expect("проект");
    assert_eq!(rec.model, "mock-model-2");
    assert_eq!(rec.provider, "openai-compatible");
    assert_eq!(rec.name, "Пилот");
    assert_eq!(rec.created, 0);
    assert!(stand.projects_path().is_file());

    // 4. secrets.json: ключ записан, mcp.servers и незнакомый блок целы.
    let secrets = Secrets::load(&stand.secrets_path()).expect("secrets.json");
    assert_eq!(
        secrets.provider("openai-compatible").expect("ключ").api_key,
        KEY
    );
    assert_eq!(
        secrets.provider_base_url("openai-compatible").as_deref(),
        Some("http://127.0.0.1:9/v1")
    );
    assert_eq!(secrets.mcp_servers().len(), 1);
    let saved: Value =
        serde_json::from_str(&fs::read_to_string(stand.secrets_path()).unwrap()).expect("JSON");
    assert_eq!(
        saved.pointer("/future_block/keep"),
        Some(&Value::Bool(true)),
        "незнакомый блок потерян: {saved}"
    );

    // 5. config.yaml: значения записаны, комментарии и соседи на месте.
    let yaml = fs::read_to_string(stand.config_path()).expect("config.yaml");
    assert!(yaml.contains("# A5.6: комментарий секции"), "{yaml}");
    assert!(
        yaml.contains("  enabled: true               # зона включена"),
        "{yaml}"
    );
    assert!(
        yaml.contains("  ask_timeout_sec: 601  # хвостовой комментарий"),
        "{yaml}"
    );
    assert!(
        yaml.contains("  # filter.max_stale_sec: 300  # дефолт (комментарий не трогать)"),
        "{yaml}"
    );
    assert!(yaml.contains("    unknown: 'block'"), "{yaml}");
    assert!(yaml.contains("  egress_log: true"), "{yaml}");
    assert!(yaml.contains("index:\n  clip: false"), "{yaml}");
    let fresh = load_from(&stand.config_path()).expect("config.yaml заново");
    let now = settings::settings(&fresh);
    assert_eq!(now.ask_timeout_secs, 601);
    assert_eq!(now.unknown, settings::UnknownPolicy::Block);
    assert!(now.egress_log);

    // 6. «Настройки применены» — notice в ленте (без значений).
    let (_, events) = get(addr, "/api/agent/events?project=P0001&after=0");
    let serialized = events.to_string();
    assert!(serialized.contains("настройки применены"), "{serialized}");
    assert!(!serialized.contains(KEY), "{serialized}");

    api.reset_for_tests();
}

/// «Очистить историю»: отчёт `{session, files, backups, replaced}`, бэкапы на
/// месте, значение заменено меткой, `notice` в ленте без значений.
#[test]
fn history_clear_reports_files_backups_and_notice() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("history") else {
        return;
    };
    stand.seed_secrets();
    let api = install_global(&stand);
    let addr = spawn_ui();
    let messages = stand.seed_session("S-CLEAR");
    let before = fs::read_to_string(&messages).expect("история");
    assert!(before.contains(VALUE), "{before}");

    let (status, body) = post(
        addr,
        "/api/agent/history/clear",
        &json!({ "project": "P0001", "session": "S-CLEAR" }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["session"], "S-CLEAR");
    let files = body["files"].as_array().expect("files");
    assert_eq!(files.len(), 2, "манифест и сообщения: {body}");
    let backups = body["backups"].as_array().expect("backups");
    assert_eq!(backups.len(), 2, "{body}");
    assert!(body["replaced"].as_u64().expect("replaced") >= 1, "{body}");

    let cleaned = fs::read_to_string(&messages).expect("история после очистки");
    assert!(!cleaned.contains(VALUE), "значение осталось: {cleaned}");
    assert!(cleaned.contains("[PERSON_1]"), "метки нет: {cleaned}");
    let backup = messages.with_extension("json.bak");
    assert!(backup.is_file(), "бэкап не создан: {}", backup.display());
    let kept = fs::read_to_string(&backup).expect("бэкап");
    assert!(
        kept.contains(VALUE),
        "бэкап — копия прежней истории: {kept}"
    );

    // `notice` в ленте: числа и пути, но не значения.
    let (_, events) = get(addr, "/api/agent/events?project=P0001&after=0");
    let serialized = events.to_string();
    assert!(serialized.contains("история очищена"), "{serialized}");
    assert!(!serialized.contains(VALUE), "{serialized}");

    api.reset_for_tests();
}

/// Коды 400/404/409/403: разбор тел, CSRF-гейт, «ход идёт» и server-профиль.
#[test]
fn routes_validate_body_csrf_and_server_profile() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("errors") else {
        return;
    };
    stand.seed_secrets();
    let api = install_global(&stand);
    let addr = spawn_ui();

    // 400: нет project, пустой ключ, порог вне диапазона, плохая сессия.
    assert_eq!(get(addr, "/api/agent/settings").0, 400);
    assert_eq!(
        post(
            addr,
            "/api/agent/settings",
            &json!({ "project": "P0001" }),
            true
        )
        .0,
        400,
        "нечего менять"
    );
    assert_eq!(
        post(
            addr,
            "/api/agent/settings",
            &json!({ "project": "P0001", "api_key": "" }),
            true
        )
        .0,
        400
    );
    assert_eq!(
        post(
            addr,
            "/api/agent/settings",
            &json!({ "project": "P0001", "ask_timeout_sec": 5 }),
            true
        )
        .0,
        400
    );
    assert_eq!(
        post(
            addr,
            "/api/agent/history/clear",
            &json!({ "project": "P0001", "session": "a/b" }),
            true
        )
        .0,
        400
    );
    assert_eq!(
        post(
            addr,
            "/api/agent/history/clear",
            &json!({ "project": "P0001" }),
            true
        )
        .0,
        404,
        "состояния проекта ещё нет"
    );

    // 404: незарегистрированный проект.
    assert_eq!(get(addr, &settings_url("P9999")).0, 404);
    assert_eq!(
        post(
            addr,
            "/api/agent/settings",
            &json!({ "project": "P9999", "model": "x" }),
            true
        )
        .0,
        404
    );

    // 409: «ход идёт», шлюза нет — остановить нельзя (маршрут не чистит историю).
    let turn = api.turn("P0001");
    turn.set_busy_for_tests(true);
    let (status, body) = post(
        addr,
        "/api/agent/history/clear",
        &json!({ "project": "P0001" }),
        true,
    );
    assert_eq!(status, 409, "{body}");
    turn.set_busy_for_tests(false);

    // 403 без CSRF: ключ не записан.
    let (status, _) = post(
        addr,
        "/api/agent/settings",
        &json!({ "project": "P0001", "api_key": KEY }),
        false,
    );
    assert_eq!(status, 403);
    let secrets = Secrets::load(&stand.secrets_path()).expect("secrets.json");
    assert!(!secrets.has_provider_key("openai-compatible"));

    // Серверный профиль: вкладка недоступна (D20). Проверка последняя —
    // `Stand` переставляет `HDS_CONFIG`.
    let server = Stand::with_profile("server", Some("server"));
    if let Some(server) = server {
        let reply = hds_ui::agent::gate().expect_err("server-профиль — 403 (D20)");
        assert_eq!(reply.0, 403);
        drop(server);
    }
    api.reset_for_tests();
}
