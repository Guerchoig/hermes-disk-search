//! A3.4 — жизненный цикл (TR-3.9) на **настоящем** `dist/worker.mjs` с
//! mock-провайдером.
//!
//! Что проверяется:
//! * реестр [`AgentRegistry`] держит два проекта одновременно (`max_live = 2`),
//!   повторный `get_or_open` не перезапускает процесс;
//! * возобновление после «перезапуска hds»: `start` с тем же `session_id` **и**
//!   `initialMessages` из `history::read_messages` — первый запрос к модели
//!   содержит прошлую реплику; `session_id` без истории её не грузит;
//! * «Очистить историю» end-to-end: `Gateway::clear_history` переписывает файлы
//!   сессии (бэкапы `.json.bak`), значения в них больше нет, метка на месте;
//!   следующий `start` берёт очищенную историю — модель видит метку.
//!
//! Предпосылки те же, что у `gateway_worker.rs`: `node` в PATH (или `HDS_NODE`)
//! и собранный `dist/worker.mjs`; нет — тест печатает причину и выходит.

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
use hds_agent::filter::{sys_values, Filter};
use hds_agent::gateway::{Gateway, GatewayEvent, GatewayHost, NoAskUser};
use hds_agent::history;
use hds_agent::protocol::Provider;
use hds_agent::registry::AgentRegistry;
use hds_agent::settings::UnknownPolicy;
use hds_agent::worker::WorkerSpec;

use hds_anonymize::protocol::{Snapshot, SnapshotForm, SnapshotLabel};

/// Собранный воркер без verbatim-префикса `\\?\` (§4 п.13): Node его не
/// переваривает (`realpathSync` падает на `lstat 'C:'`).
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
            "A3.4 пропущен: нет {} (собрать: cd sidecar/hds_agent && npm run build)",
            script.display()
        );
        return None;
    }
    let Some(node) = node_program() else {
        eprintln!("A3.4 пропущен: node не найден (PATH или HDS_NODE)");
        return None;
    };
    Some((node, script))
}

/// Каталоги проекта и окружение процесса (как в `env::build`).
struct Project {
    base: PathBuf,
    root: PathBuf,
    workspace: PathBuf,
    config: PathBuf,
    state: PathBuf,
}

impl Project {
    fn new(tag: &str) -> Project {
        // Тесты идут параллельно в одном процессе: каталог должен быть свой.
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let base =
            std::env::temp_dir().join(format!("hds-agent-a34-{tag}-{}-{seq}", std::process::id()));
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
        let child = env::build(&self.config, &self.state, &self.workspace, None);
        WorkerSpec {
            program: node.to_path_buf(),
            script: script.to_path_buf(),
            cwd: child.cwd.clone(),
            env: child.vars.clone(),
            stderr_log: Some(self.base.join("worker-stderr.log")),
            timeout: Duration::from_secs(120),
        }
    }

