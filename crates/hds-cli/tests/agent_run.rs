//! A3.3: `hds agent project add|list` и `hds agent run` на mock-провайдере.
//!
//! Что проверяется:
//! * реестр проектов пишется в `projects.json` (путь — из `HDS_ROOT`), `list`
//!   печатает его (в т.ч. `--json`), `--dry-run` отдаёт план без ключей;
//! * `run` собирает сессию из конфига: каталоги `<P>\workspace|agent-config|
//!   agent-state`, правила, `cline_mcp_settings.json`, снимок vault (mock-воркер
//!   анонимизации), `env::build`, фильтр и `Gateway` поверх настоящего
//!   `dist/worker.mjs`;
//! * реплика с значением уходит модели меткой, ответ деанонимизирован,
//!   срабатывание фильтра видно событием `redacted`, успех — по `turn_end`;
//! * ключ провайдера не попадает ни в план, ни в файлы проекта.
//!
//! Предпосылки живого прогона: `node`, собранный `dist/worker.mjs` и
//! интерпретатор воркера анонимизации (venv). Нет — тест печатает причину и
//! выходит (как тесты `hds-agent`).

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use hds_agent::projects;
use hds_cli::cmd::agent::cmd_agent_cfg;
use hds_cli::cmd::agent_run::{self, RunOptions};
use hds_core::config::load_from;

/// Тесты трогают env (`HDS_ROOT`, `HDS_CONFIG`, `HDS_AGENT_SECRETS`).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Корень поставки (`sidecar/`) без verbatim-префикса: Node его не переваривает.
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

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Стенд: временный `<tmp>` (он же `HDS_ROOT`), зона агента и папка проекта.
struct Stand {
    dir: common::TempDir,
    cfg_path: PathBuf,
    cfg: hds_core::config::Config,
    project_dir: PathBuf,
    zone: PathBuf,
}

impl Stand {
    fn new(tag: &str) -> Stand {
        std::env::remove_var("HDS_AGENT_SECRETS");
        std::env::remove_var("HDS_CONFIG");
        let dir = common::TempDir::new(&format!("agentrun-{tag}"));
        let project_dir = dir.join("Проект");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join("note.txt"), "пользовательский файл").unwrap();
        let zone = dir.join("zone");
        fs::create_dir_all(&zone).unwrap();
        let cfg_path = dir.write_config(&format!(
            "agent:\n  enabled: true\n  root: '{}'\n\
             db_path: '{}'\n\
             embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n\
             index:\n  clip: false\n\
             anonymize:\n  output_dir: '{}'\n\
             mcp_http:\n  port: 8787\n  path: /mcp\n",
            slash(&zone),
            slash(&dir.join("index.db")),
            slash(&dir.join("data/anonymized")),
        ));
        std::env::set_var("HDS_CONFIG", &cfg_path);
        std::env::set_var("HDS_ROOT", &dir.path);
        let cfg = load_from(&cfg_path).expect("config.yaml");
        Stand {
            dir,
            cfg_path,
            cfg,
            project_dir,
            zone,
        }
    }

    fn workspace(&self) -> PathBuf {
        self.zone.join("P0001").join("workspace")
    }

    fn secrets_path(&self) -> PathBuf {
        self.dir.join("data").join("agent").join("secrets.json")
    }

    /// `secrets.json`: провайдер (base_url стенда), модель и токен проекта.
    fn write_secrets(&self, base_url: &str, api_key: &str) {
        let path = self.secrets_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let secrets = json!({
            "projects": {
                "P0001": {
                    "token": "tok-a33-project",
                    "provider": "openai-compatible",
                    "model": "mock-model"
                }
            },
            "providers": {
                "openai-compatible": { "api_key": api_key, "base_url": base_url }
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&secrets).unwrap()).unwrap();
    }

    fn add_project(&self) {
        let args: Vec<String> = vec![
            "project".to_string(),
            "add".to_string(),
            "P0001".to_string(),
            "--name".to_string(),
            "Проект стенда".to_string(),
            "--path".to_string(),
            self.project_dir.to_string_lossy().to_string(),
            "--provider".to_string(),
            "openai-compatible".to_string(),
            "--model".to_string(),
            "mock-model".to_string(),
        ];
        assert_eq!(cmd_agent_cfg(&self.cfg, &args), 0, "project add");
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        std::env::remove_var("HDS_ROOT");
        std::env::remove_var("HDS_CONFIG");
        std::env::remove_var("HDS_AGENT_SECRETS");
        std::env::remove_var("HDS_ANONYMIZE_WORKER");
    }
}

