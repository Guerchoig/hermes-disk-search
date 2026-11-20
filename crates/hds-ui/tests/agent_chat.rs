//! A5.2: реплика вкладки «Агент» на живом стеке — предпросмотр, метки,
//! вложения, отправка хода и события.
//!
//! Что поднимается:
//! * настоящий воркер агента `sidecar/hds_agent/dist/worker.mjs` (как A3.2);
//! * mock-провайдер (`crates/hds-cli/tests/fixtures/mock_provider.mjs`) —
//!   OpenAI-совместимый, пишет тела запросов: проверяем, что к «модели» ушли
//!   метки, а не значения;
//! * mock-воркер анонимизации (Python, NDJSON через `hds_sidecar`) —
//!   `HDS_ANONYMIZE_WORKER`, поэтому NER-модели не нужны.
//!
//! Нет node / собранного воркера / интерпретатора venv — тест печатает причину
//! и выходит (как тесты `hds-agent`). `HDS_CONFIG`/`HDS_ROOT` — глобальные
//! переменные процесса, поэтому тесты идут под одним мьютексом.
//!
//! Маршруты `POST /api/agent/*` проверяются отдельно (`agent_api.rs`, A5.1):
//! здесь работа идёт через тот же [`AgentApi`], который зовут обработчики —
//! HTTP-слой поверх него тонкий (разбор тела + `ApiError` → статус).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use hds_agent::registry_db::{Artifact, ArtifactRegistry, Kind, Status};
use hds_agent::{projects, settings};
use hds_core::config::{load_from, Config};
use hds_ui::agent_session::AgentApi;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Mock-провайдер: тот же файл, что у тестов CLI (A3.3) — один источник.
const MOCK_PROVIDER: &str = include_str!("../../hds-cli/tests/fixtures/mock_provider.mjs");

/// Mock-воркер анонимизации: минимальный NDJSON-сервер через `hds_sidecar`.
/// Две формы «Иванова» дают метку с падежным суффиксом — как живой NER.
const MOCK_ANON: &str = r#"
import os
import sys

sys.path.insert(0, r"__SIDECAR__")
from hds_sidecar import runtime, stdio

FORMS = [("Иванову Ивану", "[PERSON_1|дат]"), ("Иванов Иван", "[PERSON_1]")]
KEEP = ["1С:ERP", "ТЗ"]


def handle(req):
    mid = req.get("id")
    method = req.get("method")
    params = req.get("params") or {}
    if method == "hello":
        stdio.ok(mid, {
            "protocol": 1, "python": "mock", "pid": os.getpid(), "backend": "mock",
            "providers": ["MockExecutionProvider"], "model": "mock/gliner-pii",
            "formats": ["docx", "xlsx", "txt", "md", "xml"],
            "capabilities": ["anonymize", "anonymize_text", "vault_snapshot", "vault_rev",
                             "label_for_value", "keep_terms", "mode_agent"],
            "schema_version": 2, "morph": "mock", "keep_terms": len(KEEP),
        })
        return False
    if method == "vault_snapshot":
        stdio.ok(mid, {
            "rev": 3,
            "labels": [{
                "label": "[PERSON_1]", "type": "PERSON",
                "forms": [
                    {"value": "Иванов Иван", "gram": "nomn", "origin": "ner"},
                    {"value": "Иванову Ивану", "gram": "datv", "origin": "ner"},
                ],
            }],
            "others": ["Петров Пётр"],
            "keep": KEEP,
        })
        return False
    if method == "vault_rev":
        stdio.ok(mid, {"rev": 3})
        return False
    if method == "anonymize_text":
        text = params.get("text") or ""
        labels = []
        for value, label in FORMS:
            if value in text:
                text = text.replace(value, label)
                labels.append({"label": label, "type": "PERSON", "count": 1})
        if "Петров" in text:
            text = text.replace("Петров", "[PERSON_2]")
            labels.append({"label": "[PERSON_2]", "type": "PERSON", "count": 1})
        stdio.ok(mid, {"text": text, "labels": labels,
                       "project_id": params.get("project_id"), "warnings": []})
        return False
    if method == "label_for_value":
        stdio.ok(mid, {"project_id": params.get("project_id"),
                       "value_type": params.get("type") or "OTHER",
                       "label": "[PERSON_2]", "kept": False, "confident": True})
        return False
    if method == "keep_terms":
        if params.get("action") == "add":
            stdio.ok(mid, {"value": params.get("value"), "added": True})
        else:
            stdio.ok(mid, {"keep": KEEP})
        return False
    if method == "anonymize":
        in_path = params.get("in_path")
        out_path = params.get("out_path")
        with open(in_path, "rb") as source:
            data = source.read()
        with open(out_path, "wb") as target:
            target.write(b"<!-- hds-agent/mock -->\n" + data)
        stdio.ok(mid, {"out_path": out_path, "labels": [], "warnings": [],
                       "project_id": params.get("project_id")})
        return False
    if method == "shutdown":
        stdio.ok(mid, {"ok": True})
        return True
    stdio.err(mid, -32601, "unknown method: %s" % method)
    return False


