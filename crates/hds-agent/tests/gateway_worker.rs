//! A3.2 — Rust ↔ **настоящий** Node-воркер (`sidecar/hds_agent/dist/worker.mjs`).
//!
//! Проверяется проводка, которую нельзя проверить на заглушке: боевой [`Gateway`]
//! поверх живого процесса, **настоящий** [`Filter`] в `egress_check`, деанонимизация
//! событий и `turn_end`. Снимок vault собирается вручную (Python не нужен):
//! [`Gateway::spawn_egress_only`] + [`GatewayHost::without_client`].
//!
//! Предпосылки: `node` в PATH (или `HDS_NODE`) и собранный `dist/worker.mjs`.
//! Если чего-то нет — тест пишет причину и выходит (как тесты `worker.rs`).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use hds_agent::deanon::DeAnonymizer;
use hds_agent::env;
use hds_agent::filter::{sys_values, Filter, Hit, Source};
use hds_agent::gateway::{Gateway, GatewayError, GatewayEvent, GatewayHost, NoAskUser};
use hds_agent::protocol::Provider;
use hds_agent::rules;
use hds_agent::settings::UnknownPolicy;
use hds_agent::worker::WorkerSpec;

use hds_anonymize::protocol::{Snapshot, SnapshotForm, SnapshotLabel};

/// Собранный воркер (`sidecar/hds_agent/dist/worker.mjs`).
///
/// `canonicalize` на Windows возвращает verbatim-путь `\\?\C:\…`, который Node
/// не переваривает (`realpathSync` падает на `lstat 'C:'`) — префикс снимаем.
fn worker_script() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sidecar")
        .join("hds_agent")
        .join("dist")
        .join("worker.mjs");
    let resolved = joined.canonicalize().unwrap_or(joined);
    let text = resolved.to_string_lossy().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => resolved,
    }
}

/// Интерпретатор Node: `HDS_NODE` или `node` из PATH.
fn node_program() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HDS_NODE") {
        return Some(PathBuf::from(path));
    }
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()
        .filter(|status| status.success())
        .map(|_| PathBuf::from("node"))
}

/// `(node, dist/worker.mjs)` или `None` с причиной в stderr.
fn prerequisites() -> Option<(PathBuf, PathBuf)> {
    let script = worker_script();
    if !script.is_file() {
        eprintln!(
            "A3.2 пропущен: нет {} (собрать: cd sidecar/hds_agent && npm run build)",
            script.display()
        );
        return None;
    }
    let Some(node) = node_program() else {
        eprintln!("A3.2 пропущен: node не найден (PATH или HDS_NODE)");
        return None;
    };
    Some((node, script))
}

