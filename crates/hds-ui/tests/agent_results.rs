//! A5.4: панель «Результаты» на живом стеке — `pending`, `compare`, `decision`.
//!
//! Поднимается настоящий HTTP-сервер (`run_http_listener`, порт 0) и настоящий
//! воркер анонимизации: mock-скрипт `tests/fixtures/mock_anon.py` (NDJSON через
//! `hds_sidecar`), интерпретатор — `sidecar/venv-anonymize` или
//! `HDS_ANONYMIZE_PYTHON`. NER-модели не нужны.
//!
//! Состояние варианта готовится напрямую по контракту A4: оригинал, снимок
//! основания, вариант в `.hds-agent\pending` и отчёт валидатора. Решение
//! принимает `Mirror::decide` — то же ядро, что у CLI, поэтому тест проверяет
//! маршруты и UI-договорённости, а не переписанную логику решений.
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
use hds_agent::results::{variant_paths, MESSAGES_FILE};
use hds_agent::validator::{Check, Report, Verdict, CHECK_LABELS, CHECK_TEXT_LOST};
use hds_agent::{projects, settings};
use hds_core::config::{load_from, Config};
use hds_ui::agent_session::AgentApi;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Mock-воркер анонимизации — общий с A5.2/A5.3 (`fixtures/mock_anon.py`).
const MOCK_ANON: &str = include_str!("fixtures/mock_anon.py");

const REL: &str = "06 Переписка/Письмо.md";
const ANON_REL: &str = "06 Переписка/Письмо [PERSON_1].md";

/// Путь как в реестре: разделитель платформы (маршрут принимает и `/`).
fn rel() -> String {
    REL.replace('/', std::path::MAIN_SEPARATOR_STR)
}

/// Путь образа как в реестре.
fn anon_rel() -> String {
    ANON_REL.replace('/', std::path::MAIN_SEPARATOR_STR)
}

fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("agent-results-{tag}-{}-{n}", std::process::id()));
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
    let name = if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    };
    let path = root
        .join("sidecar")
        .join("venv-anonymize")
        .join(if cfg!(windows) { "Scripts" } else { "bin" })
        .join(name);
    path.is_file().then_some(path)
}

/// Временный стенд: папка проекта, зона, `config.yaml`, `projects.json`.
struct Stand {
    dir: PathBuf,
    project: PathBuf,
    cfg: Config,
}

impl Stand {
    /// `None` — нет интерпретатора воркера: тест печатает причину и выходит.
    fn new(tag: &str) -> Option<Stand> {
        Self::with_profile(tag, None)
    }

