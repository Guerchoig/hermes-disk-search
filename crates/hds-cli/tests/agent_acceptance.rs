//! A3.6 — приёмка A3 (e2e, mock-провайдер): агент читает образ через
//! `office_read`, ищет через `disk-search` (живой `hds mcp --http`, маршрут
//! `/mcp/agent/P0001`), вызывает заглушку `tavily` (MCP stdio), пишет файл в
//! workspace. Инварианты I2 (ни одна известная форма значения не уходит
//! провайдеру и во внешний MCP) и I7 (ключ провайдера в тексте блокирует ход)
//! проверяются на заглушках; профиль `%USERPROFILE%\.cline` до/после не
//! меняется. Плюс `run --session` (прошлая реплика модели) и «Очистить историю»
//! на копии `agent-state` (стенд и есть копия).
//!
//! Воркер анонимизации по умолчанию — `mock_worker.py` (быстро, без NER).
//! `HDS_A36_REAL_ANON=1` переключает на настоящий venv (`sidecar\venv-anonymize`)
//! и CPU-профиль: тогда проверяется, что `office_read` разобрал настоящий docx
//! («Пояснительная записка») — приёмочный прогон с настоящим Python-воркером.
//!
//! Предпосылки: `node`, собранный `dist/worker.mjs`, интерпретатор воркера
//! анонимизации и образ `tools/parity/fixtures/записка_ртк_полная.docx`.
//! Нет — тест печатает причину и выходит (как остальные интеграционные тесты).

mod common;

use std::fs;
use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use hds_agent::projects::{self, Project};
use hds_cli::cmd::agent_run::{self, RunOptions};
use hds_core::access::token_sha256_hex;
use hds_core::config::load_from;

/// Тесты трогают env (`HDS_ROOT`, `HDS_CONFIG`, `HDS_ANONYMIZE_*`,
/// `HDS_EXTRACT_PYTHON`) — идут последовательно и восстанавливают значения.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Значение из снимка vault (mock-воркер: `[PERSON_1]` ↔ это значение).
const PERSON: &str = "Иванов Иван";
/// Ключ провайдера: не должен появляться ни в одном файле, кроме `secrets.json`.
const PROVIDER_KEY: &str = "sk-a36-provider-key";
/// Токен проекта для маршрута `/mcp/agent/P0001` (plain; в файле — и хэш тоже).
const PROJECT_TOKEN: &str = "a36-project-token-0123456789abcdef";
/// Фраза из настоящего docx (приёмка с настоящим Python-воркером).
const DOCX_PHRASE: &str = "Пояснительная записка";
/// Финальный ответ модели — по нему проверяется возобновление сессии.
const FINAL_TEXT: &str = "ИТОГ-A36: образ прочитан, поиск и tavily отработали.";

fn counter() -> u32 {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    SEQ.fetch_add(1, Ordering::SeqCst)
}

/// Корень поставки без verbatim-префикса (Node его не переваривает, §4 п.13).
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

/// Снимок каталога: `(путь, длина, mtime)` для каждого файла — сравнение
/// профиля `%USERPROFILE%\.cline` до и после прогона.
fn snapshot_dir(dir: &Path) -> Vec<(String, u64, u128)> {
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
                continue;
            }
            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            out.push((path.to_string_lossy().to_string(), meta.len(), mtime));
        }
    }
    out.sort();
    out
}

/// Стенд: временный корень (`HDS_ROOT`), зона агента, проект и индекс.
struct Stand {
    dir: common::TempDir,
    cfg: hds_core::config::Config,
    project_dir: PathBuf,
    zone: PathBuf,
    port: u16,
    real_anon: bool,
}

impl Stand {
    fn new(tag: &str) -> Stand {
        let suffix = format!("{}-{}", std::process::id(), counter());
        let dir = common::TempDir::new(&format!("agent-accept-{tag}-{suffix}"));
        let project_dir = dir.join("Проект");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join("договор.md"), "договор поставки").unwrap();
        let zone = dir.join("zone");
        fs::create_dir_all(&zone).unwrap();

        // Порт MCP-сервера: свободный, но зафиксированный в конфиге.
        let port = {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            listener.local_addr().unwrap().port()
        };