/// Mock-провайдер (OpenAI-совместимый) на `127.0.0.1:0`.
struct Mock {
    child: Child,
    base_url: String,
    log: PathBuf,
}

impl Mock {
    fn start(dir: &Path, script: &[Value]) -> Mock {
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("mock_provider.mjs");
        let log = dir.join("mock-requests.jsonl");
        let _ = fs::remove_file(&log);
        let mut child = Command::new("node")
            .arg(&file)
            .env("MOCK_REQUESTS", &log)
            .env("MOCK_SCRIPT", serde_json::to_string(script).unwrap())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("mock-провайдер");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
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

fn run_cli_bin(stand: &Stand, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hds"))
        .args(args)
        .env("HDS_CONFIG", &stand.cfg_path)
        .env("HDS_ROOT", &stand.dir.path)
        .output()
        .expect("hds")
}

#[test]
fn project_registry_and_plan() {
    let _env = ENV_LOCK.lock().unwrap();
    let stand = Stand::new("plan");
    stand.add_project();

    let stored = projects::get(&stand.cfg, "P0001").expect("проект в реестре");
    assert_eq!(stored.name, "Проект стенда");
    assert_eq!(stored.provider, "openai-compatible");
    assert_eq!(stored.model, "mock-model");
    assert_eq!(
        PathBuf::from(&stored.path),
        stand.project_dir,
        "путь папки проекта"
    );

    // `list` (human и --json) — через настоящий бинарь CLI.
    for args in [
        vec!["agent", "project", "list"],
        vec!["agent", "project", "list", "--json"],
    ] {
        let out = run_cli_bin(&stand, &args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("P0001"), "{args:?}: {text}");
    }
    let json_out = run_cli_bin(&stand, &["agent", "project", "list", "--json"]);
    let parsed: Value = serde_json::from_slice(&json_out.stdout).expect("JSON реестра");
    assert_eq!(parsed["projects"][0]["id"], "P0001");

    // План без процессов: провайдер из реестра, ключ — только в памяти.
    stand.write_secrets("https://example.invalid/v1", "sk-a33-secret-key");
    let opts = RunOptions::new(Some("привет".to_string()));
    let plan = agent_run::plan_with(&stand.cfg, "P0001", &opts, &repo_root()).expect("план сессии");
    assert_eq!(plan.provider.kind, "openai-compatible");
    assert_eq!(plan.provider.model, "mock-model");
    assert_eq!(plan.provider.base_url, "https://example.invalid/v1");
    assert!(plan.worker.ends_with("worker.mjs"));
    assert!(plan.workspace.ends_with("workspace"));
    assert!(!plan.workspace.exists(), "plan ничего не создаёт");

    let plan_value = agent_run::plan_json(&plan).to_string();
    assert!(plan_value.contains("P0001"));
    assert!(
        !plan_value.contains("sk-a33-secret-key"),
        "ключ не должен попадать в план: {plan_value}"
    );
    assert!(
        !plan_value.contains("tok-a33-project"),
        "токен не должен попадать в план"
    );

    // `prepare` — каталоги, правила, MCP-настройки.
    let mcp_path = agent_run::prepare(&plan).expect("prepare");
    assert!(plan.workspace.is_dir());
    assert!(plan.config_dir.join("rules").join("privacy.md").is_file());
    assert!(mcp_path.is_file(), "{}", mcp_path.display());
    let mcp: Value = serde_json::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
    let url = mcp["mcpServers"]["disk-search"]["transport"]["url"]
        .as_str()
        .unwrap_or_default();
    assert!(url.ends_with("/mcp/agent/P0001"), "маршрут A2: {url}");
}

#[test]
fn run_dry_run_prints_plan_without_secrets() {
    let _env = ENV_LOCK.lock().unwrap();
    let stand = Stand::new("dry");
    stand.add_project();
    stand.write_secrets("https://example.invalid/v1", "sk-a33-secret-key");
    // `--dry-run` не запускает воркер, но путь должен существовать.
    let dummy = stand.dir.join("sidecar/hds_agent/dist/worker.mjs");
    fs::create_dir_all(dummy.parent().unwrap()).unwrap();
    fs::write(&dummy, "// dummy\n").unwrap();

    let out = run_cli_bin(
        &stand,
        &[
            "agent",
            "run",
            "--project",
            "P0001",
            "--prompt",
            "тест",
            "--dry-run",
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let plan: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("план не JSON ({e}): {stdout}");
    });
    assert_eq!(plan["project"], "P0001");
    assert_eq!(plan["provider"]["model"], "mock-model");
    assert!(plan["worker"]
        .as_str()
        .unwrap_or_default()
        .ends_with("worker.mjs"));
    assert!(!stdout.contains("sk-a33-secret-key"), "ключ в плане");
    assert!(!stdout.contains("tok-a33-project"), "токен в плане");

    // Без --project — код 2; неизвестный проект — 1.
    assert_eq!(
        run_cli_bin(&stand, &["agent", "run", "--prompt", "x"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run_cli_bin(
            &stand,
            &["agent", "run", "--project", "P9999", "--prompt", "x"]
        )
        .status
        .code(),
        Some(1)
    );
}

/// Все файлы каталога с текстом (поиск утечек ключа).
fn files_under(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(text) = fs::read_to_string(&path) {
                out.push((path, text));
            }
        }
    }
    out
}

#[test]
fn run_turn_on_mock_provider() {
    let _env = ENV_LOCK.lock().unwrap();
    if !node_available() {
        eprintln!("A3.3 пропущен: node недоступен");
        return;
    }
    let root = repo_root();
    if !root.join("sidecar/hds_agent/dist/worker.mjs").is_file() {
        eprintln!("A3.3 пропущен: нет dist/worker.mjs (npm run build)");
        return;
    }
    if hds_anonymize::discover_python(&root).is_none() {
        eprintln!("A3.3 пропущен: нет интерпретатора воркера анонимизации (venv)");
        return;
    }

    let stand = Stand::new("turn");
    stand.add_project();
    fs::create_dir_all(stand.workspace()).unwrap();
    fs::write(
        stand.workspace().join("allowed.txt"),
        "ALLOWED-42 Иванов Иван",
    )
    .unwrap();

    let mock = Mock::start(
        &stand.dir.path,
        &[
            json!({"toolCalls": [{"name": "read_files", "arguments": {"files": [{"path": "allowed.txt"}]}}]}),
            json!({"text": "Готово: [PERSON_1] прочитан."}),
        ],
    );
    stand.write_secrets(&mock.base_url, "sk-a33-live-key");

    let opts = RunOptions {
        prompt: Some("что известно про Иванов Иван?".to_string()),
        max_iterations: 4,
        json: true,
        ..Default::default()
    };
    let plan = agent_run::plan_with(&stand.cfg, "P0001", &opts, &root).expect("план");
    assert_eq!(plan.provider.base_url, mock.base_url);
    assert_eq!(plan.provider.model, "mock-model");

    // Тот же подъём воркера, что у `hds agent run`: Python из репозитория,
    // скрипт подменён mock-воркером (снимок vault + anonymize_text, без NER).
    std::env::set_var(
        "HDS_ANONYMIZE_WORKER",
        root.join("crates/hds-anonymize/tests/mock_worker.py"),
    );
    let client = agent_run::open_client(&stand.cfg, &plan, &root).expect("воркер анонимизации");
    std::env::remove_var("HDS_ANONYMIZE_WORKER");

    let mut out = Vec::new();
    let mut err = Vec::new();
    let report = agent_run::run(&plan, Arc::clone(&client), &opts, &mut out, &mut err)
        .expect("ход на mock-провайдере");
    if let Ok(client) = Arc::try_unwrap(client) {
        if let Ok(client) = client.into_inner() {
            client.shutdown();
        }
    }
    let stderr = String::from_utf8_lossy(&err).to_string();
    assert!(report.ok, "ход не завершён: {report:?}; stderr={stderr}");
    assert_eq!(report.reason, "completed");
    assert!(
        report.text.contains("Иванов Иван"),
        "ответ должен быть деанонимизирован: {}",
        report.text
    );
    assert!(!report.text.contains("[PERSON_1]"));
    assert!(
        report.tools.iter().any(|t| t == "read_files"),
        "фактический список: {:?}",
        report.tools
    );

    // NDJSON-события: реплика ушла меткой, фильтр сработал, ход завершён.
    let text = String::from_utf8_lossy(&out);
    let events: Vec<Value> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("NDJSON-событие"))
        .collect();
    let prompt = events
        .iter()
        .find(|event| event["kind"] == "prompt")
        .expect("событие prompt");
    let prompt_text = prompt["text"].as_str().unwrap_or_default();
    assert!(
        prompt_text.contains("[PERSON_1]"),
        "реплика должна уйти меткой: {prompt_text}"
    );
    assert!(!prompt_text.contains("Иванов Иван"));
    assert!(
        events.iter().any(|event| event["kind"] == "redacted"),
        "фильтр должен сработать на теле запроса с содержимым файла: {text}"
    );
    assert!(
        events
            .iter()
            .any(|event| event["kind"] == "turn_end" && event["ok"] == true),
        "turn_end: {text}"
    );

    // Что увидела «модель»: метки, без значения.
    let raw = mock.raw();
    assert!(raw.contains("[PERSON_1]"), "метка ушла провайдеру");
    assert!(!raw.contains("Иванов Иван"), "значение ушло провайдеру");
    assert!(raw.contains("read_files"));

    // Собранная сессия: правила, MCP-настройки, история SDK в agent-state.
    assert!(plan.config_dir.join("rules").join("1c-domain.md").is_file());
    assert!(plan
        .state_dir
        .join(".cline")
        .join("data")
        .join("sessions")
        .is_dir());
    // Ключ провайдера — только в secrets.json (вне agent-config/agent-state).
    for (path, text) in files_under(&plan.config_dir)
        .into_iter()
        .chain(files_under(&plan.state_dir))
    {
        assert!(
            !text.contains("sk-a33-live-key"),
            "ключ провайдера в {}",
            path.display()
        );
    }
}

/// Тот же запуск бинаря с дополнительными переменными окружения (воркер
/// анонимизации: скрипт и интерпретатор).
fn run_cli_bin_env(stand: &Stand, args: &[&str], envs: &[(&str, String)]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hds"));
    command
        .args(args)
        .env("HDS_CONFIG", &stand.cfg_path)
        .env("HDS_ROOT", &stand.dir.path);
    for (name, value) in envs {
        command.env(name, value);
    }
    command.output().expect("hds")
}

/// Окружение воркера-заглушки анонимизации (снимок vault без NER-моделей).
fn anonymize_env() -> Option<Vec<(&'static str, String)>> {
    let root = repo_root();
    let python = root
        .join("sidecar")
        .join("venv-anonymize")
        .join("Scripts")
        .join("python.exe");
    if !python.is_file() {
        eprintln!("A3.4 пропущен: нет интерпретатора воркера анонимизации (venv)");
        return None;
    }
    Some(vec![
        (
            "HDS_ANONYMIZE_WORKER",
            root.join("crates")
                .join("hds-anonymize")
                .join("tests")
                .join("mock_worker.py")
                .to_string_lossy()
                .to_string(),
        ),
        ("HDS_ANONYMIZE_PYTHON", python.to_string_lossy().to_string()),
    ])
}

/// Файл истории сессии с значением — модель угрозы TR-6.6 (текст мог попасть в
/// файл до фильтра или из результата инструмента).
fn seed_session(stand: &Stand, session: &str, value: &str) -> PathBuf {
    let dir = stand
        .zone
        .join("P0001")
        .join("agent-state")
        .join(".cline")
        .join("data")
        .join("sessions")
        .join(session);
    fs::create_dir_all(&dir).unwrap();
    let messages = json!({
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": value }] }
        ]
    });
    let path = dir.join(format!("{session}.messages.json"));
    fs::write(&path, serde_json::to_string_pretty(&messages).unwrap()).unwrap();
    path
}

