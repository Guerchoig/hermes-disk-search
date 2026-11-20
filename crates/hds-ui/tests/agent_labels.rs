//! A5.5: панель «Метки проекта» на живом стеке — `labels`, `merge`, `unmerge`.
//!
//! Настоящий HTTP-сервер (`run_http_listener`, порт 0) и настоящий клиент
//! анонимизации на mock-скрипте `tests/fixtures/mock_anon.py`: NER-модели не
//! нужны, а состояние «базы меток» (метки, псевдонимы, журнал склеек) живёт в
//! процессе mock-воркера — у UI-клиента своё, как на стенде.
//!
//! Образы и записи реестра готовятся напрямую по контракту A4: маршруты меток
//! читают реестр и `workspace`, а не собирают зеркало заново.
//!
//! `HDS_CONFIG`/`HDS_ROOT` — глобальные переменные процесса: тесты идут под
//! одним мьютексом, каждый со своим временным каталогом.

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use hds_agent::mirror::sha256_file;
use hds_agent::registry_db::{Artifact, ArtifactRegistry, Kind, Status};
use hds_agent::results::MESSAGES_FILE;
use hds_agent::{projects, settings};
use hds_core::config::{load_from, Config};
use hds_ui::agent_session::AgentApi;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Mock-воркер анонимизации — общий с A5.2…A5.4 (`fixtures/mock_anon.py`).
const MOCK_ANON: &str = include_str!("fixtures/mock_anon.py");

const LETTER: &str = "Письмо.md";
const LETTER_ANON: &str = "Письмо [PERSON_1].md";
const REPORT: &str = "Отчёт.md";
const REPORT_ANON: &str = "Отчёт [PERSON_1].md";
/// Оригиналы: значения в зоне пользователя (в образах — метки).
const ORIGINAL: &str = "Контакт: Иванов Иван и Петров Пётр\n";
/// Образ до склейки: младшая метка `[PERSON_2]` есть в обоих файлах.
const IMAGE: &str = "Контакт: [PERSON_1] и [PERSON_2]\n";

fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("agent-labels-{tag}-{}-{n}", std::process::id()));
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

