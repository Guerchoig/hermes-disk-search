//! Шлюз реплик и событий — посредник между `hds` и Node-воркером
//! (`PLAN_AGENT_PRIVACY_ZONE.md` §4.3, TR-5.1…TR-5.3, TR-6.4, TR-6.6).
//!
//! * **Реплика** (TR-5.1/5.2): текст пользователя анонимизируется Rust-стороной
//!   (`Client::anonymize_text`) до отправки воркеру; вызывающий получает
//!   `TextResult` с метками для предпросмотра.
//! * **События** (TR-5.3): каждое событие воркера деанонимизируется
//!   [`DeAnonymizer`] и отдаётся парой `raw` / `text`; пути `<workspace>`
//!   заменяются путями папки проекта.
//! * **Срабатывания фильтра** (TR-6.4): складываются в очередь и отдаются
//!   [`GatewayEvent::Redacted`] без значений — только метки и источник.
//! * **«Очистить историю»** (TR-6.6): переписывает файлы сессии SDK через
//!   словарь [`Filter`] с бэкапом.
//!
//! Встречные `host_call` воркера обрабатываются здесь же: `egress_check`
//! прогоняется через [`Filter`], `office_*` — через Python-клиент
//! `hds_anonymize`, `ask_user` — через [`AskUser`].
//!
//! Исходные значения живут только в памяти процесса: наружу уходят метки,
//! а в события — уже деанонимизированный текст. В журналы и конфиги значения
//! не попадают.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use hds_anonymize::client::Client;
use hds_anonymize::protocol::{OfficeResult, TextResult};

use crate::deanon::DeAnonymizer;
use crate::filter::{Filter, FilterError, Hit, Labeler, Point};
use crate::history::{self, HistoryError, HistoryOutcome};
use crate::paths::{check_within, GuardError};
use crate::protocol::{Down, EgressPoint, HostCall, Provider, PROTOCOL_VERSION};
use crate::worker::{HostHandler, Worker, WorkerError, WorkerSpec};

/// Вопрос пользователю через UI (TR-3.13: `host_call ask_user`).
///
/// Реализация блокирует вызывающий поток до ответа или таймаута. Вызывается
/// из потока чтения воркера, поэтому не должна держать блокировки [`Filter`].
pub trait AskUser: Send + Sync {
    /// Задать вопрос и вернуть ответ. `Err` — UI недоступен.
    fn ask(&self, question: &str) -> Result<Value, String>;
}

/// Заглушка: UI не подключён — любой вопрос получает отказ.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAskUser;

impl AskUser for NoAskUser {
    fn ask(&self, _question: &str) -> Result<Value, String> {
        Err("ask_user недоступен: UI не подключён".to_string())
    }
}

/// Событие шлюза для вызывающей стороны (UI, CLI, тесты).
#[derive(Debug, Clone, PartialEq)]
pub enum GatewayEvent {
    /// Готовность воркера: версия протокола и список инструментов модели.
    Ready { protocol: u32, tools: Vec<String> },
    /// Событие агента: `raw` — как пришло от воркера, `text` — после
    /// деанонимизации. Неизвестные метки перечислены в `unknown`.
    Agent {
        raw: Value,
        text: Value,
        unknown: Vec<String>,
    },
    /// Срабатывания словарного фильтра (TR-6.4): без значений, только метки.
    Redacted { hits: Vec<Hit> },
    /// Ход завершён (ответ на `send`/`start`).
    Done {
        ok: bool,
        result: Value,
        error: Option<String>,
    },
}

/// Ошибка шлюза.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    /// Транспорт воркера.
    Worker(String),
    /// Сбой Python-клиента анонимизации.
    Anonymize(String),
    /// Путь вне workspace или недопустимый путь.
    Path(String),
    /// Словарь исходящего недоступен (fail-closed, TR-6.5).
    Filter(String),
    /// История сессии.
    History(String),
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayError::Worker(m) => write!(f, "воркер: {m}"),
            GatewayError::Anonymize(m) => write!(f, "анонимизация: {m}"),
            GatewayError::Path(m) => write!(f, "путь: {m}"),
            GatewayError::Filter(m) => write!(f, "фильтр: {m}"),
            GatewayError::History(m) => write!(f, "история: {m}"),
        }
    }
}

impl std::error::Error for GatewayError {}