/// A3.4: `run --session` возобновляет сессию — первый запрос к модели во втором
/// прогоне содержит прошлую реплику (история читается из `agent-state`).
#[test]
fn run_session_resumes_history() {
    let _env = ENV_LOCK.lock().unwrap();
    if !node_available() {
        eprintln!("A3.4 пропущен: node недоступен");
        return;
    }
    let root = repo_root();
    if !root.join("sidecar/hds_agent/dist/worker.mjs").is_file() {
        eprintln!("A3.4 пропущен: нет dist/worker.mjs (npm run build)");
        return;
    }
    if hds_anonymize::discover_python(&root).is_none() {
        eprintln!("A3.4 пропущен: нет интерпретатора воркера анонимизации (venv)");
        return;
    }

    let stand = Stand::new("resume");
    stand.add_project();
    fs::create_dir_all(stand.workspace()).unwrap();
    let mock = Mock::start(
        &stand.dir.path,
        &[
            json!({ "text": "ЗАПОМНИЛ-ЭТО" }),
            json!({ "text": "продолжаем" }),
        ],
    );
    stand.write_secrets(&mock.base_url, "sk-a34-resume");

    std::env::set_var(
        "HDS_ANONYMIZE_WORKER",
        root.join("crates/hds-anonymize/tests/mock_worker.py"),
    );
    let opts = RunOptions {
        prompt: Some("первый вопрос".to_string()),
        max_iterations: 4,
        json: true,
        ..Default::default()
    };
    let plan = agent_run::plan_with(&stand.cfg, "P0001", &opts, &root).expect("план");
    let client = agent_run::open_client(&stand.cfg, &plan, &root).expect("воркер анонимизации");
    std::env::remove_var("HDS_ANONYMIZE_WORKER");

    let mut out = Vec::new();
    let mut err = Vec::new();
    let first = agent_run::run(&plan, Arc::clone(&client), &opts, &mut out, &mut err)
        .expect("первый прогон");
    assert!(
        first.ok,
        "первый ход: {first:?}; stderr={}",
        String::from_utf8_lossy(&err)
    );
    let session = first.session_id.clone().expect("session_id новой сессии");
    assert!(
        plan.state_dir
            .join(".cline")
            .join("data")
            .join("sessions")
            .join(&session)
            .is_dir(),
        "файлы сессии лежат в agent-state"
    );

    // Второй прогон — возобновление той же сессии с её историей.
    let resume = RunOptions {
        prompt: Some("второй вопрос".to_string()),
        session: Some(session.clone()),
        max_iterations: 4,
        json: true,
        ..Default::default()
    };
    let mut out2 = Vec::new();
    let mut err2 = Vec::new();
    let second = agent_run::run(&plan, Arc::clone(&client), &resume, &mut out2, &mut err2)
        .expect("возобновлённый прогон");
    assert!(
        second.ok,
        "возобновлённый ход: {second:?}; stderr={}",
        String::from_utf8_lossy(&err2)
    );
    assert_eq!(second.session_id.as_deref(), Some(session.as_str()));

    let events: Vec<Value> = String::from_utf8_lossy(&out2)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("NDJSON-событие"))
        .collect();
    let session_event = events
        .iter()
        .find(|event| event["kind"] == "session")
        .expect("событие session");
    assert_eq!(session_event["resumed"], true);
    assert_eq!(session_event["session_id"], session);

    // Первый запрос второго прогона уже содержит прошлую реплику.
    let requests: Vec<Value> = mock
        .raw()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    assert_eq!(requests.len(), 2, "по запросу на прогон");
    let resumed = requests[1].to_string();
    assert!(
        resumed.contains("ЗАПОМНИЛ-ЭТО"),
        "прошлая реплика в первом запросе после возобновления: {resumed}"
    );
    assert!(resumed.contains("второй вопрос"), "{resumed}");

    // Нет файла истории — понятная ошибка, а не пустая сессия.
    let missing = RunOptions {
        prompt: Some("третий вопрос".to_string()),
        session: Some("нет-такой-сессии".to_string()),
        max_iterations: 4,
        json: true,
        ..Default::default()
    };
    let mut out3 = Vec::new();
    let mut err3 = Vec::new();
    let failed = agent_run::run(&plan, Arc::clone(&client), &missing, &mut out3, &mut err3);
    let message = failed.expect_err("отсутствующая история — ошибка");
    assert!(message.contains("история сессии"), "{message}");

    if let Ok(client) = Arc::try_unwrap(client) {
        if let Ok(client) = client.into_inner() {
            client.shutdown();
        }
    }
}

