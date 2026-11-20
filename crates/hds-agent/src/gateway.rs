//! Шлюз реплик и событий — посредник между `hds` и Node-воркером
//! (`PLAN_AGENT_PRIVACY_ZONE.md` §4.3, TR-5.1…TR-5.3, TR-6.4, TR-6.6).
//!
//! * **Реплика** (TR-5.1/5.2): текст пользователя анонимизируется Rust-стороной
//!   (`Client::anonymize_text`) до отправки воркеру; вызывающий получает
//!   `TextResult` с метками для предпросмотра.
//! * **События** (TR-5.3): каждое событие воркера деанонимизируется
//!   [`DeAnonymizer`] и отдаётся парой `raw` / `text`; пути `<workspace>`
//!   заменяются путями папки проекта. Завершение хода (`turn_end`, A3.1)
//!   приходит отдельным [`GatewayEvent::TurnEnd`] — с проверкой «успех при
//!   пустом ответе» и фактическим списком инструментов.
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

use hds_anonymize::protocol::Snapshot;

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
    /// Ход завершён (`turn_end` воркера, A3.1). `text` — деанонимизированный
    /// финальный ответ, `tools` — **фактический** список инструментов запроса
    /// к модели (из `beforeModel`), `raw` — событие как пришло от воркера.
    ///
    /// `ok` проверен здесь же: «успех» с пустым текстом и без usage — сбой
    /// (грабли §6.1 карты), а не готовый ответ.
    TurnEnd {
        ok: bool,
        reason: String,
        text: String,
        error: Option<String>,
        tools: Vec<String>,
        raw: Value,
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

/// Текст ошибки «событие не пришло за отведённое время»: [`Gateway::next_event`]
/// отдаёт его вместо `WorkerError::Timeout`, чтобы вызывающая сторона (pump
/// вкладки «Агент») могла отличить пустой poll от сбоя канала.
pub const TIMEOUT_EVENT: &str = "таймаут события";

impl GatewayError {
    /// Пул пуст: за таймаут не пришло ни события, ни срабатывания фильтра.
    /// Это нормальное состояние ожидания, а не сбой.
    pub fn is_timeout(&self) -> bool {
        matches!(self, GatewayError::Worker(m) if m == TIMEOUT_EVENT)
    }
}

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
    /// Обновление снимка vault по `vault_rev` перед каждым запросом (TR-6.5).
    refresh: Option<Arc<RefreshState>>,
}

/// Данные для обновления снимка vault из потока `egress_check` (TR-6.5).
///
/// Снимок и деанонимизатор должны пересобираться, когда `anonymize_text`
/// завёл новые метки (первый ход, новое ФИО в реплике), и не реже
/// `agent.filter.max_stale_sec` — иначе метка, созданная в этом же ходе, не
/// восстановится в ответе, а долгий ход упрётся в fail-closed.
pub struct RefreshState {
    pub system: Vec<String>,
    pub projects: Vec<String>,
    pub secrets: Vec<String>,
    pub workspace: PathBuf,
    pub project_root: PathBuf,
    pub deanon: Arc<Mutex<DeAnonymizer>>,
}