impl From<WorkerError> for GatewayError {
    fn from(e: WorkerError) -> Self {
        GatewayError::Worker(e.to_string())
    }
}

impl From<GuardError> for GatewayError {
    fn from(e: GuardError) -> Self {
        GatewayError::Path(e.reason())
    }
}

impl From<FilterError> for GatewayError {
    fn from(e: FilterError) -> Self {
        GatewayError::Filter(format!("{}: {}", e.code(), e.reason()))
    }
}

impl From<HistoryError> for GatewayError {
    fn from(e: HistoryError) -> Self {
        GatewayError::History(e.to_string())
    }
}

/// Выдаёт новые метки через Python-клиент (`label_for_value`, TR-6.2 б).
struct ClientLabeler<'a> {
    client: &'a mut Client,
    project: &'a str,
}

impl Labeler for ClientLabeler<'_> {
    fn label_for(&mut self, value: &str) -> Option<String> {
        self.client
            .label_for_value(self.project, value, "OTHER")
            .ok()
            .and_then(|r| r.label)
    }
}

/// Обработчик `host_call` воркера.
///
/// Живёт в `Arc` и вызывается из потока чтения, поэтому все разделяемые
/// состояния — под `Mutex`. Блокировки держатся коротко: `office_*` и
/// `anonymize_text` идут через один и тот же клиент, поэтому вызовы
/// сериализуются, но не пересекаются с `Gateway::send` (тот отпускает
/// блокировку до `worker.call`).
pub struct GatewayHost {
    project: String,
    workspace: PathBuf,
    filter: Arc<Filter>,
    /// Python-клиент анонимизации. `None` — шлюз без клиента: `egress_check`
    /// и очередь срабатываний работают, `office_*` отвечают ошибкой, новые
    /// метки для `others` не выдаются (тесты, egress-only режим).
    client: Option<Arc<Mutex<Client>>>,
    ask: Arc<dyn AskUser>,
    egress_log: Option<PathBuf>,
    redactions: Mutex<Vec<Hit>>,
}

impl GatewayHost {
    /// Собрать обработчик с Python-клиентом.
    pub fn new(
        project: impl Into<String>,
        workspace: impl Into<PathBuf>,
        filter: Arc<Filter>,
        client: Arc<Mutex<Client>>,
        ask: Arc<dyn AskUser>,
        egress_log: Option<PathBuf>,
    ) -> Self {
        Self::with_client(project, workspace, filter, Some(client), ask, egress_log)
    }

    /// Шлюз без Python-клиента: `egress_check` работает, `office_*` отвечают
    /// ошибкой, новые метки для `others` не выдаются.
    pub fn without_client(
        project: impl Into<String>,
        workspace: impl Into<PathBuf>,
        filter: Arc<Filter>,
        ask: Arc<dyn AskUser>,
        egress_log: Option<PathBuf>,
    ) -> Self {
        Self::with_client(project, workspace, filter, None, ask, egress_log)
    }

    fn with_client(
        project: impl Into<String>,
        workspace: impl Into<PathBuf>,
        filter: Arc<Filter>,
        client: Option<Arc<Mutex<Client>>>,
        ask: Arc<dyn AskUser>,
        egress_log: Option<PathBuf>,
    ) -> Self {
        GatewayHost {
            project: project.into(),
            workspace: workspace.into(),
            filter,
            client,
            ask,
            egress_log,
            redactions: Mutex::new(Vec::new()),
        }
    }

    /// Забрать накопленные срабатывания фильтра (TR-6.4).
    pub fn take_redactions(&self) -> Vec<Hit> {
        self.redactions
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default()
    }

    fn push_redactions(&self, hits: &[Hit]) {
        if hits.is_empty() {
            return;
        }
        if let Ok(mut q) = self.redactions.lock() {
            q.extend(hits.iter().cloned());
        }
    }