    /// Файл истории сессии (`.messages.json`).
    fn messages_file(&self, session: &str) -> PathBuf {
        history::session_dir(&self.state, session).join(format!("{session}.messages.json"))
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// Mock-провайдер на Node (OpenAI-совместимый, `127.0.0.1:0`): печатает
/// `PORT <n>` и складывает **тела запросов** в файл — что реально ушло
/// «к модели». Шаг сценария: `{toolCalls: [...]}` или `{text}`.
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

    fn provider(&self) -> Provider {
        Provider {
            kind: "openai-compatible".to_string(),
            base_url: format!("{}/v1", self.base_url),
            model: "mock-model".to_string(),
            api_key: "sk-mock-a34".to_string(),
        }
    }

    /// Тела запросов, как их получил mock (уже после фильтра).
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

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Снимок vault без Python: метка `[PERSON_1]` с двумя формами (как отдаёт
/// `Client::snapshot`). Значение живёт только в памяти теста.
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

/// Шлюз проекта: `hello` + `start` (с историей и/или `session_id`, если заданы).
fn open_gateway(
    project: &Project,
    node: &Path,
    script: &Path,
    provider: Provider,
    session: Option<&str>,
    history: Vec<Value>,
) -> Gateway {
    let snapshot = snapshot();
    let filter = Arc::new(Filter::new(
        "P0001",
        Vec::new(),
        UnknownPolicy::Label,
        Duration::from_secs(300),
        None,
    ));
    filter.set_snapshot(&snapshot, &[], &[], &[]);
    let deanon = Arc::new(Mutex::new(DeAnonymizer::new(
        &snapshot,
        &sys_values(&[], &[]),
        &project.workspace,
        &project.root,
    )));
    let host = Arc::new(GatewayHost::without_client(
        "P0001",
        &project.workspace,
        Arc::clone(&filter),
        Arc::new(NoAskUser),
        None,
    ));
    let mut gateway = Gateway::spawn_egress_only(
        &project.spec(node, script),
        host,
        "P0001",
        &project.workspace,
        &project.state,
        filter,
        deanon,
    )
    .expect("шлюз поверх настоящего воркера");
    gateway.hello().expect("hello");
    gateway
        .open(provider, history, session.map(str::to_string), 4, false)
        .expect("start на настоящем воркере");
    gateway
}

/// Провести ход и вернуть `turn_end`: `(ok, reason, text, error)`.
fn run_turn(gateway: &mut Gateway, text: &str) -> (bool, String, String, Option<String>) {
    gateway.send_anonymized(text).expect("send");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
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
                ..
            } => return (ok, reason, text, error),
            GatewayEvent::Redacted { .. }
            | GatewayEvent::Agent { .. }
            | GatewayEvent::Ready { .. }
            | GatewayEvent::Done { .. } => {}
        }
    }
}

/// Реестр на настоящем воркере: два проекта живы (`max_live = 2`), повторный
/// `get_or_open` переиспользует процесс (PID тот же, второго `start` нет),
/// переход на третий проект выгружает LRU.
#[test]
fn registry_reuses_and_evicts_on_real_worker() {
    let Some((node, script)) = prerequisites() else {
        return;
    };
    let project = Project::new("registry");
    fs::write(
        project.workspace.join("allowed.txt"),
        "ALLOWED-42 Иванов Иван",
    )
    .expect("allowed.txt");
    let mock = Mock::start(&project.base, &[json!({ "text": "ответ" })]);
    let provider = mock.provider();

    let mut registry = AgentRegistry::new(2, Duration::from_secs(1800));
    let t0 = Instant::now();
    let first_pid = registry
        .get_or_open("P0001", t0, || {
            Ok(open_gateway(
                &project,
                &node,
                &script,
                provider.clone(),
                None,
                Vec::new(),
            ))
        })
        .expect("первый проект")
        .pid();
    let second_pid = registry
        .get_or_open("P0002", t0 + Duration::from_secs(1), || {
            Ok(open_gateway(
                &project,
                &node,
                &script,
                provider.clone(),
                None,
                Vec::new(),
            ))
        })
        .expect("второй проект")
        .pid();
    assert_ne!(first_pid, second_pid, "на каждый проект свой процесс");
    assert_eq!(
        registry.projects(),
        vec!["P0001".to_string(), "P0002".to_string()],
        "второй проект не выгрузил первый"
    );

    // Ход на первом проекте после открытия второго — процесс тот же.
    let (ok, reason, _, error) = {
        let gateway = registry.get_mut("P0001").expect("P0001 жив");
        assert_eq!(gateway.pid(), first_pid);
        run_turn(gateway, "сколько стоит ALLOWED-42?")
    };
    assert!(ok, "ход не завершён: {reason} / {error:?}");

    // Повторный `get_or_open` процесса не запускает (spawn не вызывается).
    let calls = std::cell::Cell::new(0);
    let same_pid = registry
        .get_or_open("P0001", t0 + Duration::from_secs(2), || {
            calls.set(calls.get() + 1);
            Ok(open_gateway(
                &project,
                &node,
                &script,
                provider.clone(),
                None,
                Vec::new(),
            ))
        })
        .expect("переиспользование")
        .pid();
    assert_eq!(calls.get(), 0, "spawn при переиспользовании не вызывается");
    assert_eq!(same_pid, first_pid, "процесс не перезапускался");

    // Третий проект при max_live = 2 выгружает LRU: P0001 использован позже
    // P0002 — значит жертвой становится P0002.
    registry.touch("P0001", t0 + Duration::from_secs(3));
    let third_pid = registry
        .get_or_open("P0003", t0 + Duration::from_secs(4), || {
            Ok(open_gateway(
                &project,
                &node,
                &script,
                provider.clone(),
                None,
                Vec::new(),
            ))
        })
        .expect("третий проект")
        .pid();
    assert_eq!(
        registry.projects(),
        vec!["P0001".to_string(), "P0003".to_string()],
        "выгружен LRU-проект"
    );
    assert_ne!(third_pid, second_pid);

    // Реплика и содержимое файла ушли модели метками.
    assert!(mock.raw().contains("[PERSON_1]"), "метка ушла провайдеру");
    assert!(
        !mock.raw().contains("Иванов Иван"),
        "значение не должно уходить провайдеру"
    );
}