/// Временный стенд: папка проекта, зона, `config.yaml`, `projects.json`.
struct Stand {
    dir: PathBuf,
    project: PathBuf,
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
                    "A5.5 {tag} пропущен: нет интерпретатора воркера анонимизации \
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
        let mut body = format!(
            "agent:\n  enabled: true\n  root: '{}'\n\
             db_path: '{}'\n\
             embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n\
             index:\n  clip: false\n",
            slash(&zone),
            slash(&dir.join("index.db")),
        );
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
            hds_agent::Project {
                id: "P0001".to_string(),
                name: "Пилот".to_string(),
                path: project.to_string_lossy().to_string(),
                provider: "openai-compatible".to_string(),
                model: "mock-model".to_string(),
                created: 0,
            },
        )
        .expect("projects.json");
        Some(Stand { dir, project, cfg })
    }

    fn db(&self) -> PathBuf {
        settings::registry_db(&self.cfg, "P0001").expect("реестр проекта")
    }

    fn image(&self, anon_rel: &str) -> PathBuf {
        settings::workspace_dir(&self.cfg, "P0001")
            .expect("workspace")
            .join(anon_rel)
    }

    fn messages(&self) -> String {
        fs::read_to_string(self.project.join(".hds-agent").join(MESSAGES_FILE)).unwrap_or_default()
    }

    /// Оригиналы, образы и записи реестра: в обоих образах есть `[PERSON_2]` —
    /// метка, которую пользователь склеит со старшей.
    fn seed(&self) {
        fs::create_dir_all(self.db().parent().unwrap()).unwrap();
        let registry = ArtifactRegistry::open(&self.db()).unwrap();
        for (orig, anon) in [(LETTER, LETTER_ANON), (REPORT, REPORT_ANON)] {
            let original = self.project.join(orig);
            fs::write(&original, ORIGINAL).unwrap();
            let image = self.image(anon);
            fs::create_dir_all(image.parent().unwrap()).unwrap();
            fs::write(&image, IMAGE).unwrap();
            let mut row = Artifact::new(orig, anon, Kind::Image, Status::Synced);
            row.orig_sha256 = Some(sha256_file(&original).unwrap());
            row.anon_sha256_issued = Some(sha256_file(&image).unwrap());
            row.anon_sha256_seen = row.anon_sha256_issued.clone();
            registry.upsert(&row).expect("запись реестра");
        }
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
        .set_read_timeout(Some(Duration::from_secs(60)))
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

fn labels_url(project: &str) -> String {
    format!("/api/agent/labels?project={}", urlencode(project))
}

/// Склейка, «Разъединить», «Не скрывать» — один проход по сокету.
#[test]
fn labels_merge_unmerge_cycle_on_socket() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("cycle") else {
        return;
    };
    stand.seed();
    let api = install_global(&stand);
    let addr = spawn_ui();

    // 1. `GET /api/agent/labels`: две метки, «где встречается» — 2 файла.
    let (status, body) = get(addr, &labels_url("P0001"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["labels"].as_array().unwrap().len(), 2, "{body}");
    let senior = &body["labels"][0];
    assert_eq!(senior["label"], "[PERSON_1]");
    assert_eq!(senior["type"], "PERSON");
    assert_eq!(senior["files"], 2, "{body}");
    assert_eq!(senior["forms_count"], 2, "{body}");
    assert_eq!(senior["occurrences"][0]["count"], 1, "{senior}");
    assert!(body["keep"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k == "1С:ERP"));
    assert_eq!(body["merges"].as_array().unwrap().len(), 0, "{body}");

    // 2. `POST /api/agent/labels/merge`: старшая [PERSON_1], младшая [PERSON_2].
    let (status, body) = post(
        addr,
        "/api/agent/labels/merge",
        &json!({ "project": "P0001", "keep": "[PERSON_1]", "drop": ["[PERSON_2]"] }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["merge_id"], 1, "{body}");
    assert_eq!(body["keep"], "[PERSON_1]");
    assert_eq!(body["dropped"][0], "[PERSON_2]");
    assert_eq!(body["rebuilt"].as_array().unwrap().len(), 2, "{body}");
    assert_eq!(body["message"]["kind"], "labels_merged");
    assert!(body["message"]["text"]
        .as_str()
        .unwrap()
        .contains("[PERSON_2]"));

    // 3. Образы пересобраны: старшая метка есть, младшей и значений нет.
    for anon in [LETTER_ANON, REPORT_ANON] {
        let image = fs::read_to_string(stand.image(anon)).unwrap();
        assert!(image.contains("[PERSON_1]"), "{anon}: {image}");
        assert!(!image.contains("[PERSON_2]"), "{anon}: {image}");
        assert!(!image.contains("Петров"), "{anon}: {image}");
    }

    // 4. Очередь агенту: `labels_merged`, только метки (I8).
    let messages = stand.messages();
    assert!(messages.contains("labels_merged"), "{messages}");
    assert!(!messages.contains("Петров"), "{messages}");
    assert!(!messages.contains("Иванов"), "{messages}");

    // 5. Лента узнала о склейке (`notice`), без значений.
    let events = api.events_json("P0001", 0, 500);
    let notice = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "notice")
        .cloned()
        .expect("notice в кольце");
    let text = notice["text"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("метки склеены"), "{notice}");
    assert!(!text.contains("Петров"), "{notice}");

    // 6. Панель после склейки: одна метка, младшая — псевдоним, склейка в журнале.
    let (status, body) = get(addr, &labels_url("P0001"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["labels"].as_array().unwrap().len(), 1, "{body}");
    assert_eq!(body["labels"][0]["aliases"][0], "[PERSON_2]");
    let merges = body["merges"].as_array().unwrap();
    assert_eq!(merges.len(), 1, "{body}");
    assert_eq!(merges[0]["keep"], "[PERSON_1]");
    assert_eq!(merges[0]["dropped"][0], "[PERSON_2]");

    // 7. Повторная склейка псевдонима — 409; неизвестная метка — 404.
    let (status, body) = post(
        addr,
        "/api/agent/labels/merge",
        &json!({ "project": "P0001", "keep": "[PERSON_1]", "drop": ["[PERSON_2]"] }),
        true,
    );
    assert_eq!(status, 409, "{body}");
    assert!(body["error"]
        .as_str()
        .unwrap_or_default()
        .contains("псевдоним"));
    let (status, body) = post(
        addr,
        "/api/agent/labels/merge",
        &json!({ "project": "P0001", "keep": "[PERSON_1]", "drop": ["[PERSON_9]"] }),
        true,
    );
    assert_eq!(status, 404, "{body}");

    // 8. `POST /api/agent/labels/unmerge`: метка вернулась, образ снова с ней.
    let (status, body) = post(
        addr,
        "/api/agent/labels/unmerge",
        &json!({ "project": "P0001", "merge_id": 1 }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["restored"][0], "[PERSON_2]");
    assert_eq!(body["message"]["kind"], "labels_unmerged");
    assert_eq!(body["rebuilt"].as_array().unwrap().len(), 2, "{body}");
    for anon in [LETTER_ANON, REPORT_ANON] {
        let image = fs::read_to_string(stand.image(anon)).unwrap();
        assert!(image.contains("[PERSON_2]"), "{anon}: {image}");
    }
    let messages = stand.messages();
    assert!(messages.contains("labels_unmerged"), "{messages}");
    let (status, body) = get(addr, &labels_url("P0001"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["labels"].as_array().unwrap().len(), 2, "{body}");
    assert_eq!(body["merges"].as_array().unwrap().len(), 0, "{body}");

    // 9. «Не скрывать» пишет в общую базу и остаётся в панели.
    let (status, body) = post(
        addr,
        "/api/agent/keep",
        &json!({ "project": "P0001", "value": "СКЛАД-ТЕСТ-55" }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["added"], true);
    let (_, body) = get(addr, &labels_url("P0001"));
    assert!(body["keep"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k == "СКЛАД-ТЕСТ-55"));

    api.reset_for_tests();
}

/// Коды 400/404/403: разбор тела, гейты CSRF и server-профиль (D20).
#[test]
fn labels_routes_validate_body_and_require_csrf() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("errors") else {
        return;
    };
    stand.seed();
    let _api = install_global(&stand);
    let addr = spawn_ui();

    // 400: нет drop, разные типы, «одна метка», нет merge_id, не JSON.
    let (status, _) = post(
        addr,
        "/api/agent/labels/merge",
        &json!({ "project": "P0001", "keep": "[PERSON_1]" }),
        true,
    );
    assert_eq!(status, 400);
    let (status, _) = post(
        addr,
        "/api/agent/labels/merge",
        &json!({ "project": "P0001", "keep": "[PERSON_1]", "drop": ["[ORG_1]"] }),
        true,
    );
    assert_eq!(status, 400);
    let (status, _) = post(
        addr,
        "/api/agent/labels/merge",
        &json!({ "project": "P0001", "keep": "[PERSON_1]", "drop": ["[PERSON_1]"] }),
        true,
    );
    assert_eq!(status, 400);
    let (status, _) = post(
        addr,
        "/api/agent/labels/unmerge",
        &json!({ "project": "P0001" }),
        true,
    );
    assert_eq!(status, 400);
    let (status, _) = http(
        addr,
        "POST /api/agent/labels/merge HTTP/1.1\r\nHost: test\r\n\
         Origin: http://127.0.0.1:8765\r\nContent-Type: application/json\r\n\
         X-HDS-UI: 1\r\nContent-Length: 7\r\nConnection: close\r\n\r\nне json"
            .to_string(),
    );
    assert_eq!(status, 400);

    // 404: нет проекта, нет такой склейки.
    let (status, _) = get(addr, &labels_url("P9999"));
    assert_eq!(status, 404);
    let (status, _) = post(
        addr,
        "/api/agent/labels/unmerge",
        &json!({ "project": "P0001", "merge_id": 99 }),
        true,
    );
    assert_eq!(status, 404);

    // 403 без CSRF: ничего не меняется.
    let (status, _) = post(
        addr,
        "/api/agent/labels/merge",
        &json!({ "project": "P0001", "keep": "[PERSON_1]", "drop": ["[PERSON_2]"] }),
        false,
    );
    assert_eq!(status, 403);
    let image = fs::read_to_string(stand.image(LETTER_ANON)).unwrap();
    assert!(image.contains("[PERSON_2]"), "{image}");
    assert!(stand.messages().is_empty(), "очередь пуста");

    // Серверный профиль: вкладка недоступна (D20). Проверка последняя —
    // `Stand` переставляет `HDS_CONFIG`.
    let server = Stand::with_profile("server", Some("server"));
    if let Some(server) = server {
        let reply = hds_ui::agent::gate().expect_err("server-профиль — 403 (D20)");
        assert_eq!(reply.0, 403);
        drop(server);
    }
}