    fn office_read(&self, path: &str) -> Result<Value, String> {
        let real = check_within(&self.workspace, path).map_err(|e| e.reason())?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| "клиент анонимизации недоступен".to_string())?;
        let mut client = client
            .lock()
            .map_err(|_| "клиент анонимизации занят".to_string())?;
        let r = client
            .office_read(&real)
            .map_err(|e| format!("office_read: {e}"))?;
        Ok(office_json(&r))
    }

    fn office_edit(&self, path: &str, ops: &[Value]) -> Result<Value, String> {
        let real = check_within(&self.workspace, path).map_err(|e| e.reason())?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| "клиент анонимизации недоступен".to_string())?;
        let mut client = client
            .lock()
            .map_err(|_| "клиент анонимизации занят".to_string())?;
        let r = client
            .office_edit(&real, ops)
            .map_err(|e| format!("office_edit: {e}"))?;
        Ok(office_json(&r))
    }

    fn office_create(&self, path: &str, kind: &str, content: &Value) -> Result<Value, String> {
        let real = check_within(&self.workspace, path).map_err(|e| e.reason())?;
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| "клиент анонимизации недоступен".to_string())?;
        let mut client = client
            .lock()
            .map_err(|_| "клиент анонимизации занят".to_string())?;
        let r = client
            .office_create(&real, kind, content)
            .map_err(|e| format!("office_create: {e}"))?;
        Ok(office_json(&r))
    }

    fn egress(&self, point: EgressPoint, body: &Value) -> Result<Value, String> {
        let point = match point {
            EgressPoint::Model => Point::Model,
            EgressPoint::McpOut => Point::McpOut,
            EgressPoint::McpIn => Point::McpIn,
        };
        let outcome = match self.client.as_ref() {
            Some(client) => {
                let mut client = client
                    .lock()
                    .map_err(|_| "клиент анонимизации занят".to_string())?;
                let mut labeler = ClientLabeler {
                    client: &mut client,
                    project: &self.project,
                };
                self.filter.redact(point, body, &mut labeler)
            }
            None => {
                let mut labeler = crate::filter::NoLabeler;
                self.filter.redact(point, body, &mut labeler)
            }
        }
        .map_err(|e| format!("{}: {}", e.code(), e.reason()))?;
        if let Some(dir) = self.egress_log.as_ref() {
            write_egress(dir, &outcome.payload);
        }
        self.push_redactions(&outcome.hits);
        Ok(json!({
            "body": outcome.payload,
            "hits": outcome.hits.len(),
        }))
    }
}

fn office_json(r: &OfficeResult) -> Value {
    json!({
        "kind": r.kind,
        "markdown": r.markdown,
        "applied": r.applied,
    })
}