/// Возобновление после «перезапуска hds»: `session_id` + `initialMessages`
/// дают модели прошлую переписку, один `session_id` — нет (A0 §3.6).
#[test]
fn resume_after_restart_uses_initial_messages() {
    let Some((node, script)) = prerequisites() else {
        return;
    };
    let project = Project::new("resume");
    let mock = Mock::start(
        &project.base,
        &[
            json!({ "text": "ЗАПОМНИЛ-ЭТО" }),
            json!({ "text": "без истории" }),
            json!({ "text": "продолжаем" }),
        ],
    );
    let provider = mock.provider();

    // Первый запуск hds: новая сессия и один ход.
    let session = {
        let mut gateway =
            open_gateway(&project, &node, &script, provider.clone(), None, Vec::new());
        let session = gateway
            .session_id()
            .expect("воркер вернул session_id новой сессии")
            .to_string();
        let (ok, reason, _, error) = run_turn(&mut gateway, "первый вопрос");
        assert!(ok, "первый ход не завершён: {reason} / {error:?}");
        gateway.stop().expect("stop");
        gateway.shutdown();
        session
    };
    assert_eq!(mock.requests().len(), 1, "один запрос к модели за ход");

    // История легла в файлы сессии — её и читает `history::read_messages`.
    let messages = history::read_messages(&project.state, &session).expect("файл истории");
    assert!(!messages.is_empty(), "история сессии пуста");
    assert!(project.messages_file(&session).is_file());

    // Перезапуск без `initialMessages`: историю SDK не грузит.
    {
        let mut fresh = open_gateway(
            &project,
            &node,
            &script,
            provider.clone(),
            Some(&session),
            Vec::new(),
        );
        let (ok, reason, _, error) = run_turn(&mut fresh, "вопрос без истории");
        assert!(ok, "ход без истории не завершён: {reason} / {error:?}");
        fresh.shutdown();
    }
    let without = mock.requests()[1].to_string();
    assert!(
        !without.contains("ЗАПОМНИЛ-ЭТО"),
        "session_id без initialMessages историю не грузит: {without}"
    );

    // Возобновление: тот же session_id плюс история из файлов.
    {
        let mut resumed = open_gateway(
            &project,
            &node,
            &script,
            provider.clone(),
            Some(&session),
            messages,
        );
        let (ok, reason, _, error) = run_turn(&mut resumed, "второй вопрос");
        assert!(ok, "возобновлённый ход не завершён: {reason} / {error:?}");
        resumed.shutdown();
    }
    let resumed = mock.requests()[2].to_string();
    assert!(
        resumed.contains("ЗАПОМНИЛ-ЭТО"),
        "прошлая реплика в первом запросе после перезапуска: {resumed}"
    );
    assert!(
        resumed.contains("второй вопрос"),
        "новая реплика: {resumed}"
    );
}