        let real_anon = std::env::var("HDS_A36_REAL_ANON").is_ok_and(|v| v == "1");
        let cfg_text = format!(
            "agent:\n  enabled: true\n  root: '{}'\n  mcp:\n    tavily:\n      command: '{}'\n\
             \x20   onec_buddy:\n      enabled: false\n\
             db_path: '{}'\n\
             embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n  batch_size: 1\n\
             index:\n  clip: false\n\
             anonymize:\n  output_dir: '{}'\n  db_path: '{}'\n  gpu: cpu\n\
             mcp_http:\n  host: 127.0.0.1\n  port: {}\n  path: /mcp\n  start_timeout: 30\n",
            slash(&zone),
            slash(&repo_root().join("crates/hds-cli/tests/fixtures/tavily_stub.mjs")),
            slash(&dir.join("index.db")),
            slash(&dir.join("data/anonymized")),
            slash(&dir.join("data/anonymize.db")),
            port,
        );
        let cfg_path = dir.write_config(&cfg_text);
        std::env::set_var("HDS_CONFIG", &cfg_path);
        std::env::set_var("HDS_ROOT", &dir.path);
        std::env::remove_var("HDS_AGENT_SECRETS");
        let cfg = load_from(&cfg_path).expect("config.yaml");

        // Воркер извлечения/лемматизации ищется в `<root>/sidecar` — стенд
        // держит копию нужных модулей (в поставке их кладёт build_sidecar.ps1).
        copy_python_sidecar(&dir.path);
        let venv_python = repo_root()
            .join("sidecar")
            .join("venv-anonymize")
            .join("Scripts")
            .join("python.exe");
        if venv_python.is_file() {
            std::env::set_var("HDS_EXTRACT_PYTHON", &venv_python);
            std::env::set_var("HDS_ANONYMIZE_PYTHON", &venv_python);
            if real_anon {
                std::env::remove_var("HDS_ANONYMIZE_WORKER");
            } else {
                std::env::set_var(
                    "HDS_ANONYMIZE_WORKER",
                    repo_root().join("crates/hds-anonymize/tests/mock_worker.py"),
                );
            }
        }

        Stand {
            dir,
            cfg,
            project_dir,
            zone,
            port,
            real_anon,
        }
    }

    fn workspace(&self) -> PathBuf {
        self.zone.join("P0001").join("workspace")
    }

    fn state_dir(&self) -> PathBuf {
        self.zone.join("P0001").join("agent-state")
    }

    fn config_dir(&self) -> PathBuf {
        self.zone.join("P0001").join("agent-config")
    }

    /// Проект в реестре + файлы workspace + строка в индексе.
    fn prepare(&self, with_docx: bool) {
        fs::create_dir_all(self.workspace()).unwrap();
        if with_docx {
            let docx = repo_root().join("tools/parity/fixtures/записка_ртк_полная.docx");
            fs::copy(&docx, self.workspace().join("записка.docx"))
                .unwrap_or_else(|e| panic!("копия docx {}: {e}", docx.display()));
        }
        let note = self.workspace().join("договор.md");
        fs::write(
            &note,
            format!("Условия договора: срок 30 дней, контакт {PERSON}, сумма 1 200 000 руб."),
        )
        .unwrap();
        seed_index(
            &self.dir.join("index.db"),
            &note,
            "условия договора срок 30 дней контакт иванов иван сумма 1 200 000",
        );

        projects::add(
            &self.cfg,
            Project {
                id: "P0001".to_string(),
                name: "Стенд A3.6".to_string(),
                path: self.project_dir.to_string_lossy().to_string(),
                provider: "openai-compatible".to_string(),
                model: "mock-model".to_string(),
                created: 0,
            },
        )
        .expect("проект в реестре");
        self.write_secrets("http://127.0.0.1:9/v1");
    }

    /// `secrets.json`: провайдер, токен проекта (plain + хэш), ключ tavily.
    ///
    /// Ключ tavily несёт путь журнала заглушки (`tvly-a36-stand|<путь>`):
    /// `McpSpec` передаёт в env только `TAVILY_API_KEY`, а стенду нужно видеть,
    /// что именно получила внешняя сторона (`mcp_out`).
    fn write_secrets(&self, base_url: &str) {
        let path = self.dir.join("data").join("agent").join("secrets.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stub_log = format!("tvly-a36-stand|{}", slash(&self.stub_log()));
        let secrets = json!({
            "projects": {
                "P0001": {
                    "token": PROJECT_TOKEN,
                    "token_sha256": token_sha256_hex(PROJECT_TOKEN),
                    "expires": 0,
                    "provider": "openai-compatible",
                    "model": "mock-model"
                }
            },
            "providers": {
                "openai-compatible": { "api_key": PROVIDER_KEY, "base_url": base_url }
            },
            "mcp": { "tavily": { "api_key": stub_log } }
        });
        fs::write(&path, serde_json::to_string_pretty(&secrets).unwrap()).unwrap();
    }

    /// Журнал вызовов заглушки `tavily` (что получила внешняя сторона).
    fn stub_log(&self) -> PathBuf {
        self.dir.join("tavily-stub.jsonl")
    }

    fn plan(&self, opts: &RunOptions) -> agent_run::RunPlan {
        agent_run::plan_with(&self.cfg, "P0001", opts, &repo_root()).expect("план сессии")
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        for name in [
            "HDS_ROOT",
            "HDS_CONFIG",
            "HDS_AGENT_SECRETS",
            "HDS_ANONYMIZE_WORKER",
            "HDS_ANONYMIZE_PYTHON",
            "HDS_EXTRACT_PYTHON",
        ] {
            std::env::remove_var(name);
        }
    }
}