/// Каталоги проекта P0001 и окружение процесса (как в `env::build`).
struct Project {
    base: PathBuf,
    root: PathBuf,
    workspace: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl Project {
    fn new() -> Project {
        // Тесты идут параллельно в одном процессе: каталог должен быть свой.
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let base = std::env::temp_dir().join(format!("hds-agent-a32-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("P0001");
        let workspace = root.join("workspace");
        let config = root.join("agent-config");
        let state = root.join("agent-state");
        env::prepare_dirs(&config, &state, &workspace).expect("каталоги проекта");
        Project {
            base,
            root,
            workspace,
            config,
            state,
        }
    }

    fn spec(&self, node: &Path, script: &Path) -> WorkerSpec {
        let config = serde_yaml::Value::Mapping(Default::default());
        let child = env::build(&config, &self.config, &self.state, &self.workspace, None);
        WorkerSpec {
            program: node.to_path_buf(),
            script: script.to_path_buf(),
            cwd: child.cwd.clone(),
            env: child.vars.clone(),
            stderr_log: Some(self.base.join("worker-stderr.log")),
            timeout: Duration::from_secs(120),
        }
    }
}

/// Mock-провайдер на Node: OpenAI-совместимый, слушает `127.0.0.1:0`,
/// печатает `PORT <n>` и складывает **тела запросов** в файл (что реально
/// ушло «к модели»). Шаг сценария: `{toolCalls: [...]}` или `{text}`.
const MOCK_JS: &str = r#"
import http from 'node:http';
import fs from 'node:fs';

const log = process.env.MOCK_REQUESTS;
const script = JSON.parse(process.env.MOCK_SCRIPT);
const usage = { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 };
let seq = 0;

function callsOf(step, n) {
  return (step.toolCalls ?? []).map((call, i) => ({
    index: i,
    id: `call_${n}_${i}`,
    type: 'function',
    function: { name: call.name, arguments: JSON.stringify(call.arguments ?? {}) },
  }));
}

function replyJson(res, step, n) {
  const calls = callsOf(step, n);
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(JSON.stringify({
    id: `chatcmpl-${n}`,
    object: 'chat.completion',
    created: 0,
    model: 'mock-model',
    choices: [{
      index: 0,
      finish_reason: calls.length ? 'tool_calls' : 'stop',
      message: {
        role: 'assistant',
        content: step.text ?? '',
        ...(calls.length ? { tool_calls: calls.map(({ index, ...rest }) => rest) } : {}),
      },
    }],
    usage,
  }));
}

function replyStream(res, step, n) {
  const calls = callsOf(step, n);
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
  const base = { id: `chatcmpl-${n}`, object: 'chat.completion.chunk', created: 0, model: 'mock-model' };
  const send = (chunk) => res.write(`data: ${typeof chunk === 'string' ? chunk : JSON.stringify(chunk)}\n\n`);
  send({ ...base, choices: [{ index: 0, delta: { role: 'assistant' }, finish_reason: null }] });
  for (const piece of String(step.text ?? '').match(/[\s\S]{1,12}/g) ?? []) {
    send({ ...base, choices: [{ index: 0, delta: { content: piece }, finish_reason: null }] });
  }
  for (const call of calls) {
    send({ ...base, choices: [{ index: 0, delta: { tool_calls: [call] }, finish_reason: null }] });
  }
  send({ ...base, choices: [{ index: 0, delta: {}, finish_reason: calls.length ? 'tool_calls' : 'stop' }], usage });
  send('[DONE]');
  res.end();
}

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on('data', (chunk) => chunks.push(chunk));
  req.on('end', () => {
    const raw = Buffer.concat(chunks).toString('utf8');
    fs.appendFileSync(log, `${raw}\n`);
    let body = {};
    try { body = JSON.parse(raw); } catch { body = {}; }
    seq += 1;
    const step = script[Math.min(seq - 1, script.length - 1)] ?? { text: 'ok' };
    if (body.stream === true) replyStream(res, step, seq);
    else replyJson(res, step, seq);
  });
});

server.listen(0, '127.0.0.1', () => {
  process.stdout.write(`PORT ${server.address().port}\n`);
});
"#;

struct Mock {
    child: Child,
    base_url: String,
    log: PathBuf,
}

impl Mock {
    fn start(base: &Path, script: &[Value]) -> Mock {
        let file = base.join("mock_provider.mjs");
        fs::write(&file, MOCK_JS).expect("mock_provider.mjs");
        let log = base.join("mock-requests.jsonl");
        let _ = fs::remove_file(&log);
        let mut child = Command::new(node_program().unwrap_or_else(|| PathBuf::from("node")))
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
        let port = line
            .trim()
            .strip_prefix("PORT ")
            .unwrap_or_else(|| panic!("mock не сообщил порт: {line:?}"))
            .parse::<u16>()
            .expect("порт");
        Mock {
            child,
            base_url: format!("http://127.0.0.1:{port}"),
            log,
        }
    }