fn write_egress(dir: &Path, payload: &Value) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("{stamp}.jsonl"));
    if let Ok(mut line) = serde_json::to_string(payload) {
        line.push('\n');
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

impl HostHandler for GatewayHost {
    fn handle(&self, call: &str, payload: &Value) -> Result<Value, String> {
        match HostCall::parse(call, payload) {
            HostCall::EgressCheck { point, body } => self.egress(point, &body),
            HostCall::OfficeRead { path } => self.office_read(&path),
            HostCall::OfficeEdit { path, ops } => self.office_edit(&path, &ops),
            HostCall::OfficeCreate {
                path,
                kind,
                content,
            } => self.office_create(&path, &kind, &content),
            HostCall::AskUser { question } => self.ask.ask(&question),
            HostCall::Unknown { .. } => Err(format!("неизвестный host_call: {call}")),
        }
    }
}

/// Шлюз: воркер + обработчик + деанонимизатор + словарь.
pub struct Gateway {
    worker: Worker,
    host: Arc<GatewayHost>,
    project: String,
    workspace: PathBuf,
    state_dir: PathBuf,
    filter: Arc<Filter>,
    deanon: Arc<Mutex<DeAnonymizer>>,
    client: Arc<Mutex<Client>>,
    session_id: Option<String>,
}

impl Gateway {
    /// Запустить воркер и вернуть шлюз. Обработчик `host_call` собирается
    /// заранее и передаётся в [`Worker::spawn`].
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        spec: &WorkerSpec,
        host: Arc<GatewayHost>,
        project: impl Into<String>,
        workspace: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        filter: Arc<Filter>,
        deanon: Arc<Mutex<DeAnonymizer>>,
        client: Arc<Mutex<Client>>,
    ) -> Result<Gateway, GatewayError> {
        let worker = Worker::spawn(spec, Arc::clone(&host) as Arc<dyn HostHandler>)?;
        Ok(Gateway {
            worker,
            host,
            project: project.into(),
            workspace: workspace.into(),
            state_dir: state_dir.into(),
            filter,
            deanon,
            client,
            session_id: None,
        })
    }

    /// PID процесса воркера.
    pub fn pid(&self) -> u32 {
        self.worker.pid()
    }

    /// Признак закрытого канала.
    pub fn is_closed(&self) -> bool {
        self.worker.is_closed()
    }

    /// Обмен `hello` → `ready`. Возвращает список инструментов модели.
    pub fn hello(&mut self) -> Result<Vec<String>, GatewayError> {
        let up = self.worker.call(|id| Down::Hello {
            id: id.to_string(),
            protocol: PROTOCOL_VERSION,
        })?;
        match up {
            crate::protocol::Up::Ready { tools, .. } => Ok(tools),
            other => Err(GatewayError::Worker(format!(
                "ожидался ready, получено {}",
                other.kind()
            ))),
        }
    }

    /// Открыть сессию (`start`). Возвращает инструменты из ответа.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        provider: Provider,
        history: Vec<Value>,
        session_id: Option<String>,
        max_iterations: u32,
        subagents: bool,
    ) -> Result<Vec<String>, GatewayError> {
        let project = self.project.clone();
        let up = self.worker.call(|id| Down::Start {
            id: id.to_string(),
            project: project.clone(),
            provider: provider.clone(),
            history: history.clone(),
            session_id: session_id.clone(),
            max_iterations,
            subagents,
        })?;
        self.session_id = session_id;
        match up {
            crate::protocol::Up::Ready { tools, .. } => Ok(tools),
            crate::protocol::Up::Done {
                ok, result, error, ..
            } => {
                if !ok {
                    return Err(GatewayError::Worker(
                        error.unwrap_or_else(|| "start завершился ошибкой".to_string()),
                    ));
                }
                Ok(result
                    .get("tools")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default())
            }
            other => Err(GatewayError::Worker(format!(
                "ожидался ready/done, получено {}",
                other.kind()
            ))),
        }
    }

    /// Анонимизировать реплику и отправить её агенту (TR-5.1).
    ///
    /// Блокировка клиента отпускается до `worker.call`, иначе встречный
    /// `host_call egress_check` из потока чтения ждал бы тот же мьютекс.
    pub fn send(&mut self, text: &str) -> Result<TextResult, GatewayError> {
        let project = self.project.clone();
        let anon = {
            let mut client = self
                .client
                .lock()
                .map_err(|_| GatewayError::Anonymize("клиент занят".to_string()))?;
            client
                .anonymize_text(&project, text)
                .map_err(|e| GatewayError::Anonymize(e.to_string()))?
        };
        let clean = anon.text.clone();
        self.worker.call(|id| Down::Send {
            id: id.to_string(),
            text: clean,
        })?;
        Ok(anon)
    }

    /// Прервать текущий ход.
    pub fn abort(&self) -> Result<(), GatewayError> {
        self.worker
            .write(&Down::Abort {
                id: "abort".to_string(),
            })
            .map_err(GatewayError::from)
    }

    /// Мягкая остановка (сессия сохраняется).
    pub fn stop(&self) -> Result<(), GatewayError> {
        self.worker
            .write(&Down::Stop {
                id: "stop".to_string(),
            })
            .map_err(GatewayError::from)
    }

    /// Следующее событие шлюза. Сначала отдаёт накопленные срабатывания
    /// фильтра, затем — событие воркера.
    pub fn next_event(&self, timeout: Duration) -> Result<GatewayEvent, GatewayError> {
        let hits = self.host.take_redactions();
        if !hits.is_empty() {
            return Ok(GatewayEvent::Redacted { hits });
        }
        match self.worker.next_event(timeout) {
            Ok(up) => Ok(self.map_up(up)),
            Err(WorkerError::Timeout(_)) => {
                let hits = self.host.take_redactions();
                if hits.is_empty() {
                    Err(GatewayError::Worker("таймаут события".to_string()))
                } else {
                    Ok(GatewayEvent::Redacted { hits })
                }
            }
            Err(e) => Err(GatewayError::from(e)),
        }
    }

    fn map_up(&self, up: crate::protocol::Up) -> GatewayEvent {
        use crate::protocol::Up;
        match up {
            Up::Ready {
                protocol, tools, ..
            } => GatewayEvent::Ready { protocol, tools },
            Up::Event { event, .. } => {
                let text = self
                    .deanon
                    .lock()
                    .map(|d| d.value(&event))
                    .unwrap_or_else(|_| event.clone());
                let unknown = collect_unknown(&event);
                GatewayEvent::Agent {
                    raw: event,
                    text,
                    unknown,
                }
            }
            Up::Done {
                ok, result, error, ..
            } => GatewayEvent::Done { ok, result, error },
            Up::HostCall { call, .. } => GatewayEvent::Agent {
                raw: json!({"host_call": call}),
                text: json!({"host_call": call}),
                unknown: Vec::new(),
            },
            Up::Error { message, .. } => GatewayEvent::Done {
                ok: false,
                result: Value::Null,
                error: Some(message),
            },
        }
    }

    /// Пересобрать словарь и деанонимизатор по снимку vault (TR-6.5).
    ///
    /// Вызывается при смене `vault_rev` и по таймауту `max_stale_sec`.
    pub fn refresh(
        &self,
        system: &[String],
        projects: &[String],
        secrets: &[String],
        project_root: &Path,
    ) -> Result<i64, GatewayError> {
        let snapshot = {
            let mut client = self
                .client
                .lock()
                .map_err(|_| GatewayError::Anonymize("клиент занят".to_string()))?;
            client
                .snapshot(&self.project)
                .map_err(|e| GatewayError::Anonymize(e.to_string()))?
        };
        let rev = snapshot.rev;
        self.filter
            .set_snapshot(&snapshot, system, projects, secrets);
        // Фильтр нумерует `[SYS_n]` по цепочке `system` + `projects`, поэтому
        // деанонимизатор получает тот же список в том же порядке.
        let sys_all = crate::filter::sys_values(system, projects);
        if let Ok(mut d) = self.deanon.lock() {
            *d = DeAnonymizer::new(&snapshot, &sys_all, &self.workspace, project_root);
        }
        Ok(rev)
    }

    /// «Очистить историю» (TR-6.6): переписать файлы сессии через словарь.
    pub fn clear_history(&self) -> Result<HistoryOutcome, GatewayError> {
        let session = self
            .session_id
            .as_deref()
            .ok_or_else(|| GatewayError::History("сессия не открыта".to_string()))?;
        history::clear(&self.state_dir, session, &self.filter).map_err(GatewayError::from)
    }

    /// Корректно завершить воркер.
    pub fn shutdown(self) {
        self.worker.shutdown();
    }
}