/// Копия python-части воркера извлечения в корень стенда (как в поставке).
fn copy_python_sidecar(root: &Path) {
    let repo = repo_root();
    let dst_extract = root.join("sidecar").join("hds_extract");
    fs::create_dir_all(&dst_extract).unwrap();
    fs::copy(
        repo.join("sidecar/hds_extract/worker.py"),
        dst_extract.join("worker.py"),
    )
    .expect("worker.py");
    for (src_rel, dst_rel) in [
        ("sidecar/hds_sidecar", "sidecar/hds_sidecar"),
        ("hds", "hds"),
    ] {
        let dst = root.join(dst_rel);
        fs::create_dir_all(&dst).unwrap();
        for entry in fs::read_dir(repo.join(src_rel)).unwrap() {
            let entry = entry.unwrap();
            if entry.path().extension().is_some_and(|e| e == "py") {
                fs::copy(entry.path(), dst.join(entry.file_name())).unwrap();
            }
        }
    }
}

/// Строка индекса: файл + один чанк (FTS; векторы не нужны — стенд без эмбеддера).
fn seed_index(db: &Path, path: &Path, chunk: &str) {
    let conn = hds_core::db::connect(db, 4).expect("index.db");
    let file = hds_core::db::upsert_file(
        &conn,
        &path.to_string_lossy(),
        "md",
        "text",
        64,
        1.0,
        Some("hash-a36"),
        Some("P0001"),
    )
    .expect("файл в индексе");
    hds_core::db::add_chunk(&conn, file, 0, Some(1), None, None, chunk, chunk).expect("чанк");
    hds_core::db::finish_file(&conn, file, "indexed", None, Some(1), 1.0).expect("finish");
}

/// Mock-провайдер (OpenAI-совместимый) на `127.0.0.1:0`; тела запросов — в файл.
struct MockProvider {
    child: Child,
    base_url: String,
    log: PathBuf,
}

impl MockProvider {
    fn start(dir: &Path, script: &[Value]) -> MockProvider {
        use std::io::BufRead;
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
        MockProvider {
            child,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            log,
        }
    }