stdio.init()
sys.exit(runtime.serve(handle, idle_timeout=30.0))
"#;

/// Тестовый каталог: `<tmp>/agent-chat-<tag>-<pid>-<n>`.
fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("agent-chat-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Корень поставки без verbatim-префикса (Node его не переваривает).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let resolved = joined.canonicalize().unwrap_or(joined);
    let text = resolved.to_string_lossy().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => resolved,
    }
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Интерпретатор воркера анонимизации: `HDS_ANONYMIZE_PYTHON` или venv.
fn python_available(root: &Path) -> bool {
    if let Some(path) = std::env::var_os("HDS_ANONYMIZE_PYTHON") {
        return Path::new(&path).is_file();
    }
    let name = if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    };
    root.join("sidecar")
        .join("venv-anonymize")
        .join(if cfg!(windows) { "Scripts" } else { "bin" })
        .join(name)
        .is_file()
}

/// Все предпосылки живого прогона; `false` — тест печатает причину и выходит.
fn prerequisites(tag: &str) -> bool {
    let root = repo_root();
    let worker = root
        .join("sidecar")
        .join("hds_agent")
        .join("dist")
        .join("worker.mjs");
    if !worker.is_file() {
        eprintln!(
            "{tag} пропущен: нет {} (cd sidecar/hds_agent && npm run build)",
            worker.display()
        );
        return false;
    }
    if !node_available() {
        eprintln!("{tag} пропущен: node не найден в PATH");
        return false;
    }
    if !python_available(&root) {
        eprintln!("{tag} пропущен: нет интерпретатора воркера анонимизации (venv-anonymize)");
        return false;
    }
    true
}

/// Mock-провайдер (OpenAI-совместимый) на `127.0.0.1:0`.
struct Mock {
    child: Child,
    base_url: String,
    log: PathBuf,
}