fn collect_unknown(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_unknown_into(value, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_unknown_into(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            // Пользователю показываются исходные метки (`[PERSON_1]`), а не
            // FTS-форма: неизвестная метка остаётся в тексте как есть.
            out.extend(hds_core::labels::find_labels(s));
        }
        Value::Array(items) => {
            for item in items {
                collect_unknown_into(item, out);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_unknown_into(item, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Source;

    #[test]
    fn unknown_labels_are_collected() {
        let v = json!({
            "text": "видел [PERSON_1] и [ORG_2|род]",
            "nested": ["[SYS_1]", 7]
        });
        let u = collect_unknown(&v);
        assert_eq!(
            u,
            vec![
                "[ORG_2|род]".to_string(),
                "[PERSON_1]".to_string(),
                "[SYS_1]".to_string()
            ]
        );
    }

    #[test]
    fn no_ask_user_refuses() {
        let ask = NoAskUser;
        assert!(ask.ask("кто ты?").is_err());
    }

    #[test]
    fn egress_writes_redacted_payload() {
        let dir = std::env::temp_dir().join(format!("hds-gw-egress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_egress(&dir, &json!({"messages": [{"content": "[PERSON_1]"}]}));
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(files[0].path()).unwrap();
        assert!(text.contains("[PERSON_1]"));
        assert!(!text.contains("Иванов"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redaction_queue_is_drained() {
        let filter = Arc::new(Filter::new(
            "p1",
            Vec::new(),
            crate::settings::UnknownPolicy::Label,
            Duration::from_secs(60),
            None,
        ));
        // Клиент не нужен: очередь наполняется напрямую через push_redactions.
        let host = GatewayHost::without_client(
            "p1",
            std::env::temp_dir(),
            filter,
            Arc::new(NoAskUser),
            None,
        );
        host.push_redactions(&[Hit {
            label: "[PERSON_1]".to_string(),
            source: Source::Other,
        }]);
        let taken = host.take_redactions();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].label, "[PERSON_1]");
        assert!(host.take_redactions().is_empty());
    }
}