/// Дописать в файл истории сообщение со значением (сохраняя форму файла).
///
/// Значение в истории — модель угрозы TR-6.6: SDK мог сохранить текст до
/// фильтра или из результата инструмента. Тест кладёт его явно, чтобы проверить
/// саму очистку, а не догадку о том, что именно пишет SDK.
fn seed_value(path: &Path, value: &str) {
    let text = fs::read_to_string(path).expect("история сессии");
    let mut root: Value = serde_json::from_str(&text).expect("JSON истории");
    let message = json!({
        "role": "user",
        "content": [{ "type": "text", "text": value }],
    });
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

/// «Очистить историю» end-to-end: `Gateway::clear_history` переписывает файлы
/// сессии (бэкапы `.json.bak`), значения в них больше нет, метка на месте;
/// следующий `start` берёт **очищенную** историю — модель видит метку.
#[test]
fn clear_history_end_to_end() {
    let Some((node, script)) = prerequisites() else {
        return;
    };
    let project = Project::new("clear");
    let mock = Mock::start(
        &project.base,
        &[json!({ "text": "ответ-1" }), json!({ "text": "ответ-2" })],
    );
    let provider = mock.provider();

    // Сессия и ход: файлы истории появляются на диске.
    let mut gateway = open_gateway(&project, &node, &script, provider.clone(), None, Vec::new());
    let session = gateway
        .session_id()
        .expect("session_id новой сессии")
        .to_string();
    let (ok, reason, _, error) = run_turn(&mut gateway, "первый вопрос");
    assert!(ok, "ход не завершён: {reason} / {error:?}");
    gateway.stop().expect("stop");

    let file = project.messages_file(&session);
    assert!(file.is_file(), "файл истории: {}", file.display());
    seed_value(&file, "Иванов Иван");

    let outcome = gateway.clear_history().expect("очистка истории");
    gateway.shutdown();
    assert!(outcome.replaced >= 1, "замен не было: {outcome:?}");
    assert!(!outcome.files.is_empty(), "файлы перезаписаны: {outcome:?}");
    assert!(!outcome.backups.is_empty(), "бэкапы: {outcome:?}");

    let cleaned = fs::read_to_string(&file).expect("очищенный файл");
    assert!(
        !cleaned.contains("Иванов Иван"),
        "значение осталось в истории: {cleaned}"
    );
    assert!(
        cleaned.contains("[PERSON_1]"),
        "метка не появилась: {cleaned}"
    );
    for backup in &outcome.backups {
        assert!(backup.is_file(), "бэкап на месте: {}", backup.display());
        assert!(
            backup.to_string_lossy().ends_with(".json.bak"),
            "бэкап с расширением .json.bak: {}",
            backup.display()
        );
    }
    // Бэкап сообщений хранит исходное содержимое (манифест значения не содержал).
    let messages_backup = outcome
        .backups
        .iter()
        .find(|b| b.to_string_lossy().contains(".messages.json.bak"))
        .expect("бэкап файла сообщений");
    assert!(
        fs::read_to_string(messages_backup)
            .unwrap()
            .contains("Иванов Иван"),
        "бэкап хранит исходное содержимое: {}",
        messages_backup.display()
    );

    // Перезапуск с очищенной историей: в первом запросе к модели — метка.
    let history = history::read_messages(&project.state, &session).expect("очищенная история");
    assert!(!history.is_empty(), "очищенная история пуста");
    {
        let mut resumed = open_gateway(
            &project,
            &node,
            &script,
            provider.clone(),
            Some(&session),
            history,
        );
        let (ok, reason, _, error) = run_turn(&mut resumed, "второй вопрос");
        assert!(ok, "ход после очистки не завершён: {reason} / {error:?}");
        resumed.shutdown();
    }
    let requests = mock.requests();
    let last = requests.last().expect("запрос к модели").to_string();
    assert!(
        last.contains("[PERSON_1]"),
        "метка в первом запросе после очистки: {last}"
    );
    assert!(
        !last.contains("Иванов Иван"),
        "значение ушло модели после очистки: {last}"
    );
}