/// A3.4: `hds agent history clear` — чистка истории через бинарь: значения
/// заменяются метками, рядом остаётся бэкап, печатаются число замен и пути
/// бэкапов; без `--session` берётся последняя сессия.
#[test]
fn history_clear_cli() {
    let _env = ENV_LOCK.lock().unwrap();
    let Some(envs) = anonymize_env() else {
        return;
    };
    let stand = Stand::new("history");
    stand.add_project();
    let old = seed_session(&stand, "S-OLD", "Иванов Иван");
    let new = seed_session(&stand, "S-NEW", "Иванов Иван");
    // S-OLD явно старее: без `--session` берётся последняя сессия.
    fs::OpenOptions::new()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();

    let out = run_cli_bin_env(
        &stand,
        &["agent", "history", "clear", "--project", "P0001"],
        &envs,
    );
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("S-NEW"), "последняя сессия: {stdout}");
    assert!(stdout.contains("замен"), "число замен: {stdout}");
    assert!(stdout.contains("[бэкап]"), "пути бэкапов: {stdout}");

    let cleaned = fs::read_to_string(&new).unwrap();
    assert!(
        !cleaned.contains("Иванов Иван"),
        "значение осталось: {cleaned}"
    );
    // Форма с граммемой приходит как `[PERSON_1|nomn]` (метки разбирает
    // `hds_core::labels`), поэтому проверяем префикс метки.
    assert!(cleaned.contains("[PERSON_1"), "метки нет: {cleaned}");
    assert!(
        new.with_extension("json.bak").is_file(),
        "бэкап рядом с файлом сессии"
    );
    assert!(
        fs::read_to_string(&old).unwrap().contains("Иванов Иван"),
        "не последняя сессия не тронута"
    );

    // Явный `--session`.
    let out = run_cli_bin_env(
        &stand,
        &[
            "agent",
            "history",
            "clear",
            "--project",
            "P0001",
            "--session",
            "S-OLD",
        ],
        &envs,
    );
    assert!(out.status.success(), "{out:?}");
    assert!(
        !fs::read_to_string(&old).unwrap().contains("Иванов Иван"),
        "история S-OLD очищена"
    );

    // Коды возврата: 1 — нет истории/сбой, 2 — использование.
    let missing = run_cli_bin_env(
        &stand,
        &[
            "agent",
            "history",
            "clear",
            "--project",
            "P0001",
            "--session",
            "НЕТ-ТАКОЙ",
        ],
        &envs,
    );
    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("не найден"),
        "понятная ошибка: {}",
        String::from_utf8_lossy(&missing.stderr)
    );
    assert_eq!(
        run_cli_bin_env(&stand, &["agent", "history", "clear"], &envs)
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run_cli_bin_env(&stand, &["agent", "history"], &envs)
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run_cli_bin_env(&stand, &["agent", "history", "bogus"], &envs)
            .status
            .code(),
        Some(2)
    );
    // Проект не зарегистрирован — код 1.
    assert_eq!(
        run_cli_bin_env(
            &stand,
            &["agent", "history", "clear", "--project", "P9999"],
            &envs
        )
        .status
        .code(),
        Some(1)
    );
}