    /// Тела запросов к модели (уже после фильтра `model`).
    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
    }

    fn raw(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for MockProvider {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Живой `hds mcp --http` (тот же сервер, которым управляет `hds mcp-http`).
struct McpServer {
    child: Child,
    host: String,
    port: u16,
}

impl McpServer {
    fn start(stand: &Stand) -> McpServer {
        let child = Command::new(env!("CARGO_BIN_EXE_hds"))
            .args([
                "mcp",
                "--http",
                "--host",
                "127.0.0.1",
                "--port",
                &stand.port.to_string(),
                "--path",
                "/mcp",
            ])
            .env("HDS_CONFIG", stand.dir.path.join("config.yaml"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("hds mcp --http");
        let server = McpServer {
            child,
            host: "127.0.0.1".to_string(),
            port: stand.port,
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if server.health() {
                return server;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("MCP-сервер не поднялся на 127.0.0.1:{}/mcp", stand.port);
    }

    fn health(&self) -> bool {
        hds_core::http::request(
            &self.host,
            self.port,
            "GET",
            "/health",
            &[("Accept", "application/json")],
            None,
            Duration::from_secs(2),
        )
        .map(|r| r.status == 200)
        .unwrap_or(false)
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Сырой POST JSON-RPC на маршрут (диагностика: сервер отвечает до агента).
fn rpc_call(port: u16, path: &str, token: &str, method: &str, params: Value) -> (u16, String) {
    use std::io::Read;
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect MCP");
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Authorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write MCP");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    let status = resp
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = resp
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// Настройки MCP, которые записал `prepare`: маршрут A2 и заглушка tavily.
fn mcp_settings(stand: &Stand) -> Value {
    let path = hds_agent::mcp::settings_path(&stand.config_dir());
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("MCP-настройки {}: {e}", path.display()));
    serde_json::from_str(&text).expect("JSON MCP-настроек")
}

/// Ход агента с готовым планом (им обёрнут `hds agent run`).
fn run_turn(
    plan: &agent_run::RunPlan,
    client: &Arc<std::sync::Mutex<hds_anonymize::Client>>,
    opts: &RunOptions,
) -> (Result<agent_run::TurnReport, String>, String, String) {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = agent_run::run(plan, Arc::clone(client), opts, &mut out, &mut err);
    (
        result,
        String::from_utf8_lossy(&out).to_string(),
        String::from_utf8_lossy(&err).to_string(),
    )
}

/// Дописать в файл истории сообщение со значением (сохраняя форму файла).
fn seed_value(path: &Path, value: &str) {
    let text = fs::read_to_string(path).expect("история сессии");
    let mut root: Value = serde_json::from_str(&text).expect("JSON истории");
    let message = json!({ "role": "user", "content": [{ "type": "text", "text": value }] });
    match &mut root {
        Value::Array(items) => items.push(message),
        Value::Object(map) => {
            let items = map.entry("messages").or_insert_with(|| json!([]));
            items
                .as_array_mut()
                .expect("messages — массив")
                .push(message);
        }
        other => panic!("неожиданный формат истории: {other}"),
    }
    fs::write(path, serde_json::to_string_pretty(&root).unwrap()).expect("запись истории");
}

/// A3.6: полный ход через CLI на стенде из A2 — office_read, disk-search через
/// живой `hds mcp --http`, заглушка `tavily`, запись файла; I2/I7, профиль,
/// возобновление сессии и «Очистить историю».
#[test]
fn acceptance_a3_end_to_end() {
    let _env = ENV_LOCK.lock().unwrap();
    if !node_available() {
        eprintln!("A3.6 пропущен: node недоступен");
        return;
    }
    let root = repo_root();
    if !root.join("sidecar/hds_agent/dist/worker.mjs").is_file() {
        eprintln!("A3.6 пропущен: нет dist/worker.mjs (npm run build)");
        return;
    }
    if !root
        .join("tools/parity/fixtures/записка_ртк_полная.docx")
        .is_file()
    {
        eprintln!("A3.6 пропущен: нет образа tools/parity/fixtures/записка_ртк_полная.docx");
        return;
    }
    if hds_anonymize::discover_python(&root).is_none() {
        eprintln!("A3.6 пропущен: нет интерпретатора воркера анонимизации (venv)");
        return;
    }

    let stand = Stand::new("e2e");
    stand.prepare(true);

    // Профиль владельца до прогона: агент работает в подменённом HOME.
    let profile = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    let profile_before = profile
        .as_deref()
        .map(|p| snapshot_dir(&p.join(".cline")))
        .unwrap_or_default();

    // Модель-заглушка: образ → поиск → tavily → запись файла → ответ.
    let mock = MockProvider::start(
        &stand.dir.path,
        &[
            json!({ "toolCalls": [{ "name": "office_read", "arguments": { "path": "записка.docx" } }] }),
            json!({ "toolCalls": [{ "name": "disk-search__search_local_files",
                "arguments": { "query": "условия договора", "limit": 5 } }] }),
            json!({ "toolCalls": [{ "name": "tavily__search",
                "arguments": { "query": format!("стоимость 1С ERP, подписант {PERSON}") } }] }),
            json!({ "toolCalls": [{ "name": "editor",
                "arguments": { "path": "ответ.md", "new_text": format!("ИТОГ: образ прочитан, контрагент {PERSON}") } }] }),
            json!({ "text": FINAL_TEXT }),
        ],
    );
    stand.write_secrets(&mock.base_url);

    // Живой MCP-сервер и маршрут проекта отвечают ещё до агента.
    let _server = McpServer::start(&stand);
    let (status, body) = rpc_call(
        stand.port,
        "/mcp/agent/P0001",
        PROJECT_TOKEN,
        "tools/list",
        json!({}),
    );
    assert_eq!(status, 200, "маршрут MCP: {body}");
    assert!(body.contains("search_local_files"), "{body}");
    assert!(!body.contains("ask_my_files"), "{body}");

    // --- 1. полный ход -----------------------------------------------------
    let opts = RunOptions {
        prompt: Some(format!(
            "прочитай записку и найди условия договора по {PERSON}"
        )),
        max_iterations: 8,
        json: true,
        ..Default::default()
    };
    let plan = stand.plan(&opts);
    let client = agent_run::open_client(&stand.cfg, &plan, &root).expect("воркер анонимизации");
    // Настоящий воркер нумерует метки в пустом vault: заводим значение заранее,
    // чтобы снимок фильтра знал его форму (как после первого прогона по
    // документам проекта). Mock-воркер отдаёт `[PERSON_1]` из своего снимка.
    if stand.real_anon {
        let mut locked = client.lock().expect("клиент анонимизации");
        locked
            .label_for_value("P0001", PERSON, "PERSON")
            .expect("метка для значения в настоящем vault");
    }
    let (first, stdout, stderr) = run_turn(&plan, &client, &opts);
    let report = first.unwrap_or_else(|e| panic!("ход: {e}; stderr={stderr}"));
    assert!(
        report.ok,
        "ход не завершён: {report:?}; stderr={stderr}; события={stdout}"
    );
    assert_eq!(report.reason, "completed");
    assert!(stdout.contains("\"kind\":\"turn_end\""), "{stdout}");
    let session = report.session_id.clone().expect("session_id");

    // Инструменты: образ, поиск, внешний MCP, запись файла; запрещённых нет.
    for expected in [
        "office_read",
        "disk-search__search_local_files",
        "tavily__search",
        "editor",
        "ask_user",
    ] {
        assert!(
            report.tools.iter().any(|t| t == expected),
            "{expected} в фактическом списке: {:?}",
            report.tools
        );
    }
    for banned in [
        "run_commands",
        "fetch_web_content",
        "ask_my_files",
        "tavily__ask_my_files",
        "disk-search__ask_my_files",
    ] {
        assert!(
            !report.tools.iter().any(|t| t == banned),
            "{banned} не должен попадать к модели: {:?}",
            report.tools
        );
    }

    // Файл записан в workspace агента.
    let written = stand.workspace().join("ответ.md");
    let written_text =
        fs::read_to_string(&written).unwrap_or_else(|e| panic!("файл {}: {e}", written.display()));
    assert!(
        written_text.contains(PERSON),
        "файл в workspace: {written_text}"
    );

    // I2: провайдер не получил ни одной известной формы значения и ключ.
    let all = mock.raw();
    assert!(!all.contains(PERSON), "I2: значение ушло провайдеру: {all}");
    assert!(!all.contains(PROVIDER_KEY), "ключ провайдера в запросе");
    assert!(
        all.contains("[PERSON_"),
        "метка должна быть в запросах: {all}"
    );
    // office_read: mock-воркер отвечает "[1] mock"; настоящий — текстом docx.
    if stand.real_anon {
        assert!(
            all.contains(DOCX_PHRASE),
            "настоящий воркер должен вернуть текст docx: {all}"
        );
    } else {
        assert!(
            all.contains("[1] mock"),
            "office_read дошёл до модели: {all}"
        );
    }
    // Поиск вернул фрагмент своего проекта (через живой маршрут A2).
    assert!(
        all.contains("договор.md"),
        "search_local_files вернул фрагмент: {all}"
    );

    // mcp_out: внешняя сторона получила метку, а не значение; ответ заглушки
    // (в нём значение есть) дошёл до модели уже меткой.
    let stub_log = fs::read_to_string(stand.stub_log())
        .unwrap_or_else(|e| panic!("журнал заглушки tavily: {e}"));
    assert!(stub_log.contains("search"), "заглушка вызвана: {stub_log}");
    assert!(
        !stub_log.contains(PERSON),
        "I2/mcp_out: значение ушло во внешний MCP: {stub_log}"
    );
    assert!(
        stub_log.contains("[PERSON_"),
        "mcp_out обязан вернуть метку: {stub_log}"
    );
    assert!(
        all.contains("TAVILY-STUB"),
        "результат tavily дошёл до модели: {all}"
    );

    // Ключ провайдера не осел ни в одном файле проекта, кроме secrets.json.
    for (path, text) in files_under(&plan.config_dir)
        .into_iter()
        .chain(files_under(&plan.state_dir))
    {
        assert!(
            !text.contains(PROVIDER_KEY),
            "ключ провайдера в {}",
            path.display()
        );
    }

    // Настройки MCP: маршрут A2 с токеном проекта и заглушка `tavily`.
    let mcp = mcp_settings(&stand);
    let mcp_url = mcp["mcpServers"]["disk-search"]["transport"]["url"]
        .as_str()
        .unwrap_or_default();
    assert!(
        mcp_url.ends_with("/mcp/agent/P0001"),
        "маршрут A2: {mcp_url}"
    );
    assert_eq!(
        mcp["mcpServers"]["disk-search"]["transport"]["headers"]["Authorization"],
        json!(format!("Bearer {PROJECT_TOKEN}"))
    );
    assert!(
        mcp["mcpServers"]["tavily"]["transport"]["command"].is_string(),
        "заглушка tavily в настройках: {mcp}"
    );
    assert!(
        mcp["mcpServers"].get("1c-buddy").is_none(),
        "1c-buddy выключен в стенде: {mcp}"
    );

    // --- 2. I7: ключ провайдера в реплике — ход блокируется ----------------
    let before = mock.requests().len();
    let secret_opts = RunOptions {
        prompt: Some(format!("вот ключ {PROVIDER_KEY}, проверь")),
        max_iterations: 4,
        json: true,
        ..Default::default()
    };
    let (blocked, _, _) = run_turn(&plan, &client, &secret_opts);
    let blocked = blocked.expect("ход с ключом завершается событием, не ошибкой CLI");
    assert!(
        !blocked.ok,
        "ход с ключом обязан быть неуспешным: {blocked:?}"
    );
    let message = blocked.error.clone().unwrap_or_default();
    assert!(
        message.contains("privacy_filter_unavailable"),
        "I7: ожидался fail-closed, получено {blocked:?}"
    );
    assert_eq!(
        mock.requests().len(),
        before,
        "I7: при ключе в тексте провайдер не должен получать запрос"
    );

    // --- 3. возобновление: `run --session` ---------------------------------
    let resume_opts = RunOptions {
        prompt: Some("повтори итог".to_string()),
        session: Some(session.clone()),
        max_iterations: 4,
        json: true,
        ..Default::default()
    };
    let (resumed, resumed_out, resumed_err) = run_turn(&plan, &client, &resume_opts);
    let resumed = resumed.unwrap_or_else(|e| panic!("возобновление: {e}; {resumed_err}"));
    assert!(resumed.ok, "возобновлённый ход: {resumed:?}");
    assert!(resumed_out.contains("\"resumed\":true"), "{resumed_out}");
    let requests = mock.requests();
    let last = requests.last().expect("запрос к модели").to_string();
    assert!(
        last.contains(FINAL_TEXT),
        "прошлая реплика в первом запросе после возобновления: {last}"
    );
    assert!(!last.contains(PERSON), "I2 после возобновления: {last}");

    // --- 4. «Очистить историю» на копии agent-state ------------------------
    let messages = hds_agent::history::session_dir(&stand.state_dir(), &session)
        .join(format!("{session}.messages.json"));
    assert!(messages.is_file(), "файл истории: {}", messages.display());
    seed_value(&messages, PERSON);
    let (cleared, outcome) = agent_run::clear_history(&stand.cfg, "P0001", Some(&session), &root)
        .expect("очистка истории");
    assert_eq!(cleared, session);
    assert!(outcome.replaced >= 1, "замен нет: {outcome:?}");
    assert!(!outcome.backups.is_empty(), "бэкапы: {outcome:?}");
    let cleaned = fs::read_to_string(&messages).unwrap();
    assert!(
        !cleaned.contains(PERSON),
        "значение осталось в истории: {cleaned}"
    );
    assert!(cleaned.contains("[PERSON_"), "метка в истории: {cleaned}");
    for backup in &outcome.backups {
        assert!(backup.is_file(), "бэкап: {}", backup.display());
    }

    // --- 5. профиль владельца не изменился ---------------------------------
    let profile_after = profile
        .as_deref()
        .map(|p| snapshot_dir(&p.join(".cline")))
        .unwrap_or_default();
    assert_eq!(
        profile_before.len(),
        profile_after.len(),
        "профиль .cline изменился по составу файлов"
    );
    assert_eq!(profile_before, profile_after, "профиль .cline изменился");

    // Завершение воркера анонимизации.
    if let Ok(client) = Arc::try_unwrap(client) {
        if let Ok(client) = client.into_inner() {
            client.shutdown();
        }
    }
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