impl Mock {
    fn start(dir: &Path, script: &[Value]) -> Mock {
        let file = dir.join("mock_provider.mjs");
        fs::write(&file, MOCK_PROVIDER).expect("mock_provider.mjs");
        let log = dir.join("mock-requests.jsonl");
        let _ = fs::remove_file(&log);
        let mut child = Command::new("node")
            .arg(&file)
            .env("MOCK_REQUESTS", &log)
            .env(
                "MOCK_SCRIPT",
                serde_json::to_string(script).expect("сценарий"),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("mock-провайдер");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout mock"))
            .read_line(&mut line)
            .expect("PORT от mock");
        let port: u16 = line
            .trim()
            .strip_prefix("PORT ")
            .unwrap_or_else(|| panic!("mock не сообщил порт: {line:?}"))
            .parse()
            .expect("порт");
        Mock {
            child,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            log,
        }
    }

    /// Тела запросов, как их получил mock (после фильтра).
    fn raw(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Стенд: временный `<tmp>` (он же `HDS_ROOT`), зона агента, папка проекта,
/// `projects.json`, `secrets.json`, реестр с одним зеркалированным файлом.
struct Stand {
    dir: PathBuf,
    cfg: Config,
    zone: PathBuf,
    project: PathBuf,
}

impl Stand {
    fn new(tag: &str) -> Stand {
        std::env::remove_var("HDS_ANONYMIZE_PYTHON");
        std::env::remove_var("HDS_ANONYMIZE_WORKER");
        let dir = tmpdir(tag);
        let project = dir.join("Проект");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("ТЗ.docx"), b"PK-fake-docx").unwrap();
        let zone = dir.join("zone");
        fs::create_dir_all(&zone).unwrap();
        let cfg_path = dir.join("config.yaml");
        fs::write(
            &cfg_path,
            format!(
                "agent:\n  enabled: true\n  root: '{}'\n  db_path: '{}'\n\
                 embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n\
                 index:\n  clip: false\n\
                 anonymize:\n  output_dir: '{}'\n\
                 mcp_http:\n  port: 8787\n  path: /mcp\n",
                slash(&zone),
                slash(&dir.join("index.db")),
                slash(&dir.join("data/anonymized")),
            ),
        )
        .unwrap();
        std::env::set_var("HDS_CONFIG", &cfg_path);
        std::env::set_var("HDS_ROOT", &dir);
        // Mock-воркер анонимизации (Python): NER-модели не нужны.
        let script = dir.join("mock_anon.py");
        fs::write(
            &script,
            MOCK_ANON.replace("__SIDECAR__", &slash(&repo_root().join("sidecar"))),
        )
        .unwrap();
        std::env::set_var("HDS_ANONYMIZE_WORKER", &script);
        let cfg = load_from(&cfg_path).expect("config.yaml");
        Stand {
            dir,
            cfg,
            zone,
            project,
        }
    }

    fn workspace(&self) -> PathBuf {
        self.zone.join("P0001").join("workspace")
    }

    /// Проект в `projects.json` (id `P0001`, провайдер mock).
    fn add_project(&self) {
        projects::add(
            &self.cfg,
            hds_agent::Project {
                id: "P0001".to_string(),
                name: "Пилот".to_string(),
                path: self.project.to_string_lossy().to_string(),
                provider: "openai-compatible".to_string(),
                model: "mock-model".to_string(),
                created: 0,
            },
        )
        .expect("projects.json");
    }

    fn write_secrets(&self, base_url: &str) {
        let path = settings::secrets_file(&self.cfg);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let secrets = json!({
            "projects": {
                "P0001": { "token": "tok-a52", "provider": "openai-compatible", "model": "mock-model" }
            },
            "providers": { "openai-compatible": { "api_key": "sk-a52-secret", "base_url": base_url } }
        });
        fs::write(&path, serde_json::to_string_pretty(&secrets).unwrap()).unwrap();
    }

    /// Реестр с одним файлом: `ТЗ.docx` → образ `ТЗ [ORG_1].docx`.
    fn mirror_one(&self) {
        let db = settings::registry_db(&self.cfg, "P0001").expect("реестр проекта");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        let registry = ArtifactRegistry::open(&db).expect("реестр");
        registry
            .insert(&Artifact::new(
                "ТЗ.docx",
                "ТЗ [ORG_1].docx",
                Kind::Image,
                Status::Synced,
            ))
            .expect("запись реестра");
        let workspace = self.workspace();
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join("ТЗ [ORG_1].docx"), b"PK-image").unwrap();
    }

    /// `AgentApi` стенда: свой реестр сессий, корень поставки — репозиторий.
    fn api(&self) -> std::sync::Arc<AgentApi> {
        AgentApi::for_tests_at(repo_root())
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        std::env::remove_var("HDS_CONFIG");
        std::env::remove_var("HDS_ROOT");
        std::env::remove_var("HDS_ANONYMIZE_WORKER");
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Подождать, пока насос опустит `busy` (событие `turn_end`/`done`).
fn wait_idle(api: &AgentApi, project: &str, timeout: Duration) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        let value = api.events_json(project, 0, 500);
        if value["busy"] == false && value["seq"].as_u64().unwrap_or(0) > 0 {
            return value;
        }
        if Instant::now() > deadline {
            return value;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn preview_shows_labels_and_paths() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !prerequisites("A5.2 preview") {
        return;
    }
    let stand = Stand::new("preview");
    stand.add_project();
    stand.mirror_one();
    let api = stand.api();

    // TR-5.1: «Иванову Ивану» → [PERSON_1|дат], падеж сохранён.
    let value = api
        .preview(&stand.cfg, "P0001", "Подготовь письмо Иванову Ивану")
        .expect("предпросмотр");
    assert_eq!(value["changed"], true, "{value}");
    let text = value["text"].as_str().unwrap();
    assert!(text.contains("[PERSON_1|дат]"), "{text}");
    assert!(!text.contains("Иванов"), "значение не ушло: {text}");
    assert_eq!(value["labels"][0]["token"], "[PERSON_1|дат]");
    assert_eq!(value["labels"][0]["type"], "PERSON");

    // D19: «1С:ERP» в keep_terms — не скрывается, текст не меняется.
    let kept = api
        .preview(&stand.cfg, "P0001", "Сравни 1С:ERP с прошлым годом")
        .expect("предпросмотр keep");
    assert_eq!(kept["changed"], false, "{kept}");
    assert!(kept["text"].as_str().unwrap().contains("1С:ERP"));

    // Путь файла проекта: соответствие `paths` ведёт к образу в workspace.
    let path_text = format!("Прочитай {}", stand.project.join("ТЗ.docx").display());
    let with_path = api
        .preview(&stand.cfg, "P0001", &path_text)
        .expect("предпросмотр пути");
    let paths = with_path["paths"].as_array().expect("paths");
    assert_eq!(paths.len(), 1, "{with_path}");
    // Абсолютный путь фильтр заменил на [SYS_n], поэтому агенту предлагается
    // относительный путь образа (cwd воркера — workspace).
    assert_eq!(paths[0]["anon"], "ТЗ [ORG_1].docx", "{with_path}");
    let to = paths[0]["to"].as_str().unwrap();
    assert!(to.ends_with("ТЗ [ORG_1].docx"), "{to}");
    assert!(Path::new(to).is_file(), "{to}");
    assert!(
        with_path["text"].as_str().unwrap().contains("[SYS_"),
        "папка проекта — системная строка: {with_path}"
    );

    // Валидация: пусто — 400, сверх лимита — 400.
    assert_eq!(
        api.preview(&stand.cfg, "P0001", "   ").unwrap_err().status,
        400
    );
    let long = "я".repeat(hds_ui::agent_session::MAX_TEXT + 1);
    assert_eq!(
        api.preview(&stand.cfg, "P0001", &long).unwrap_err().status,
        400
    );
    assert!(
        api.preview(&stand.cfg, "P0001", "текст").is_ok(),
        "проект есть — ошибки нет"
    );
    api.reset_for_tests();
}

#[test]
fn label_keep_and_attach() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !prerequisites("A5.2 label") {
        return;
    }
    let stand = Stand::new("label");
    stand.add_project();
    stand.mirror_one();
    let api = stand.api();

    // «Скрыть ещё…»: метка выдаётся, значение не возвращается.
    let labeled = api
        .label(&stand.cfg, "P0001", "Петрова", "PERSON")
        .expect("метка");
    assert_eq!(labeled["label"], "[PERSON_2]");
    assert_eq!(labeled["kept"], false);
    assert_eq!(labeled["confident"], true);
    assert!(!labeled.to_string().contains("Петрова"), "{labeled}");

    // «Не скрывать»: запись в базу (глобально), идемпотентность — за воркером.
    let kept = api.keep(&stand.cfg, "P0001", "Ромашка").expect("keep");
    assert_eq!(kept["added"], true);
    assert_eq!(kept["scope"], "база");

    // Вложение из папки проекта — путь образа по реестру.
    let image = api
        .attach(
            &stand.cfg,
            "P0001",
            &stand.project.join("ТЗ.docx").to_string_lossy(),
        )
        .expect("вложение проекта");
    assert_eq!(image["kind"], "image", "{image}");
    assert_eq!(image["rel"], "ТЗ.docx");
    assert_eq!(image["status"], "synced");
    assert!(Path::new(image["path"].as_str().unwrap()).is_file());

    // Файл вне папки проекта — разовая копия в workspace\_attachments.
    let outside = stand.dir.join("заметки.txt");
    fs::write(&outside, "заметки вне проекта").unwrap();
    let attachment = api
        .attach(&stand.cfg, "P0001", &outside.to_string_lossy())
        .expect("вложение вне проекта");
    assert_eq!(attachment["kind"], "attachment", "{attachment}");
    let copy = PathBuf::from(attachment["path"].as_str().unwrap());
    assert!(copy.is_file(), "{}", copy.display());
    assert_eq!(copy.file_name().unwrap(), "заметки.txt");
    assert!(copy.starts_with(stand.workspace()));

    // Файл в папке проекта, но не зеркалированный — 409 и подсказка про sync.
    fs::write(stand.project.join("новый.docx"), b"PK-new").unwrap();
    let err = api
        .attach(
            &stand.cfg,
            "P0001",
            &stand.project.join("новый.docx").to_string_lossy(),
        )
        .unwrap_err();
    assert_eq!(err.status, 409, "{}", err.message);
    assert!(err.message.contains("hds agent sync"), "{}", err.message);

    // Неизвестный проект — 404.
    assert_eq!(
        api.label(&stand.cfg, "P9999", "Петров", "PERSON")
            .unwrap_err()
            .status,
        404
    );
    api.reset_for_tests();
}

#[test]
fn message_turn_second_409_and_abort() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !prerequisites("A5.2 message") {
        return;
    }
    let stand = Stand::new("message");
    stand.add_project();
    stand.mirror_one();
    let mock = Mock::start(&stand.dir, &[json!({ "text": "Готово по письму" })]);
    stand.write_secrets(&mock.base_url);
    let api = stand.api();

    let started = api
        .message(&stand.cfg, "P0001", "Подготовь письмо Иванову Ивану", None)
        .expect("первый ход");
    assert_eq!(started["ok"], true, "{started}");
    assert!(
        started["session_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "{started}"
    );
    assert!(
        api.existing_turn("P0001").expect("ход").is_busy(),
        "ход идёт"
    );

    // Второй ход при активном — 409 (В-A5.2-3).
    let second = api
        .message(&stand.cfg, "P0001", "И ещё одно письмо", None)
        .unwrap_err();
    assert_eq!(second.status, 409);
    assert!(
        second.message.contains("ход уже идёт"),
        "{}",
        second.message
    );

    let events = wait_idle(&api, "P0001", Duration::from_secs(60));
    assert_eq!(events["busy"], false, "{events}");
    let kinds: Vec<String> = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str().map(str::to_string))
        .collect();
    for kind in ["prompt", "agent", "turn_end"] {
        assert!(kinds.iter().any(|k| k == kind), "нет {kind}: {kinds:?}");
    }
    // `prompt` — до начала хода: он раньше первого события агента.
    let prompt_at = kinds.iter().position(|k| k == "prompt").unwrap();
    let agent_at = kinds.iter().position(|k| k == "agent").unwrap();
    assert!(prompt_at < agent_at, "{kinds:?}");

    // «Как увидит модель»: в событиях — метки, в запросе к провайдеру — тоже
    // метки. Системная подсказка (правила) содержит слово «Иванов» как пример,
    // поэтому значение ищем только в сообщении пользователя.
    let raw_events = events["events"].to_string();
    assert!(raw_events.contains("PERSON_1"), "{raw_events}");
    let requests: Vec<Value> = mock
        .raw()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    assert!(!requests.is_empty(), "провайдер не получил запрос");
    let user_text = requests
        .iter()
        .flat_map(|r| r["messages"].as_array().cloned().unwrap_or_default())
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str().map(str::to_string))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(user_text.contains("[PERSON_1|дат]"), "{user_text}");
    assert!(
        !user_text.contains("Иванов"),
        "значение ушло модели: {user_text}"
    );
    // Ключ провайдера в ответах API вкладки не появляется.
    assert!(!started.to_string().contains("sk-a52-secret"));