    fn with_profile(tag: &str, profile: Option<&str>) -> Option<Stand> {
        let python = match worker_python(&repo_root()) {
            Some(path) => path,
            None => {
                eprintln!(
                    "A5.4 {tag} пропущен: нет интерпретатора воркера анонимизации \
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

    /// Состояние варианта по контракту A4: оригинал, основание, вариант, образ,
    /// запись реестра и отчёт валидатора.
    fn seed(&self, status: Status) -> Variant {
        let original = "Контакт: Иванов Иван\n";
        let variant = "Контакт: Иванов Иван — правка агента\n";
        let image = "Контакт: [PERSON_1] — правка агента\n";
        self.seed_with(status, original, variant, image)
    }

    fn seed_with(&self, status: Status, original: &str, variant: &str, image: &str) -> Variant {
        let orig = self.project.join(rel());
        fs::create_dir_all(orig.parent().unwrap()).unwrap();
        fs::write(&orig, original).unwrap();
        let paths = variant_paths(&self.project, &rel());
        fs::create_dir_all(paths.base.parent().unwrap()).unwrap();
        fs::write(&paths.base, original).unwrap();
        fs::create_dir_all(paths.pending.parent().unwrap()).unwrap();
        fs::write(&paths.pending, variant).unwrap();
        let image_path = self.image(&anon_rel());
        fs::create_dir_all(image_path.parent().unwrap()).unwrap();
        fs::write(&image_path, image).unwrap();

        fs::create_dir_all(self.db().parent().unwrap()).unwrap();
        let registry = ArtifactRegistry::open(&self.db()).unwrap();
        let mut row = Artifact::new(&rel(), &anon_rel(), Kind::Image, status);
        row.orig_sha256 = Some(sha256_file(&orig).unwrap());
        row.anon_sha256_issued = Some(sha256_file(&image_path).unwrap());
        row.anon_sha256_seen = row.anon_sha256_issued.clone();
        registry.upsert(&row).expect("запись реестра");

        let report = Report {
            rel: rel(),
            anon_rel: anon_rel(),
            ts: 1.0,
            rev: 3,
            verdict: Verdict::Check,
            checks: vec![
                Check {
                    name: CHECK_LABELS.to_string(),
                    verdict: Verdict::Ok,
                    detail: "меток вне проекта нет".to_string(),
                },
                Check {
                    name: CHECK_TEXT_LOST.to_string(),
                    verdict: Verdict::Check,
                    detail: "потеряно 41 % слов".to_string(),
                },
            ],
        };
        report.write(&paths.pending).expect("отчёт валидатора");
        Variant {
            original: orig,
            pending: paths.pending,
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

/// Что и где лежит в стенде после `seed`.
struct Variant {
    original: PathBuf,
    pending: PathBuf,
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

/// Процентное кодирование пути/запроса (кириллица и пробелы).
fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b'/' => out.push_str("%2F"),
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

fn pending_url(project: &str) -> String {
    format!("/api/agent/pending?project={}", urlencode(project))
}

fn compare_url(project: &str, rel: &str) -> String {
    format!(
        "/api/agent/compare?project={}&rel={}",
        urlencode(project),
        urlencode(rel)
    )
}

/// Ситуации 1 и 3 (TR-7.6): pending с отчётом, сравнение, приём и повтор.
#[test]
fn pending_compare_accept_updates_original_backup_and_image() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("accept") else {
        return;
    };
    let variant = stand.seed(Status::PendingAccept);
    let api = install_global(&stand);
    let addr = spawn_ui();

    // 1. `GET /api/agent/pending`: вариант, отчёт валидатора, признаки.
    let (status, body) = get(addr, &pending_url("P0001"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 1, "{body}");
    let item = &body["items"][0];
    assert_eq!(item["rel"], rel());
    assert_eq!(item["status"], "pending_accept");
    assert_eq!(item["conflict"], false);
    assert_eq!(item["variant"], true);
    assert_eq!(item["base"], true);
    assert_eq!(item["report"]["verdict"], "check", "{item}");
    assert_eq!(item["report"]["checks"], 2, "{item}");
    assert!(
        item["variant_path"]
            .as_str()
            .unwrap()
            .ends_with("Письмо.md"),
        "{item}"
    );

    // 2. `GET /api/agent/compare`: метаданные, отчёт, diff.
    let (status, body) = get(addr, &compare_url("P0001", REL));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["rel"], rel());
    assert_eq!(body["status"], "pending_accept");
    assert_ne!(body["original"]["sha256"], body["variant"]["sha256"]);
    assert_eq!(body["report"]["verdict"], "check", "{body}");
    assert_eq!(body["report"]["checks"].as_array().unwrap().len(), 2);
    let diff = body["diff"].as_array().unwrap();
    assert!(
        diff.iter()
            .any(|line| line.as_str().unwrap().starts_with("- ")),
        "{body}"
    );
    assert!(
        diff.iter()
            .any(|line| line.as_str().unwrap().starts_with("+ ")),
        "{body}"
    );
    assert_eq!(body["diff_truncated"], false);

    // 3. `POST /api/agent/decision` accept: оригинал = вариант, backup есть,
    //    образ пересоздан из нового оригинала, pending пуст.
    let (status, body) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": REL, "decision": "accept" }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "accepted", "{body}");
    let backup = PathBuf::from(body["backup"].as_str().expect("backup"));
    assert!(backup.is_file(), "{}", backup.display());
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        "Контакт: Иванов Иван\n"
    );
    assert_eq!(
        fs::read_to_string(&variant.original).unwrap(),
        "Контакт: Иванов Иван — правка агента\n"
    );
    let image = fs::read_to_string(stand.image(&anon_rel())).unwrap();
    assert!(image.contains("[PERSON_1]"), "{image}");
    assert!(
        !image.contains("Иванов Иван"),
        "в образе должны быть метки: {image}"
    );
    assert!(!variant.pending.exists(), "вариант после решения удалён");

    let (status, body) = get(addr, &pending_url("P0001"));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 0, "{body}");

    // 4. Лента узнала о решении (notice), без содержимого документа.
    let events = api.events_json("P0001", 0, 500);
    let notice = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["kind"] == "notice")
        .cloned()
        .expect("notice в кольце");
    let text = notice["text"]["text"].as_str().unwrap_or_default();
    assert!(text.contains("решение по"), "{notice}");
    assert!(!text.contains("правка агента"), "{notice}");

    // 5. Повторное решение — 409: решать уже нечего.
    let (status, body) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": REL, "decision": "accept" }),
        true,
    );
    assert_eq!(status, 409, "{body}");
    api.reset_for_tests();
}

/// Ситуация 4: «Отклонить» возвращает образ к оригиналу и уведомляет агента.
#[test]
fn reject_restores_image_and_notifies_agent() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("reject") else {
        return;
    };
    let variant = stand.seed(Status::PendingAccept);
    let _api = install_global(&stand);
    let addr = spawn_ui();

    let (status, body) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": REL, "decision": "reject" }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "synced", "{body}");
    assert!(body.get("backup").is_none(), "оригинал не менялся: {body}");
    assert_eq!(
        fs::read_to_string(&variant.original).unwrap(),
        "Контакт: Иванов Иван\n",
        "оригинал не тронут"
    );
    let image = fs::read_to_string(stand.image(&anon_rel())).unwrap();
    assert!(image.contains("[PERSON_1]"), "{image}");
    assert!(
        !image.contains("правка агента"),
        "образ вернулся к оригиналу: {image}"
    );
    assert!(!variant.pending.exists());
    let messages = stand.messages();
    assert!(messages.contains("document_rejected"), "{messages}");
    assert!(
        !messages.contains("Иванов Иван"),
        "в очередь агенту значения не уходят (I8): {messages}"
    );

    let (status, body) = get(addr, &pending_url("P0001"));
    assert_eq!(status, 200);
    assert_eq!(body["count"], 0, "{body}");
}

/// Ситуация 2 (TR-7.6): три исхода конфликта, каждый на своём стенде.
#[test]
fn conflict_resolves_with_three_choices() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (choice, expected) in [
        ("agent", "accepted"),
        ("new", "synced"),
        ("delegate", "synced"),
    ] {
        let Some(stand) = Stand::new(&format!("conflict-{choice}")) else {
            return;
        };
        let variant = stand.seed(Status::Conflict);
        // Пользователь правил оригинал, пока агент работал: sha ≠ основание.
        fs::write(
            &variant.original,
            "Контакт: Иванов Иван — правка пользователя\n",
        )
        .unwrap();
        let _api = install_global(&stand);
        let addr = spawn_ui();

        let (status, body) = get(addr, &pending_url("P0001"));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["items"][0]["status"], "conflict", "{body}");
        assert_eq!(body["items"][0]["conflict"], true, "{body}");

        let (status, body) = post(
            addr,
            "/api/agent/decision",
            &json!({
                "project": "P0001", "rel": REL,
                "decision": "conflict", "choice": choice,
            }),
            true,
        );
        assert_eq!(status, 200, "{choice}: {body}");
        assert_eq!(body["status"], expected, "{choice}: {body}");

        let original = fs::read_to_string(&variant.original).unwrap();
        let image = fs::read_to_string(stand.image(&anon_rel())).unwrap();
        match choice {
            // «Оставить вариант агента»: правки пользователя — в backup, есть
            // предупреждение (молча ничего не теряется).
            "agent" => {
                assert!(original.contains("правка агента"), "{original}");
                assert!(body["backup"].as_str().is_some(), "{body}");
                assert!(
                    body["warnings"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|w| w.as_str().unwrap().contains("потеряны")),
                    "{body}"
                );
                assert!(image.contains("[PERSON_1]"), "{image}");
            }
            // «Взять новую версию»: оригинал пользователя, образ из него.
            "new" => {
                assert!(original.contains("правка пользователя"), "{original}");
                assert!(image.contains("правка пользователя"), "{image}");
                assert!(!image.contains("правка агента"), "{image}");
                assert!(stand.messages().contains("document_rejected"));
            }
            // «Поручить агенту»: анонимная копия варианта в `_conflicts`.
            "delegate" => {
                assert!(original.contains("правка пользователя"), "{original}");
                let copy = body["conflict_copy"].as_str().expect("conflict_copy");
                assert!(copy.contains("_conflicts"), "{copy}");
                let workspace = settings::workspace_dir(&stand.cfg, "P0001").expect("workspace");
                let copy_path = workspace.join(copy);
                assert!(copy_path.is_file(), "{}", copy_path.display());
                let text = fs::read_to_string(&copy_path).unwrap();
                assert!(text.contains("[PERSON_1]"), "копия анонимна: {text}");
                assert!(!text.contains("Иванов Иван"), "{text}");
                assert!(stand.messages().contains("conflict_delegated"));
            }
            other => panic!("неизвестный исход {other}"),
        }

        assert!(!variant.pending.exists(), "{choice}: вариант убран");
        let (status, body) = get(addr, &pending_url("P0001"));
        assert_eq!(status, 200);
        assert_eq!(body["count"], 0, "{choice}: {body}");
        hds_ui::agent_session::global(&stand.cfg).reset_for_tests();
    }
}