impl std::fmt::Debug for RefreshState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshState")
            .field("system", &self.system.len())
            .field("projects", &self.projects.len())
            .field("secrets", &self.secrets.len())
            .field("workspace", &self.workspace)
            .finish()
    }
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
            refresh: None,
        }
    }

    /// Включить сверку снимка по `vault_rev` перед каждым запросом (TR-6.5).
    ///
    /// Без неё фильтр живёт снимком, взятым при старте: новые метки этого хода
    /// не деанонимизируются, а ход дольше `max_stale` упирается в fail-closed.
    pub fn with_refresh(mut self, state: Arc<RefreshState>) -> Self {
        self.refresh = Some(state);
        self
    }

    /// Пересобрать снимок, если ревизия vault изменилась (TR-6.5).
    ///
    /// Клиент уже заблокирован вызывающим: `vault_rev` и `snapshot` — два
    /// дешёвых RPC, снимок строится только при реальном изменении.
    fn refresh_if_needed(&self, client: &mut Client, state: &RefreshState) -> Result<(), String> {
        let rev = client.vault_rev().map_err(|e| format!("vault_rev: {e}"))?;
        if self.filter.rev() == Some(rev) {
            return Ok(());
        }
        let snapshot = client
            .snapshot(&self.project)
            .map_err(|e| format!("снимок vault: {e}"))?;
        self.filter
            .set_snapshot(&snapshot, &state.system, &state.projects, &state.secrets);
        let sys_all = crate::filter::sys_values(&state.system, &state.projects);
        if let Ok(mut deanon) = state.deanon.lock() {
            *deanon = DeAnonymizer::new(&snapshot, &sys_all, &state.workspace, &state.project_root);
        }
        Ok(())
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

    /// Вопрос модели → текст для пользователя (TR-5.3, AG-1).
    ///
    /// Разбирает метки только шлюз: берётся тот же [`DeAnonymizer`], что у
    /// событий (`RefreshState.deanon`). В egress-only режиме (`refresh == None`,
    /// стенды и тесты) снимка vault нет — вопрос уходит **как есть, с метками**;
    /// это ожидаемо и видно в ленте.
    fn deanon_question(&self, question: &str) -> String {
        self.refresh
            .as_ref()
            .and_then(|state| {
                state
                    .deanon
                    .lock()
                    .ok()
                    .map(|deanon| deanon.text(question).text)
            })
            .unwrap_or_else(|| question.to_string())
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
                // TR-6.5: перед каждым запросом снимок сверяется по `vault_rev`
                // и пересобирается, если метки изменились (например, реплика
                // только что завела новое ФИО).
                if let Some(state) = self.refresh.as_ref() {
                    self.refresh_if_needed(&mut client, state)?;
                }
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
            // Вопрос формирует модель, значит он приходит с метками. Пользователь
            // должен видеть значения, а UI метки не разбирает (AG-1, TR-5.3), —
            // деанонимизируем здесь, в шлюзе.
            HostCall::AskUser { question } => self.ask.ask(&self.deanon_question(&question)),
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
    /// Python-клиент анонимизации. `None` — egress-only режим (тесты, стенды):
    /// [`Gateway::send`] недоступен, [`Gateway::send_anonymized`] работает.
    client: Option<Arc<Mutex<Client>>>,
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
        Self::spawn_inner(
            spec,
            host,
            project,
            workspace,
            state_dir,
            filter,
            deanon,
            Some(client),
        )
    }

    /// Шлюз без Python-клиента (`GatewayHost::without_client`): фильтр и
    /// деанонимизация работают по готовому снимку vault, `office_*` отвечают
    /// ошибкой, [`Gateway::send_anonymized`] отправляет уже обезличенный текст.
    ///
    /// Нужен для стендов и тестов A3.2, где снимок собирается без Python, и для
    /// режима «анонимизатор недоступен → fail-closed» (TR-6.5).
    pub fn spawn_egress_only(
        spec: &WorkerSpec,
        host: Arc<GatewayHost>,
        project: impl Into<String>,
        workspace: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        filter: Arc<Filter>,
        deanon: Arc<Mutex<DeAnonymizer>>,
    ) -> Result<Gateway, GatewayError> {
        Self::spawn_inner(
            spec, host, project, workspace, state_dir, filter, deanon, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_inner(
        spec: &WorkerSpec,
        host: Arc<GatewayHost>,
        project: impl Into<String>,
        workspace: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
        filter: Arc<Filter>,
        deanon: Arc<Mutex<DeAnonymizer>>,
        client: Option<Arc<Mutex<Client>>>,
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

    /// Признак egress-only режима (нет Python-клиента).
    pub fn is_egress_only(&self) -> bool {
        self.client.is_none()
    }

    /// PID процесса воркера.
    pub fn pid(&self) -> u32 {
        self.worker.pid()
    }

    /// Идентификатор открытой сессии.
    ///
    /// При возобновлении это `session_id` из `start`, при новой сессии —
    /// `session_id`, который выдал воркер (`start.result.session_id`). Нужен
    /// «Очистить историю» (TR-6.6) и вкладке A5.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
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
                // Новая сессия: воркер вернул её идентификатор — запоминаем его,
                // чтобы «Очистить историю» и UI работали без явного `--session`.
                if self.session_id.is_none() {
                    self.session_id = result
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                Ok(result
                    // A3.1: воркер отдаёт объявленный набор как `declared_tools`
                    // (старое `tools` снято). Фактический список — в TurnEnd.
                    .get("declared_tools")
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
    /// В egress-only режиме (без Python) вернёт ошибку — текст, уже прошедший
    /// анонимизацию, отправляется через [`Gateway::send_anonymized`].
    pub fn send(&mut self, text: &str) -> Result<TextResult, GatewayError> {
        let project = self.project.clone();
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| GatewayError::Anonymize("клиент анонимизации недоступен".to_string()))?;
        let anon = {
            let mut client = client
                .lock()
                .map_err(|_| GatewayError::Anonymize("клиент занят".to_string()))?;
            client
                .anonymize_text(&project, text)
                .map_err(|e| GatewayError::Anonymize(e.to_string()))?
        };
        self.send_anonymized(&anon.text)?;
        Ok(anon)
    }

    /// Отправить текст, который **уже** обезличен (метки и пути образов вместо
    /// значений). Нужен egress-only режиму (стенды, тесты A3.2) и повторной
    /// отправке из уже очищенной истории; фильтр исходящего всё равно применяется
    /// в `egress_check`.
    pub fn send_anonymized(&mut self, text: &str) -> Result<(), GatewayError> {
        self.worker
            .call(|id| Down::Send {
                id: id.to_string(),
                text: text.to_string(),
            })
            .map(|_| ())
            .map_err(GatewayError::from)
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
            Ok(up) => self.map_up(up),
            Err(WorkerError::Timeout(_)) => {
                let hits = self.host.take_redactions();
                if hits.is_empty() {
                    Err(GatewayError::Worker(TIMEOUT_EVENT.to_string()))
                } else {
                    Ok(GatewayEvent::Redacted { hits })
                }
            }
            Err(e) => Err(GatewayError::from(e)),
        }
    }

    fn map_up(&self, up: crate::protocol::Up) -> Result<GatewayEvent, GatewayError> {
        use crate::protocol::Up;
        match up {
            Up::Ready {
                protocol, tools, ..
            } => Ok(GatewayEvent::Ready { protocol, tools }),
            Up::Event { event, .. } => {
                if event.get("kind").and_then(Value::as_str) == Some("turn_end") {
                    return Ok(self.map_turn_end(event));
                }
                let (text, unknown) = self
                    .deanon
                    .lock()
                    .map(|d| (d.value(&event), d.unknown_labels(&event)))
                    .unwrap_or_else(|_| (event.clone(), Vec::new()));
                Ok(GatewayEvent::Agent {
                    raw: event,
                    text,
                    unknown,
                })
            }
            Up::Done {
                ok, result, error, ..
            } => Ok(GatewayEvent::Done { ok, result, error }),
            // host_call обрабатывает поток чтения (`worker::read_loop`), в поток
            // событий он не попадает. Если всё же попал — это ошибка транспорта,
            // а не событие агента (раньше здесь было фальшивое «событие»).
            Up::HostCall { call, .. } => Err(GatewayError::Worker(format!(
                "host_call {call} попал в поток событий"
            ))),
            Up::Error { message, .. } => Ok(GatewayEvent::Done {
                ok: false,
                result: Value::Null,
                error: Some(message),
            }),
        }
    }

    /// `turn_end` воркера → [`GatewayEvent::TurnEnd`]: текст деанонимизируется,
    /// «успех при пустом ответе» превращается в сбой (грабли §6.1).
    fn map_turn_end(&self, event: Value) -> GatewayEvent {
        let (ok, reason, error, tools) = turn_end_status(&event);
        let raw_text = event.get("text").and_then(Value::as_str).unwrap_or("");
        GatewayEvent::TurnEnd {
            ok,
            reason,
            text: self.deanon_text(raw_text),
            error: error.map(|e| self.deanon_text(&e)),
            tools,
            raw: event,
        }
    }

    fn deanon_text(&self, text: &str) -> String {
        self.deanon
            .lock()
            .map(|d| d.text(text).text)
            .unwrap_or_else(|_| text.to_string())
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
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| GatewayError::Anonymize("клиент анонимизации недоступен".to_string()))?;
        let snapshot = {
            let mut client = client
                .lock()
                .map_err(|_| GatewayError::Anonymize("клиент занят".to_string()))?;
            client
                .snapshot(&self.project)
                .map_err(|e| GatewayError::Anonymize(e.to_string()))?
        };
        Ok(self.refresh_from_snapshot(&snapshot, system, projects, secrets, project_root))
    }

    /// Применить готовый снимок vault (без Python): фильтр egress_check и
    /// деанонимизатор событий собираются здесь же, `[SYS_n]` нумеруются
    /// одинаково (`filter::sys_values`). Возвращает `rev` снимка.
    pub fn refresh_from_snapshot(
        &self,
        snapshot: &Snapshot,
        system: &[String],
        projects: &[String],
        secrets: &[String],
        project_root: &Path,
    ) -> i64 {
        let rev = snapshot.rev;
        self.filter
            .set_snapshot(snapshot, system, projects, secrets);
        // Фильтр нумерует `[SYS_n]` по цепочке `system` + `projects`, поэтому
        // деанонимизатор получает тот же список в том же порядке.
        let sys_all = crate::filter::sys_values(system, projects);
        if let Ok(mut d) = self.deanon.lock() {
            *d = DeAnonymizer::new(snapshot, &sys_all, &self.workspace, project_root);
        }
        rev
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

    /// Завершить воркер, ожидая выход не дольше `grace` (выгрузка LRU).
    pub fn shutdown_with(self, grace: Duration) {
        self.worker.shutdown_with(grace);
    }
}

/// Разбор `turn_end` воркера: `(ok, reason, error, tools)`.
///
/// Проверка «успех при пустом ответе» (грабли §6.1): если воркер прислал
/// `ok = true`, но ни текста, ни usage нет — это сбой, а не готовый ответ.
fn turn_end_status(event: &Value) -> (bool, String, Option<String>, Vec<String>) {
    let reason = event
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let raw_text = event.get("text").and_then(Value::as_str).unwrap_or("");
    let mut ok = event.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let mut error = event
        .get("error")
        .and_then(Value::as_str)
        .filter(|e| !e.is_empty())
        .map(str::to_string);
    if ok && raw_text.trim().is_empty() && usage_tokens(event) == 0 {
        ok = false;
        error = Some("empty_response: модель не вернула ни текста, ни usage".to_string());
    } else if !ok && error.is_none() {
        error = Some(reason.clone());
    }
    let tools = event
        .get("tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (ok, reason, error, tools)
}

/// Токены из `usage` события `turn_end` (camelCase — формат воркера).
fn usage_tokens(event: &Value) -> u64 {
    let usage = event.get("usage");
    let pick = |key: &str| {
        usage
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    pick("inputTokens") + pick("outputTokens")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::Source;

    #[test]
    fn no_ask_user_refuses() {
        let ask = NoAskUser;
        assert!(ask.ask("кто ты?").is_err());
    }

    /// Собирает вопросы так, как их увидит пользователь (UI).
    struct FakeAsk {
        seen: Mutex<Vec<String>>,
    }

    impl AskUser for FakeAsk {
        fn ask(&self, question: &str) -> Result<Value, String> {
            self.seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(question.to_string());
            Ok(json!({ "answer": "три дня" }))
        }
    }

    /// Снимок vault: `[PERSON_1]` = «Иванов Иван».
    fn snapshot_person() -> Snapshot {
        Snapshot {
            rev: 1,
            labels: vec![hds_anonymize::protocol::SnapshotLabel {
                label: "[PERSON_1]".to_string(),
                entity_type: "PERSON".to_string(),
                forms: vec![hds_anonymize::protocol::SnapshotForm {
                    value: "Иванов Иван".to_string(),
                    gram: None,
                    origin: "ner".to_string(),
                }],
            }],
            others: Vec::new(),
            keep: Vec::new(),
        }
    }

    fn ask_host(refresh: bool) -> (GatewayHost, Arc<FakeAsk>) {
        let dir = std::env::temp_dir().join(format!("hds-gw-ask-{}", std::process::id()));
        let filter = Arc::new(Filter::new(
            "p1",
            Vec::new(),
            crate::settings::UnknownPolicy::Label,
            Duration::from_secs(60),
            None,
        ));
        let ask = Arc::new(FakeAsk {
            seen: Mutex::new(Vec::new()),
        });
        let host = GatewayHost::without_client("p1", &dir, filter, ask.clone(), None);
        if !refresh {
            return (host, ask);
        }
        let deanon = Arc::new(Mutex::new(DeAnonymizer::new(
            &snapshot_person(),
            &[],
            &dir.join("workspace"),
            &dir,
        )));
        let host = host.with_refresh(Arc::new(RefreshState {
            system: Vec::new(),
            projects: Vec::new(),
            secrets: Vec::new(),
            workspace: dir.join("workspace"),
            project_root: dir,
            deanon,
        }));
        (host, ask)
    }

    /// Вопрос модели приходит с метками, пользователь должен увидеть значение:
    /// деанонимизация — в шлюзе, а не в UI (AG-1, TR-5.3).
    #[test]
    fn ask_user_question_is_deanonymized() {
        let (host, ask) = ask_host(true);
        let out = host
            .handle(
                "ask_user",
                &json!({ "question": "Сколько дней? [PERSON_1]" }),
            )
            .expect("ответ пользователя");
        assert_eq!(out["answer"], "три дня");
        let seen = ask.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0], "Сколько дней? Иванов Иван");
    }

    /// Egress-only (`refresh == None`): снимка нет — вопрос остаётся с метками.
    /// Ожидаемое поведение стендов и тестов, документируется в A5.3.
    #[test]
    fn ask_user_without_refresh_keeps_labels() {
        let (host, ask) = ask_host(false);
        host.handle(
            "ask_user",
            &json!({ "question": "Сколько дней? [PERSON_1]" }),
        )
        .expect("ответ пользователя");
        let seen = ask.seen.lock().unwrap();
        assert_eq!(seen[0], "Сколько дней? [PERSON_1]");
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

    #[test]
    fn turn_end_status_keeps_success_with_text() {
        let event = json!({
            "kind": "turn_end",
            "ok": true,
            "reason": "completed",
            "text": "готово",
            "tools": ["read_files", "office_read"]
        });
        let (ok, reason, error, tools) = turn_end_status(&event);
        assert!(ok);
        assert_eq!(reason, "completed");
        assert!(error.is_none());
        assert_eq!(tools, vec!["read_files", "office_read"]);
    }

    #[test]
    fn turn_end_status_turns_empty_success_into_failure() {
        // Грабли §6.1: «успех» без текста и без usage.
        for event in [
            json!({"kind": "turn_end", "ok": true, "reason": "completed", "text": "", "usage": null}),
            json!({"kind": "turn_end", "ok": true, "reason": "completed", "text": "  ", "usage": {"inputTokens": 0, "outputTokens": 0}}),
        ] {
            let (ok, _, error, _) = turn_end_status(&event);
            assert!(!ok, "{event}");
            assert!(error.unwrap_or_default().contains("empty_response"));
        }
        // usage есть — ответ считается состоявшимся (токены потрачены).
        let with_usage = json!({
            "kind": "turn_end", "ok": true, "reason": "completed", "text": "",
            "usage": {"inputTokens": 10, "outputTokens": 0}
        });
        assert!(turn_end_status(&with_usage).0);
    }

    #[test]
    fn turn_end_status_keeps_worker_error_and_reason() {
        let failed = json!({
            "kind": "turn_end", "ok": false, "reason": "error",
            "text": "", "error": "privacy_filter_unavailable: нет снимка vault"
        });
        let (ok, reason, error, tools) = turn_end_status(&failed);
        assert!(!ok);
        assert_eq!(reason, "error");
        assert!(error
            .unwrap_or_default()
            .contains("privacy_filter_unavailable"));
        assert!(tools.is_empty());

        let aborted = json!({"kind": "turn_end", "ok": false, "reason": "aborted", "text": ""});
        let (ok, reason, error, _) = turn_end_status(&aborted);
        assert!(!ok);
        assert_eq!(reason, "aborted");
        assert_eq!(error.as_deref(), Some("aborted"));
    }
}