    // Стоп: `abort` не ломает сессию, `busy` снимается.
    let aborted = api.abort(&stand.cfg, "P0001").expect("стоп");
    assert_eq!(aborted["aborted"], true);
    let after = wait_idle(&api, "P0001", Duration::from_secs(30));
    assert_eq!(after["busy"], false, "{after}");
    assert_eq!(
        api.abort(&stand.cfg, "P9999").unwrap_err().status,
        404,
        "у проекта не было хода"
    );
    api.reset_for_tests();
}

#[test]
fn message_requires_zone_and_project() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("zone");
    stand.add_project();
    let api = stand.api();
    // Пустой текст — 400 до любой работы с воркерами.
    assert_eq!(
        api.message(&stand.cfg, "P0001", "  ", None)
            .unwrap_err()
            .status,
        400
    );
    // Без `secrets.json` план не собирается: 503 с текстом, не паника.
    let err = api
        .message(&stand.cfg, "P0001", "привет", None)
        .unwrap_err();
    assert_eq!(err.status, 503, "{}", err.message);
    assert!(err.message.contains("секрет") || err.message.contains("secrets.json"));
    // Второй вызов не залипает в busy: ошибка хода снимает флаг.
    assert!(!api.existing_turn("P0001").expect("ход").is_busy());
    api.reset_for_tests();
}