/// Ситуация 3: «файл занят» → 409 с текстом A4, вариант и статус на месте.
#[cfg(windows)]
#[test]
fn busy_file_is_409_and_variant_stays() {
    use std::os::windows::fs::OpenOptionsExt;

    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("busy") else {
        return;
    };
    let variant = stand.seed(Status::PendingAccept);
    let _api = install_global(&stand);
    let addr = spawn_ui();

    // Открываем оригинал как офисный редактор: чтение можно, замену — нет.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0000_0001)
        .open(&variant.original)
        .expect("открыть как Word");
    let (status, body) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": REL, "decision": "accept" }),
        true,
    );
    assert_eq!(status, 409, "{body}");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(message.contains("файл занят"), "{message}");
    assert!(message.contains("вариант остался в pending"), "{message}");
    assert!(variant.pending.is_file(), "вариант на месте");
    assert_eq!(
        fs::read_to_string(&variant.original).unwrap(),
        "Контакт: Иванов Иван\n"
    );
    let registry = ArtifactRegistry::open(&stand.db()).unwrap();
    assert_eq!(
        registry.by_orig(&rel()).unwrap().unwrap().status,
        Status::PendingAccept,
        "статус не изменён"
    );
    drop(held);

    // После закрытия файла то же решение проходит.
    let (status, body) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": REL, "decision": "accept" }),
        true,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "accepted", "{body}");
}