    /// Тела запросов, как их получил mock (уже после фильтра).
    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .collect()
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

/// Имена инструментов в теле запроса к модели (`<server>__<tool>` и встроенные).
fn model_tools(request: &Value) -> Vec<String> {
    request
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| {
                    t.get("function")
                        .and_then(|f| f.get("name"))
                        .or_else(|| t.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Снимок vault без Python: метка `[PERSON_1]` с именительной и дательной
/// формами (так его отдаёт `Client::snapshot`).
fn snapshot() -> Snapshot {
    Snapshot {
        rev: 7,
        labels: vec![SnapshotLabel {
            label: "[PERSON_1]".to_string(),
            entity_type: "PERSON".to_string(),
            forms: vec![
                SnapshotForm {
                    value: "Иванов Иван".to_string(),
                    gram: None,
                    origin: "ner".to_string(),
                },
                SnapshotForm {
                    value: "Иванову Ивану".to_string(),
                    gram: Some("дат".to_string()),
                    origin: "ner".to_string(),
                },
            ],
        }],
        others: Vec::new(),
        keep: Vec::new(),
    }
}

/// A3.2: `Gateway::spawn` → `hello` → `open` → `send` на настоящем воркере.
///
/// Проверяется: `declared_tools` из ответа `start`; `egress_check` проходит
/// **настоящий** `Filter` (снимок собран без Python) — модель видит метки, а не
/// значения; события деанонимизированы (`[PERSON_1]` → значение, `[SYS_n]` →
/// системная строка, путь workspace → путь проекта); `turn_end` приходит как
/// [`GatewayEvent::TurnEnd`] с фактическим списком инструментов; `Redacted` не
/// содержит значений.
#[test]
fn gateway_hello_open_send_on_real_worker() {
    let Some((node, worker_script)) = prerequisites() else {
        return;
    };
    let project = Project::new();
    fs::write(
        project.workspace.join("allowed.txt"),
        "ALLOWED-42 Иванов Иван",
    )
    .expect("allowed.txt");
    rules::write_rules(&project.config.join("rules")).expect("rules");

    // Снимок и словарь — без Python (как `Client::snapshot`, TR-1.6).
    let snapshot = snapshot();
    let system = vec!["DESKTOP-A32".to_string()];
    let projects: Vec<String> = Vec::new();
    let secrets: Vec<String> = Vec::new();
    let filter = Arc::new(Filter::new(
        "P0001",
        Vec::new(),
        UnknownPolicy::Label,
        Duration::from_secs(300),
        None,
    ));
    filter.set_snapshot(&snapshot, &system, &projects, &secrets);
    let deanon = Arc::new(Mutex::new(DeAnonymizer::new(
        &snapshot,
        &sys_values(&system, &projects),
        &project.workspace,
        &project.root,
    )));

    let file_path = project.workspace.join("allowed.txt");
    let final_text = format!(
        "Готово: [PERSON_1] на [SYS_1], файл {}",
        file_path.display()
    );
    let mock = Mock::start(
        &project.base,
        &[
            json!({"toolCalls": [{"name": "read_files", "arguments": {"files": [{"path": "allowed.txt"}]}}]}),
            json!({ "text": final_text }),
        ],
    );
    let provider = Provider {
        kind: "openai-compatible".to_string(),
        base_url: format!("{}/v1", mock.base_url),
        model: "mock-model".to_string(),
        api_key: "sk-mock-a32".to_string(),
    };

    let host = Arc::new(GatewayHost::without_client(
        "P0001",
        &project.workspace,
        Arc::clone(&filter),
        Arc::new(NoAskUser),
        None,
    ));
    let spec = project.spec(&node, &worker_script);
    let mut gateway = Gateway::spawn_egress_only(
        &spec,
        host,
        "P0001",
        &project.workspace,
        &project.state,
        Arc::clone(&filter),
        deanon,
    )
    .expect("шлюз поверх настоящего воркера");
    assert!(gateway.is_egress_only());
    assert!(
        matches!(gateway.send("без клиента"), Err(GatewayError::Anonymize(_))),
        "send без Python-клиента обязан отказать (fail-closed)"
    );

    let greeting = gateway.hello().expect("hello");
    assert!(greeting.is_empty(), "hello: {greeting:?}");

    let declared = gateway
        .open(provider, Vec::new(), None, 4, false)
        .expect("start на настоящем воркере");
    assert_eq!(
        declared,
        vec![
            "read_files",
            "search_codebase",
            "editor",
            "office_read",
            "office_edit",
            "office_create",
            "ask_user",
        ],
        "declared_tools из ответа start (A3.1 переименовал поле)"
    );

    gateway
        .send_anonymized("прочитай allowed.txt")
        .expect("send");

    // События до turn_end: Redacted без значений, Agent с raw/text, TurnEnd.
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut hits: Vec<Hit> = Vec::new();
    let mut agents: Vec<(Value, Value)> = Vec::new();
    let mut turn = None;
    while turn.is_none() {
        assert!(Instant::now() < deadline, "turn_end не пришёл за 90 с");
        match gateway
            .next_event(Duration::from_secs(30))
            .expect("событие")
        {
            GatewayEvent::Redacted { hits: batch } => hits.extend(batch),
            GatewayEvent::Agent { raw, text, .. } => agents.push((raw, text)),
            GatewayEvent::TurnEnd {
                ok,
                reason,
                text,
                error,
                tools,
                raw,
            } => turn = Some((ok, reason, text, error, tools, raw)),
            GatewayEvent::Ready { .. } | GatewayEvent::Done { .. } => {}
        }
    }
    gateway.shutdown();

    let workspace = project.workspace.display().to_string();
    let project_root = project.root.display().to_string();
    let (ok, reason, text, error, tools, raw) = turn.expect("turn_end");
    assert!(ok, "ход не завершился успешно: {reason} / {error:?}");
    assert_eq!(reason, "completed");
    assert!(
        text.contains("Иванов Иван"),
        "ответ деанонимизирован: {text}"
    );
    assert!(text.contains("DESKTOP-A32"), "[SYS_1] развёрнута: {text}");
    assert!(
        text.contains(&project_root),
        "путь проекта в ответе: {text}"
    );
    assert!(
        !text.contains(&workspace),
        "путь workspace остался в ответе: {text}"
    );

    let raw_text = raw.get("text").and_then(Value::as_str).unwrap_or_default();
    assert!(
        raw_text.contains("[PERSON_1]"),
        "raw хранит метку: {raw_text}"
    );
    assert!(
        !raw_text.contains("Иванов Иван"),
        "raw не должен содержать значение: {raw_text}"
    );
    for expected in ["read_files", "office_read", "ask_user"] {
        assert!(
            tools.iter().any(|t| t == expected),
            "{expected} в фактическом списке: {tools:?}"
        );
    }
    assert!(
        !tools.iter().any(|t| t == "run_commands"),
        "run_commands выключен политикой: {tools:?}"
    );

    // События агента: raw с меткой — text с исходным значением.
    let labelled = agents
        .iter()
        .find(|(raw, _)| raw.to_string().contains("[PERSON_1]"));
    let (_, agent_text) = labelled.expect("событие агента с меткой [PERSON_1]");
    assert!(
        agent_text.to_string().contains("Иванов Иван"),
        "событие деанонимизировано: {agent_text}"
    );

    // Фильтр сработал на теле запроса с содержимым файла: только метки.
    assert!(
        !hits.is_empty(),
        "фильтр должен был сработать на теле запроса с содержимым allowed.txt"
    );
    let person: Vec<&Hit> = hits.iter().filter(|h| h.label == "[PERSON_1]").collect();
    assert!(!person.is_empty(), "нет метки [PERSON_1]: {hits:?}");
    assert!(
        person.iter().any(|h| h.source == Source::Project),
        "источник срабатывания — project: {hits:?}"
    );
    for hit in &hits {
        let debug = format!("{hit:?}");
        assert!(!debug.contains("Иванов"), "Hit со значением: {debug}");
    }

    // Что увидела «модель»: только метки и только разрешённые инструменты.
    let requests = mock.requests();
    assert!(
        requests.len() >= 2,
        "два запроса к модели (инструмент + ответ): {}",
        requests.len()
    );
    let log = mock.raw();
    assert!(log.contains("[PERSON_1]"), "метка ушла провайдеру");
    assert!(!log.contains("Иванов Иван"), "значение ушло провайдеру");
    let first_tools = model_tools(&requests[0]);
    assert!(
        first_tools.iter().any(|t| t == "read_files"),
        "read_files в запросе: {first_tools:?}"
    );
    assert!(
        !first_tools.iter().any(|t| t == "run_commands"),
        "run_commands нет в запросе: {first_tools:?}"
    );

    drop(mock);
    let _ = fs::remove_dir_all(&project.base);
}

/// TR-6.5 на живом воркере: у Rust нет снимка vault → `egress_check` отвечает
/// ошибкой, провайдер не получает **ни одного** запроса, а ход завершается
/// `TurnEnd { ok: false }` с `privacy_filter_unavailable`; процесс жив.
#[test]
fn gateway_fail_closed_without_snapshot() {
    let Some((node, worker_script)) = prerequisites() else {
        return;
    };
    let project = Project::new();
    fs::write(project.workspace.join("allowed.txt"), "ALLOWED-42").expect("allowed.txt");

    // Снимок не задан: словарь пуст, фильтр обязан отказать (fail-closed).
    let filter = Arc::new(Filter::new(
        "P0001",
        Vec::new(),
        UnknownPolicy::Label,
        Duration::from_secs(300),
        None,
    ));
    let deanon = Arc::new(Mutex::new(DeAnonymizer::new(
        &Snapshot::default(),
        &[],
        &project.workspace,
        &project.root,
    )));
    let mock = Mock::start(&project.base, &[json!({ "text": "ответа быть не должно" })]);
    let provider = Provider {
        kind: "openai-compatible".to_string(),
        base_url: format!("{}/v1", mock.base_url),
        model: "mock-model".to_string(),
        api_key: "sk-mock-a32".to_string(),
    };
    let host = Arc::new(GatewayHost::without_client(
        "P0001",
        &project.workspace,
        Arc::clone(&filter),
        Arc::new(NoAskUser),
        None,
    ));
    let spec = project.spec(&node, &worker_script);
    let mut gateway = Gateway::spawn_egress_only(
        &spec,
        host,
        "P0001",
        &project.workspace,
        &project.state,
        Arc::clone(&filter),
        deanon,
    )
    .expect("шлюз");
    gateway.hello().expect("hello");
    gateway
        .open(provider, Vec::new(), None, 4, false)
        .expect("start");
    gateway.send_anonymized("привет").expect("send");

    let deadline = Instant::now() + Duration::from_secs(90);
    let mut turn = None;
    while turn.is_none() {
        assert!(Instant::now() < deadline, "turn_end не пришёл за 90 с");
        match gateway
            .next_event(Duration::from_secs(30))
            .expect("событие")
        {
            GatewayEvent::TurnEnd {
                ok,
                reason,
                text,
                error,
                tools,
                raw,
            } => turn = Some((ok, reason, text, error, tools, raw)),
            GatewayEvent::Redacted { .. }
            | GatewayEvent::Agent { .. }
            | GatewayEvent::Ready { .. }
            | GatewayEvent::Done { .. } => {}
        }
    }
    let alive = !gateway.is_closed();
    gateway.shutdown();

    let (ok, reason, text, error, _, _) = turn.expect("turn_end");
    assert!(!ok, "ход без снимка не может быть успешным");
    assert_eq!(reason, "error");
    assert!(text.is_empty(), "текста быть не должно: {text}");
    let error = error.unwrap_or_default();
    assert!(
        error.contains("privacy_filter_unavailable"),
        "причина — недоступный фильтр: {error}"
    );
    assert!(alive, "воркер переживает отказ фильтра");
    assert!(
        mock.requests().is_empty(),
        "провайдер не должен был получить ни одного запроса"
    );

    drop(mock);
    let _ = fs::remove_dir_all(&project.base);
}