/// Маршруты вкладки: POST без CSRF — 403, server-профиль — 403 (D20), пустой
/// проект — 400. Воркеры здесь не нужны: гейты отвечают раньше.
#[test]
fn routes_gate_csrf_server_profile_and_project() {
    use hds_ui::{route, ReqHeaders};

    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("routes");
    stand.add_project();

    let body = json!({ "project": "P0001", "text": "привет" }).to_string();
    // Без `X-HDS-UI`/Origin POST отклоняется CSRF-гейтом.
    let (status, _, _) = route(
        "POST",
        "/api/agent/message",
        "",
        &ReqHeaders::default(),
        &body,
    );
    assert_eq!(status, 403, "CSRF");
    let (status, _, _) = route(
        "POST",
        "/api/agent/preview",
        "",
        &ReqHeaders::default(),
        &body,
    );
    assert_eq!(status, 403, "CSRF preview");

    // С заголовками запрос доходит до обработчика: пустой текст — 400.
    let headers = ReqHeaders {
        origin: Some("http://127.0.0.1:8765".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: Some("1".into()),
        ..ReqHeaders::default()
    };
    let (status, _, text) = route(
        "POST",
        "/api/agent/preview",
        "",
        &headers,
        &json!({ "project": "P0001", "text": "  " }).to_string(),
    );
    assert_eq!(status, 400, "{text}");
    // Событий ещё не было — пустой ответ, а не ошибка.
    let (status, _, text) = route(
        "GET",
        "/api/agent/events",
        "project=P0001",
        &ReqHeaders::default(),
        "",
    );
    assert_eq!(status, 200, "{text}");
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["busy"], false);
    assert_eq!(value["seq"], 0);
    assert!(value["events"].as_array().unwrap().is_empty());
    // Без проекта — 400.
    let (status, _, _) = route("GET", "/api/agent/events", "", &ReqHeaders::default(), "");
    assert_eq!(status, 400);

    drop(stand);
    // Server-профиль: вкладка недоступна (D20) — 403 и на POST, и на GET.
    let server = Stand::new("routes-server");
    let cfg_path = server.dir.join("config.yaml");
    let mut text = fs::read_to_string(&cfg_path).unwrap();
    text.push_str("profile: server\n");
    fs::write(&cfg_path, text).unwrap();
    let (status, _, body) = route(
        "GET",
        "/api/agent/events",
        "project=P0001",
        &ReqHeaders::default(),
        "",
    );
    assert_eq!(status, 403, "{body}");
    let (status, _, body) = route(
        "POST",
        "/api/agent/preview",
        "",
        &ReqHeaders {
            origin: Some("http://127.0.0.1:8765".into()),
            content_type: Some("application/json".into()),
            x_hds_ui: Some("1".into()),
            ..ReqHeaders::default()
        },
        &json!({ "project": "P0001", "text": "привет" }).to_string(),
    );
    assert_eq!(status, 403, "{body}");
    assert!(
        body.contains("серверном профиле"),
        "текст про профиль: {body}"
    );
}