/// POST без CSRF — 403; в серверном профиле вкладка недоступна (D20).
#[test]
fn decision_requires_csrf_and_desktop_profile() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(stand) = Stand::new("csrf") else {
        return;
    };
    let variant = stand.seed(Status::PendingAccept);
    let _api = install_global(&stand);
    let addr = spawn_ui();

    let (status, body) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": REL, "decision": "accept" }),
        false,
    );
    assert_eq!(status, 403, "{body}");
    assert!(variant.pending.is_file(), "без CSRF ничего не решается");
    assert_eq!(
        fs::read_to_string(&variant.original).unwrap(),
        "Контакт: Иванов Иван\n"
    );

    // Ошибки маршрута: неизвестный проект — 404, пустой rel и неизвестное
    // решение — 400.
    let (status, _) = get(addr, &pending_url("P9999"));
    assert_eq!(status, 404, "неизвестный проект");
    let (status, body) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": REL, "decision": "подписать" }),
        true,
    );
    assert_eq!(status, 400, "{body}");
    let (status, _) = post(
        addr,
        "/api/agent/decision",
        &json!({ "project": "P0001", "rel": "", "decision": "accept" }),
        true,
    );
    assert_eq!(status, 400, "пустой rel");

    // Серверный профиль: `agent::gate()` отвечает 403 (как A5.1). Проверка
    // последняя: `Stand` серверного профиля переставляет `HDS_CONFIG`.
    let server = Stand::with_profile("server", Some("server"));
    if let Some(server) = server {
        let reply = hds_ui::agent::gate().expect_err("server-профиль — 403 (D20)");
        assert_eq!(reply.0, 403);
        drop(server);
    }
}
