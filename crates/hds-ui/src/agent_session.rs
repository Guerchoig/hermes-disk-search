//! Состояние вкладки «Агент» (A5.2): реплика, предпросмотр, сессии и кольцевой
//! буфер событий.
//!
//! Устройство (AGENT_A5_2_HANDOFF_PROMPT.md §4.2):
//!
//! * [`AgentApi`] — одно состояние на процесс UI: реестр живых шлюзов
//!   ([`AgentRegistry`]), ходы по проектам ([`Turn`]), кэш клиента анонимизации
//!   и сквозной номер события. В проде — [`global`] (`OnceLock`), в тестах —
//!   [`AgentApi::for_tests`] (своё состояние: `OnceLock` между тестами не
//!   сбрасывается);
//! * [`Turn`] — один активный ход на проект (`busy`), кольцо на [`RING`]
//!   событий, `session_id` и поток-насос;
//! * **pump-поток** заводится после `open`: в цикле `next_event(500 мс)` кладёт
//!   события в кольцо (`agent`, `redacted`, `turn_end`, `done`, `error`).
//!   `busy` снимается по `TurnEnd`/`Done`/ошибке, а не по таймауту;
//! * `Gateway::send`/`next_event` блокирующие — из HTTP-обработчика их звать
//!   нельзя (грабля §6 п.1), поэтому отправка идёт под замком реестра, а
//!   события читает насос.
//!
//! Значения (текст реплики, ответ модели) живут только в памяти UI: ни журналов,
//! ни файлов модуль не пишет (I8). Предпросмотр и «как видит модель» — метки.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use hds_agent::deanon::DeAnonymizer;
use hds_agent::filter::{sys_values, Labeler, Point};
use hds_agent::gateway::{
    Gateway, GatewayError, GatewayEvent, GatewayHost, NoAskUser, RefreshState,
};
use hds_agent::history;
use hds_agent::projects::{self, Project};
use hds_agent::registry::AgentRegistry;
use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::run::{self, RunOptions};
use hds_agent::secrets::Secrets;
use hds_agent::settings::{self, AgentSettings};
use hds_anonymize::Client;
use hds_core::config::Config;

/// Кольцо событий: больше 500 не храним (AGENT_A5_2 §4.2), `seq` не
/// переиспользуется — UI просит `after=<seq>`.
pub const RING: usize = 500;
/// Период опроса `next_event` в насосе: HTTP-обработчик не ждёт ход, а кольцо
/// наполняется в фоне.
pub const POLL: Duration = Duration::from_millis(500);
/// Лимит реплики (В-A5.2-6): длиннее — 400.
pub const MAX_TEXT: usize = 20_000;
/// Сколько событий отдаёт `GET /api/agent/events` по умолчанию.
pub const EVENTS_DEFAULT: usize = 200;

/// Отказ API вкладки: статус HTTP и текст для пользователя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> ApiError {
        ApiError {
            status,
            message: message.into(),
        }
    }
}

/// Событие кольца. `text` — деанонимизированный вид (лента A5.3), `raw` — «как
/// видит модель» (метки).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub seq: u64,
    pub kind: String,
    pub ts: f64,
    pub text: Option<Value>,
    pub raw: Option<Value>,
}

impl Item {
    fn to_json(&self) -> Value {
        json!({
            "seq": self.seq,
            "kind": self.kind,
            "ts": self.ts,
            "text": self.text,
            "raw": self.raw,
        })
    }
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Один ход проекта: занятость, кольцо событий, сессия и насос.
pub struct Turn {
    /// Ход идёт: второй `message` получает 409 (В-A5.2-3).
    busy: AtomicBool,
    /// Процесс открыт (`start` уже отправлен) — повторный `open` дублировал бы
    /// историю (A0 §3.6).
    opened: AtomicBool,
    /// Насосу пора выходить (`stop`, сброс состояния тестов).
    stop: AtomicBool,
    events: Mutex<VecDeque<Item>>,
    session: Mutex<Option<String>>,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl Turn {
    fn new() -> Turn {
        Turn {
            busy: AtomicBool::new(false),
            opened: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            events: Mutex::new(VecDeque::with_capacity(RING)),
            session: Mutex::new(None),
            pump: Mutex::new(None),
        }
    }

    /// Добавить событие в кольцо; старые вытесняются (500 — граница §4.2).
    fn push(&self, seq: u64, kind: &str, text: Option<Value>, raw: Option<Value>) {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        while events.len() >= RING {
            events.pop_front();
        }
        events.push_back(Item {
            seq,
            kind: kind.to_string(),
            ts: now_ts(),
            text,
            raw,
        });
    }

    /// Занятость хода (ответ `/events` и проверка «второй ход»).
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    /// Сессия воркера, если она уже выдана.
    pub fn session(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Номер последнего события в кольце (0 — событий нет).
    pub fn last_seq(&self) -> u64 {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .back()
            .map(|i| i.seq)
            .unwrap_or(0)
    }
}

/// Состояние вкладки «Агент» в процессе UI.
pub struct AgentApi {
    /// Живые шлюзы: `max_live_projects`, выгрузка по `idle_unload_min`.
    registry: Arc<Mutex<AgentRegistry>>,
    turns: Mutex<HashMap<String, Arc<Turn>>>,
    /// Один Python-воркер анонимизации на процесс UI (снимок vault, метки).
    client: Mutex<Option<Arc<Mutex<Client>>>>,
    /// Сквозной номер события: не переиспользуется.
    seq: Arc<AtomicU64>,
    /// Корень поставки (`sidecar/`, портативный Node): в проде совпадает с
    /// `hds_core::config::project_root()`, в тестах данные живут в `HDS_ROOT`,
    /// а воркеры берутся из репозитория.
    root: PathBuf,
}

static GLOBAL: OnceLock<Arc<AgentApi>> = OnceLock::new();

/// Общее состояние вкладки (прод): реестр из `agent.max_live_projects`.
pub fn global(cfg: &Config) -> &'static Arc<AgentApi> {
    GLOBAL.get_or_init(|| AgentApi::new(AgentRegistry::from_settings(&settings::settings(cfg))))
}

impl AgentApi {
    /// Новое состояние с явным реестром (тесты держат своё). Корень поставки —
    /// `hds_core::config::project_root()`.
    pub fn new(registry: AgentRegistry) -> Arc<AgentApi> {
        AgentApi::at(registry, hds_core::config::project_root())
    }

    /// То же с явным корнем поставки (в тестах данные — в `HDS_ROOT`, воркеры —
    /// из репозитория).
    pub fn at(registry: AgentRegistry, root: impl Into<PathBuf>) -> Arc<AgentApi> {
        Arc::new(AgentApi {
            registry: Arc::new(Mutex::new(registry)),
            turns: Mutex::new(HashMap::new()),
            client: Mutex::new(None),
            seq: Arc::new(AtomicU64::new(0)),
            root: root.into(),
        })
    }

    /// Состояние без общего `OnceLock` — для интеграционных тестов (§6 п.8).
    pub fn for_tests() -> Arc<AgentApi> {
        AgentApi::new(AgentRegistry::defaults())
    }

    /// То же с явным корнем поставки (`sidecar/`, `dist/worker.mjs`).
    pub fn for_tests_at(root: impl Into<PathBuf>) -> Arc<AgentApi> {
        AgentApi::at(AgentRegistry::defaults(), root)
    }

    /// Ход проекта (заводится при первом обращении).
    pub fn turn(&self, project: &str) -> Arc<Turn> {
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(
            turns
                .entry(project.to_string())
                .or_insert_with(|| Arc::new(Turn::new())),
        )
    }

    /// Ход проекта, если он уже заводился.
    pub fn existing_turn(&self, project: &str) -> Option<Arc<Turn>> {
        self.turns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(project)
            .cloned()
    }

    /// Положить событие в кольцо проекта (номер выдаёт счётчик).
    fn push(&self, turn: &Turn, kind: &str, text: Option<Value>, raw: Option<Value>) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        turn.push(seq, kind, text, raw);
    }

    /// `GET /api/agent/events`: `busy`, `session`, последний номер и события
    /// после `after` (≤ `limit`, не больше кольца).
    pub fn events_json(&self, project: &str, after: u64, limit: usize) -> Value {
        let Some(turn) = self.existing_turn(project) else {
            return json!({ "busy": false, "session": Value::Null, "seq": 0, "events": [] });
        };
        let events = turn.events.lock().unwrap_or_else(|e| e.into_inner());
        let items: Vec<Value> = events
            .iter()
            .filter(|item| item.seq > after)
            .take(limit.clamp(1, RING))
            .map(Item::to_json)
            .collect();
        json!({
            "busy": turn.is_busy(),
            "session": turn.session(),
            "seq": events.back().map(|i| i.seq).unwrap_or(0),
            "events": items,
        })
    }
}

/// Зона агента: 503, если зона выключена или не настроена (§4.6 п.1).
fn zone(cfg: &Config) -> Result<AgentSettings, ApiError> {
    let agent = settings::settings(cfg);
    if !agent.enabled {
        return Err(ApiError::new(
            503,
            "агент выключен (agent.enabled: false) — включите зону в config.yaml",
        ));
    }
    if agent.root.is_none() {
        return Err(ApiError::new(
            503,
            "agent.root не задан: анонимная зона не настроена",
        ));
    }
    Ok(agent)
}

/// Проект из реестра: 400 без `project`, 404 для незарегистрированного.
fn require_project(cfg: &Config, project: &str) -> Result<Project, ApiError> {
    let id = project.trim();
    if id.is_empty() {
        return Err(ApiError::new(400, "нет параметра project"));
    }
    projects::get(cfg, id)
        .ok_or_else(|| ApiError::new(404, format!("проект {id} не зарегистрирован")))
}

/// Проверка реплики: пусто — 400, длиннее [`MAX_TEXT`] — 400 (В-A5.2-6).
pub fn check_text(text: &str) -> Result<(), ApiError> {
    if text.trim().is_empty() {
        return Err(ApiError::new(
            400,
            "пустая реплика: введите текст, чтобы увидеть, что уйдёт модели",
        ));
    }
    let len = text.chars().count();
    if len > MAX_TEXT {
        return Err(ApiError::new(
            400,
            format!("реплика длиннее {MAX_TEXT} символов ({len}) — сократите её"),
        ));
    }
    Ok(())
}

/// Относительный путь внутри папки проекта (сравнение по компонентам,
/// регистр не важен — Windows). `None` — путь вне папки.
pub fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let root_abs = std::path::absolute(root).ok()?;
    let path_abs = std::path::absolute(path).ok()?;
    let root_parts: Vec<String> = root_abs
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let path_parts: Vec<String> = path_abs
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    if path_parts.len() <= root_parts.len() {
        return None;
    }
    if !root_parts
        .iter()
        .zip(path_parts.iter())
        .all(|(root, part)| part.to_lowercase() == *root)
    {
        return None;
    }
    let rel = path_parts[root_parts.len()..].join("\\");
    (!rel.is_empty()).then_some(rel)
}

impl AgentApi {
    /// Клиент анонимизации из кэша: один Python-воркер на процесс UI.
    ///
    /// Каталог состояния берётся у первого проекта (там же stderr-лог воркера);
    /// сам воркер обслуживает все проекты по `project_id`.
    pub fn client(&self, cfg: &Config, state_dir: &Path) -> Result<Arc<Mutex<Client>>, ApiError> {
        let mut guard = self.client.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(client) = guard.as_ref() {
            return Ok(Arc::clone(client));
        }
        let client =
            run::open_client_at(cfg, state_dir, &self.root).map_err(|e| ApiError::new(503, e))?;
        *guard = Some(Arc::clone(&client));
        Ok(client)
    }

    fn state_dir(cfg: &Config, project: &str) -> Result<PathBuf, ApiError> {
        settings::state_dir_for(cfg, project)
            .ok_or_else(|| ApiError::new(503, "agent.root не задан: анонимная зона не настроена"))
    }

    /// `POST /api/agent/preview` (TR-5.1): текст с метками — ровно то, что уйдёт
    /// модели. Разбор — только в Python-воркере (`anonymize_text`), своих
    /// регулярок меток в UI нет (AG-1).
    ///
    /// После воркера текст прогоняется тем же словарным фильтром точки `model`,
    /// что и `egress_check` при отправке: NER не выделяет часть уже известных
    /// форм («Иванову» при метке на «Иван»), и без этого предпросмотр расходился
    /// бы с фактически отправленным текстом.
    pub fn preview(&self, cfg: &Config, project: &str, text: &str) -> Result<Value, ApiError> {
        zone(cfg)?;
        let rec = require_project(cfg, project)?;
        check_text(text)?;
        let state_dir = Self::state_dir(cfg, &rec.id)?;
        let client = self.client(cfg, &state_dir)?;
        let result = {
            let mut client = client
                .lock()
                .map_err(|_| ApiError::new(503, "воркер анонимизации занят"))?;
            client
                .anonymize_text(&rec.id, text)
                .map_err(|e| ApiError::new(503, format!("анонимизация: {e}")))?
        };
        let mut warnings = result.warnings.clone();
        let text_out = match egress_preview(cfg, &rec, &client, &result.text) {
            Ok(text) => text,
            Err(e) => {
                warnings.push(format!("словарь проекта: {e}"));
                result.text.clone()
            }
        };
        let labels: Vec<Value> = result
            .labels
            .iter()
            .map(|l| json!({ "token": l.token, "type": l.entity_type, "count": l.count }))
            .collect();
        Ok(json!({
            "project": rec.id,
            "text": text_out,
            "labels": labels,
            "warnings": warnings,
            "changed": text_out != text,
            // Пути ищем в исходном тексте: фильтр заменяет папку проекта на
            // `[SYS_n]` (TR-6.2 в), и после него пути уже не найти.
            "paths": image_paths(cfg, &rec, text),
        }))
    }

    /// `POST /api/agent/label` — «Скрыть ещё…» (TR-1.7). `kept=true` — значение
    /// уже в `keep_terms`, метка не выдаётся (D19); `label=null` — не разобрали.
    pub fn label(
        &self,
        cfg: &Config,
        project: &str,
        value: &str,
        value_type: &str,
    ) -> Result<Value, ApiError> {
        zone(cfg)?;
        let rec = require_project(cfg, project)?;
        let value = value.trim();
        if value.is_empty() {
            return Err(ApiError::new(400, "нет значения: выделите текст в реплике"));
        }
        let kind = match value_type.trim() {
            "" => "OTHER",
            other => other,
        };
        let state_dir = Self::state_dir(cfg, &rec.id)?;
        let client = self.client(cfg, &state_dir)?;
        let out = {
            let mut client = client
                .lock()
                .map_err(|_| ApiError::new(503, "воркер анонимизации занят"))?;
            client
                .label_for_value(&rec.id, value, kind)
                .map_err(|e| ApiError::new(503, format!("метка: {e}")))?
        };
        Ok(json!({
            "label": out.label,
            "type": out.value_type,
            "kept": out.kept,
            "confident": out.confident,
        }))
    }

    /// `POST /api/agent/keep` — «Не скрывать» (D19). `keep_terms` — **база**, а
    /// не проект (В-A5.2-2): значение перестаёт скрываться во всех проектах.
    pub fn keep(&self, cfg: &Config, project: &str, value: &str) -> Result<Value, ApiError> {
        zone(cfg)?;
        let rec = require_project(cfg, project)?;
        let value = value.trim();
        if value.is_empty() {
            return Err(ApiError::new(400, "нет значения: выделите текст в реплике"));
        }
        let state_dir = Self::state_dir(cfg, &rec.id)?;
        let client = self.client(cfg, &state_dir)?;
        let added = {
            let mut client = client
                .lock()
                .map_err(|_| ApiError::new(503, "воркер анонимизации занят"))?;
            client
                .add_keep_term(value)
                .map_err(|e| ApiError::new(503, format!("не скрывать: {e}")))?
        };
        Ok(json!({ "added": added, "scope": "база" }))
    }

    /// `POST /api/agent/attach` (TR-5.2): путь образа по реестру для файла
    /// **из папки проекта**, разовая копия в `workspace\_attachments` — для файла
    /// **вне** папки (В-A5.2-5).
    pub fn attach(&self, cfg: &Config, project: &str, path: &str) -> Result<Value, ApiError> {
        zone(cfg)?;
        let rec = require_project(cfg, project)?;
        let raw = path.trim();
        if raw.is_empty() {
            return Err(ApiError::new(400, "нет параметра path"));
        }
        let src = PathBuf::from(raw);
        if !src.is_absolute() {
            return Err(ApiError::new(400, "нужен абсолютный путь к файлу"));
        }
        if !src.is_file() {
            return Err(ApiError::new(400, format!("файл не найден: {raw}")));
        }
        let workspace = settings::workspace_dir(cfg, &rec.id)
            .ok_or_else(|| ApiError::new(503, "agent.root не задан"))?;
        if let Some(rel) = relative_to(Path::new(&rec.path), &src) {
            return image_of(cfg, &rec, &workspace, &rel);
        }
        // Файл вне папки проекта: разовая копия образа в `workspace\_attachments`.
        let Some(name) = src.file_name().map(|n| n.to_string_lossy().to_string()) else {
            return Err(ApiError::new(400, "у пути нет имени файла"));
        };
        let dir = workspace.join("_attachments");
        std::fs::create_dir_all(&dir)
            .map_err(|e| ApiError::new(500, format!("каталог вложений: {e}")))?;
        let out = dir.join(&name);
        let state_dir = Self::state_dir(cfg, &rec.id)?;
        let client = self.client(cfg, &state_dir)?;
        {
            let mut client = client
                .lock()
                .map_err(|_| ApiError::new(503, "воркер анонимизации занят"))?;
            client
                .anonymize_agent_to(&src, &out, &rec.id, None)
                .map_err(|e| ApiError::new(503, format!("вложение: {e}")))?;
        }
        Ok(json!({
            "kind": "attachment",
            "name": name,
            "anon": format!("_attachments\\{name}"),
            "path": out.to_string_lossy(),
            "note": "копия образа в workspace/_attachments; оригинал не менялся",
        }))
    }
}

/// Путь образа по реестру для относительного пути проекта: запись есть —
/// отдаём `workspace\<anon_rel>`, нет — 409 «запустите hds agent sync».
fn image_of(cfg: &Config, rec: &Project, workspace: &Path, rel: &str) -> Result<Value, ApiError> {
    let rel = rel.trim_start_matches('\\');
    let Some(db) = settings::registry_db(cfg, &rec.id) else {
        return Err(ApiError::new(
            400,
            format!("'{}' не похож на идентификатор проекта (D21)", rec.id),
        ));
    };
    if !db.is_file() {
        return Err(ApiError::new(
            409,
            "файл не зеркалирован — запустите hds agent sync",
        ));
    }
    let registry =
        ArtifactRegistry::open(&db).map_err(|e| ApiError::new(500, format!("реестр: {e}")))?;
    let artifact = registry
        .by_orig(rel)
        .map_err(|e| ApiError::new(500, format!("реестр: {e}")))?
        .ok_or_else(|| {
            ApiError::new(
                409,
                format!("файл не зеркалирован — запустите hds agent sync ({rel})"),
            )
        })?;
    if artifact.anon_rel.trim().is_empty() {
        return Err(ApiError::new(
            409,
            "у файла нет образа (статус unsupported?) — см. «Файлы проекта»",
        ));
    }
    let image = workspace.join(&artifact.anon_rel);
    if !image.is_file() {
        return Err(ApiError::new(
            409,
            format!(
                "образ не найден: {} — запустите hds agent sync",
                image.display()
            ),
        ));
    }
    Ok(json!({
        "kind": "image",
        "rel": artifact.orig_rel,
        "anon": artifact.anon_rel,
        "path": image.to_string_lossy(),
        "status": artifact.status.as_str(),
    }))
}

/// Словарный фильтр точки `model` для предпросмотра: возвращает текст ровно в
/// том виде, в каком его увидит воркер (`egress_check` при отправке).
///
/// Новые метки выдаёт тот же Python-воркер (`label_for_value`, тип `OTHER`) —
/// как `ClientLabeler` шлюза, поэтому своих регулярок меток в UI нет (AG-1).
/// Аудит не подключаем: предпросмотр — не отправка, событий аудита не плодим.
fn egress_preview(
    cfg: &Config,
    rec: &Project,
    client: &Arc<Mutex<Client>>,
    text: &str,
) -> Result<String, String> {
    let snapshot = {
        let mut client = client
            .lock()
            .map_err(|_| "клиент анонимизации занят".to_string())?;
        client
            .snapshot(&rec.id)
            .map_err(|e| format!("снимок vault: {e}"))?
    };
    let agent = settings::settings(cfg);
    let secrets = Secrets::load(&settings::secrets_file(cfg))
        .map(|s| s.secret_strings())
        .unwrap_or_default();
    let (system, names) = run::system_values(cfg, rec);
    let filter = run::build_filter(&rec.id, &agent, None, &system, &names, &secrets, &snapshot);
    let mut guard = client
        .lock()
        .map_err(|_| "клиент анонимизации занят".to_string())?;
    let mut labeler = PreviewLabeler {
        client: &mut guard,
        project: &rec.id,
    };
    let outcome = filter
        .redact(Point::Model, &json!(text), &mut labeler)
        .map_err(|e| e.reason())?;
    Ok(outcome
        .payload
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| text.to_string()))
}

/// Выдача новых меток для предпросмотра (тот же вызов, что у `egress_check`).
struct PreviewLabeler<'a> {
    client: &'a mut Client,
    project: &'a str,
}

impl Labeler for PreviewLabeler<'_> {
    fn label_for(&mut self, value: &str) -> Option<String> {
        self.client
            .label_for_value(self.project, value, "OTHER")
            .ok()
            .and_then(|r| r.label)
    }
}

/// Абсолютные пути файлов проекта, встречающиеся в тексте реплики, → путь
/// образа по реестру.
///
/// Не заменяем текст (иначе предпросмотр разошёлся бы с отправленным:
/// `Gateway::send` анонимизирует исходный текст, В-A5.2-1) — отдаём соответствия
/// `paths`, UI показывает их рядом с предпросмотром.
fn image_paths(cfg: &Config, rec: &Project, text: &str) -> Vec<Value> {
    let Some(db) = settings::registry_db(cfg, &rec.id) else {
        return Vec::new();
    };
    if !db.is_file() {
        return Vec::new();
    }
    let (Ok(registry), Some(workspace)) = (
        ArtifactRegistry::open(&db),
        settings::workspace_dir(cfg, &rec.id),
    ) else {
        return Vec::new();
    };
    let Ok(rows) = registry.list() else {
        return Vec::new();
    };
    let root = Path::new(&rec.path);
    let haystack = text.to_lowercase();
    let mut out: Vec<Value> = Vec::new();
    for artifact in rows {
        if artifact.anon_rel.trim().is_empty() {
            continue;
        }
        let original = root.join(&artifact.orig_rel).to_string_lossy().to_string();
        if !haystack.contains(&original.to_lowercase()) {
            continue;
        }
        let image = workspace.join(&artifact.anon_rel);
        if !image.is_file() {
            continue;
        }
        out.push(json!({
            "from": original,
            "to": image.to_string_lossy(),
            "rel": artifact.orig_rel,
            "anon": artifact.anon_rel,
        }));
    }
    out
}

impl AgentApi {
    /// `POST /api/agent/message` (§4.6): 400 (пусто/нет проекта), 409 (ход уже
    /// идёт), 503 (зона выключена/нет `agent.root`), затем `plan_with` →
    /// `prepare` → `Gateway::spawn|get_or_open` → `open` → `send`.
    ///
    /// Отправляется **исходный** текст (В-A5.2-1): шлюз анонимизирует его сам,
    /// предпросмотр — только показ. `busy` ставится до сборки плана и снимается
    /// насосом по `TurnEnd`/`Done`/ошибке (или сразу, если ход не начался).
    pub fn message(
        self: &Arc<Self>,
        cfg: &Config,
        project: &str,
        text: &str,
        session: Option<&str>,
    ) -> Result<Value, ApiError> {
        zone(cfg)?;
        let rec = require_project(cfg, project)?;
        check_text(text)?;
        let turn = self.turn(&rec.id);
        if turn.busy.swap(true, Ordering::SeqCst) {
            return Err(ApiError::new(
                409,
                "ход уже идёт — дождитесь ответа или нажмите «Стоп»",
            ));
        }
        let result = self.start_turn(cfg, &rec, &turn, text, session);
        if result.is_err() {
            turn.busy.store(false, Ordering::SeqCst);
        }
        result
    }

    /// Один ход: шлюз, анонимизация реплики, событие `prompt`, отправка.
    #[allow(clippy::too_many_lines)]
    fn start_turn(
        self: &Arc<Self>,
        cfg: &Config,
        rec: &Project,
        turn: &Arc<Turn>,
        text: &str,
        session: Option<&str>,
    ) -> Result<Value, ApiError> {
        let opts = RunOptions {
            prompt: Some(text.to_string()),
            session: session.map(str::to_string),
            max_iterations: 12,
            subagents: false,
            json: false,
            quiet: false,
        };
        let plan = run::plan_with(cfg, &rec.id, &opts, &self.root).map_err(plan_error)?;
        run::prepare(&plan).map_err(|e| ApiError::new(500, e))?;

        // Снимок vault: словарь исходящего и деанонимизатор событий (TR-6.5).
        let client = self.client(cfg, &plan.state_dir)?;
        let snapshot = {
            let mut client = client
                .lock()
                .map_err(|_| ApiError::new(503, "воркер анонимизации занят"))?;
            client
                .snapshot(&rec.id)
                .map_err(|e| ApiError::new(503, format!("снимок vault: {e}")))?
        };
        let secrets = plan.secret_strings();
        let filter = Arc::new(run::build_filter(
            &rec.id,
            &plan.settings,
            plan.audit.clone(),
            &plan.system,
            &plan.project_names,
            &secrets,
            &snapshot,
        ));
        let deanon = Arc::new(Mutex::new(DeAnonymizer::new(
            &snapshot,
            &sys_values(&plan.system, &plan.project_names),
            &plan.workspace,
            Path::new(&rec.path),
        )));
        let egress_log = plan
            .settings
            .egress_log
            .then(|| plan.state_dir.join("egress"));
        let refresh = Arc::new(RefreshState {
            system: plan.system.clone(),
            projects: plan.project_names.clone(),
            secrets,
            workspace: plan.workspace.clone(),
            project_root: PathBuf::from(&rec.path),
            deanon: Arc::clone(&deanon),
        });
        let host = Arc::new(
            GatewayHost::new(
                rec.id.clone(),
                &plan.workspace,
                Arc::clone(&filter),
                Arc::clone(&client),
                Arc::new(NoAskUser),
                egress_log,
            )
            .with_refresh(refresh),
        );

        // 1. Шлюз: живой из реестра или новый процесс (под замком реестра).
        let client_for_worker = Arc::clone(&client);
        let opened = {
            let project_id = rec.id.clone();
            let workspace = plan.workspace.clone();
            let state_dir = plan.state_dir.clone();
            let spec = plan.spec.clone();
            let spawn = move || {
                Gateway::spawn(
                    &spec,
                    Arc::clone(&host),
                    project_id,
                    workspace,
                    state_dir,
                    Arc::clone(&filter),
                    Arc::clone(&deanon),
                    Arc::clone(&client_for_worker),
                )
                .map_err(|e| GatewayError::Worker(e.to_string()))
            };
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            // Простой → выгрузка (В-A5-5). Кольцо событий не рвём: `session_id`
            // остаётся и возобновляется через историю.
            let reaped = registry.reap_idle(Instant::now());
            if reaped.iter().any(|id| id == &rec.id) {
                turn.opened.store(false, Ordering::SeqCst);
            }
            let existed = registry.get_mut(&rec.id).is_some();
            let need_open = !existed || !turn.opened.load(Ordering::SeqCst);
            let gateway = registry
                .get_or_open(&rec.id, Instant::now(), spawn)
                .map_err(|e| ApiError::new(503, format!("воркер агента: {e}")))?;
            match need_open {
                true => Some(open_session(gateway, &plan, turn, session)?),
                false => None,
            }
        };
        self.finish_turn(cfg, rec, turn, client, text, opened)
    }
}

/// `start` воркера: история из файлов сессии (TR-3.9) + возобновление по
/// `session_id`. Возвращает `true`, если сессия возобновлена.
fn open_session(
    gateway: &mut Gateway,
    plan: &run::RunPlan,
    turn: &Turn,
    session: Option<&str>,
) -> Result<bool, ApiError> {
    let resume = session
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| turn.session())
        .or_else(|| history::latest_session(&plan.state_dir).ok());
    let (messages, session_id) = match resume {
        Some(id) if history::valid_session_id(&id) => {
            match history::read_messages(&plan.state_dir, &id) {
                Ok(messages) => (messages, Some(id)),
                // Файла истории нет — начинаем новую сессию, а не падаем.
                Err(_) => (Vec::new(), None),
            }
        }
        _ => (Vec::new(), None),
    };
    let resumed = session_id.is_some();
    gateway
        .open(
            plan.provider.clone(),
            messages,
            session_id,
            // Потолок итераций — как `RunOptions::new` (12), пока в настройках
            // вкладки он не выведен (A5.6).
            12,
            plan.settings.subagents,
        )
        .map_err(|e| ApiError::new(503, format!("старт сессии: {e}")))?;
    turn.opened.store(true, Ordering::SeqCst);
    if let Some(id) = gateway.session_id() {
        *turn.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
    }
    Ok(resumed)
}

/// `plan_with` → статус API: 404 для незарегистрированного проекта, 503 для
/// ненастроенной зоны/секретов/воркера, 500 для остального.
fn plan_error(message: String) -> ApiError {
    let not_registered = message.contains("не зарегистрирован");
    let unavailable = message.contains("agent.root")
        || message.contains("не настроена")
        || message.contains("секрет")
        || message.contains("secrets.json")
        || message.contains("воркер агента")
        || message.contains("интерпретатор");
    let status = if not_registered {
        404
    } else if unavailable {
        503
    } else {
        500
    };
    ApiError::new(status, message)
}

impl AgentApi {
    /// Фазы 2–3 хода: анонимизация реплики (событие `prompt` до отправки) и
    /// отправка уже обезличенного текста — те же два шага, что в
    /// `Gateway::send`, но событие `prompt` попадает в кольцо до старта хода.
    fn finish_turn(
        self: &Arc<Self>,
        cfg: &Config,
        rec: &Project,
        turn: &Arc<Turn>,
        client: Arc<Mutex<Client>>,
        text: &str,
        opened: Option<bool>,
    ) -> Result<Value, ApiError> {
        let project = rec.id.as_str();
        let anonymized = {
            let mut client = client
                .lock()
                .map_err(|_| ApiError::new(503, "воркер анонимизации занят"))?;
            client
                .anonymize_text(project, text)
                .map_err(|e| ApiError::new(503, format!("анонимизация: {e}")))?
        };
        let labels: Vec<Value> = anonymized
            .labels
            .iter()
            .map(|l| json!({ "token": l.token, "type": l.entity_type, "count": l.count }))
            .collect();
        // Событие «что уйдёт модели» — тот же текст, что даст `egress_check`
        // воркера (иначе лента показывала бы формы, которые фильтр заменит).
        let prompt_text = egress_preview(cfg, rec, &client, &anonymized.text)
            .unwrap_or_else(|_| anonymized.text.clone());
        self.push(
            turn,
            "prompt",
            Some(json!({ "text": prompt_text, "labels": labels })),
            None,
        );
        let session_out = {
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            let Some(gateway) = registry.get_mut(project) else {
                return Err(ApiError::new(
                    503,
                    "сессия закрыта (проект выгружен из реестра) — повторите отправку",
                ));
            };
            gateway
                .send_anonymized(&anonymized.text)
                .map_err(|e| ApiError::new(500, format!("отправка: {e}")))?;
            let session = gateway.session_id().map(str::to_string);
            if let Some(id) = session.as_ref() {
                *turn.session.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
            }
            session
        };
        if let Some(resumed) = opened {
            self.push(
                turn,
                "session",
                Some(json!({ "session": session_out, "resumed": resumed })),
                None,
            );
        }
        self.ensure_pump(turn, project);
        Ok(json!({
            "ok": true,
            "session_id": session_out,
            "seq": turn.last_seq(),
        }))
    }

    /// Завести насос, если его нет: один поток на проект, опрос `next_event`.
    fn ensure_pump(self: &Arc<Self>, turn: &Arc<Turn>, project: &str) {
        let mut guard = turn.pump.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(handle) = guard.as_ref() {
            if !handle.is_finished() {
                return;
            }
        }
        turn.stop.store(false, Ordering::SeqCst);
        let api = Arc::clone(self);
        let turn_clone = Arc::clone(turn);
        let project = project.to_string();
        let name = format!("hds-agent-pump-{project}");
        let spawned = std::thread::Builder::new()
            .name(name)
            .spawn(move || pump_loop(&api, &turn_clone, &project));
        if let Ok(handle) = spawned {
            *guard = Some(handle);
        }
        // Не смогли поднять поток — события останутся в канале воркера,
        // `busy` снимет следующий запрос по ошибке канала.
    }

    /// `POST /api/agent/abort` — прервать текущий ход.
    ///
    /// `busy` снимает насос по `TurnEnd`/`Done` (воркер отвечает `done` на
    /// `abort` даже без активного хода) — здесь его не трогаем: иначе UI
    /// разрешил бы второй ход, который воркер отверг бы `turn_in_progress`.
    pub fn abort(&self, cfg: &Config, project: &str) -> Result<Value, ApiError> {
        zone(cfg)?;
        let rec = require_project(cfg, project)?;
        if self.existing_turn(&rec.id).is_none() {
            return Err(ApiError::new(
                404,
                format!("у проекта {} ещё не было хода", rec.id),
            ));
        }
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        let Some(gateway) = registry.get_mut(&rec.id) else {
            return Err(ApiError::new(
                404,
                format!("сессия проекта {} не открыта", rec.id),
            ));
        };
        gateway
            .abort()
            .map_err(|e| ApiError::new(500, format!("стоп: {e}")))?;
        Ok(json!({ "ok": true, "aborted": true }))
    }

    /// `POST /api/agent/stop` — мягкая остановка сессии: она сохраняется на
    /// диске и возобновляется следующим ходом. Ход закрыт совсем, поэтому
    /// `busy` снимается здесь же.
    pub fn stop(&self, cfg: &Config, project: &str) -> Result<Value, ApiError> {
        zone(cfg)?;
        let rec = require_project(cfg, project)?;
        let Some(turn) = self.existing_turn(&rec.id) else {
            return Err(ApiError::new(
                404,
                format!("у проекта {} ещё не было хода", rec.id),
            ));
        };
        {
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            let Some(gateway) = registry.get_mut(&rec.id) else {
                return Err(ApiError::new(
                    404,
                    format!("сессия проекта {} не открыта", rec.id),
                ));
            };
            gateway
                .stop()
                .map_err(|e| ApiError::new(500, format!("остановка: {e}")))?;
        }
        turn.opened.store(false, Ordering::SeqCst);
        turn.stop.store(true, Ordering::SeqCst);
        turn.busy.store(false, Ordering::SeqCst);
        self.push(
            &turn,
            "notice",
            Some(json!({ "text": "сессия остановлена" })),
            None,
        );
        Ok(json!({ "ok": true, "stopped": true }))
    }

    /// Сбросить состояние между тестами (`OnceLock` не сбрасывается, §6 п.8):
    /// остановить насосы, выгрузить шлюзы и закрыть Python-воркер.
    #[doc(hidden)]
    pub fn reset_for_tests(&self) {
        let turns: Vec<Arc<Turn>> = {
            let mut map = self.turns.lock().unwrap_or_else(|e| e.into_inner());
            map.drain().map(|(_, turn)| turn).collect()
        };
        for turn in &turns {
            turn.stop.store(true, Ordering::SeqCst);
            turn.busy.store(false, Ordering::SeqCst);
        }
        for turn in turns {
            let handle = turn.pump.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(handle) = handle {
                let _ = handle.join();
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while Arc::strong_count(&turn) > 1 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        {
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            for project in registry.projects() {
                registry.unload(&project);
            }
        }
        let client = self.client.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(client) = client {
            if let Ok(client) = Arc::try_unwrap(client) {
                if let Ok(client) = client.into_inner() {
                    client.shutdown();
                }
            }
        }
        self.seq.store(0, Ordering::SeqCst);
    }
}

/// Насос событий проекта: `next_event` под замком реестра, события в кольцо.
///
/// Замок держится только на время одного опроса (≤ [`POLL`]) — следующая
/// реплика ждёт освобождения не дольше периода опроса.
fn pump_loop(api: &AgentApi, turn: &Turn, project: &str) {
    loop {
        if turn.stop.load(Ordering::SeqCst) {
            break;
        }
        let event = {
            let mut registry = match api.registry.lock() {
                Ok(registry) => registry,
                Err(_) => break,
            };
            match registry.get_mut(project) {
                Some(gateway) => gateway.next_event(POLL),
                // Проект выгружен (LRU или простой): насос завершается, кольцо и
                // `session_id` остаются — сессия возобновится следующим ходом.
                None => break,
            }
        };
        match event {
            Ok(GatewayEvent::Agent { raw, text, .. }) => {
                api.push(turn, "agent", Some(text), Some(raw));
            }
            Ok(GatewayEvent::Redacted { hits }) => {
                let labels: Vec<Value> = hits
                    .iter()
                    .map(|h| json!({ "label": h.label, "source": h.source.as_str() }))
                    .collect();
                api.push(turn, "redacted", Some(json!({ "hits": labels })), None);
            }
            Ok(GatewayEvent::TurnEnd {
                ok,
                reason,
                text,
                error,
                tools,
                raw,
            }) => {
                api.push(
                    turn,
                    "turn_end",
                    Some(json!({
                        "ok": ok,
                        "reason": reason,
                        "text": text,
                        "error": error,
                        "tools": tools,
                    })),
                    Some(raw),
                );
                turn.busy.store(false, Ordering::SeqCst);
            }
            Ok(GatewayEvent::Done { ok, result, error }) => {
                api.push(
                    turn,
                    "done",
                    Some(json!({ "ok": ok, "result": result, "error": error })),
                    None,
                );
                turn.busy.store(false, Ordering::SeqCst);
            }
            // Готовность канала — не событие ленты (A5.3 покажет отдельно).
            Ok(GatewayEvent::Ready { .. }) => {}
            Err(e) if e.is_timeout() => {}
            Err(e) => {
                // Закрытый канал на простое — не ошибка пользователю; сбой
                // посреди хода — событие и снятие `busy`.
                if !turn.busy.load(Ordering::SeqCst) {
                    break;
                }
                api.push(turn, "error", Some(json!({ "error": e.to_string() })), None);
                turn.busy.store(false, Ordering::SeqCst);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_last_events_and_never_reuses_seq() {
        let api = AgentApi::for_tests();
        let turn = api.turn("P0001");
        for n in 0..(RING + 20) {
            api.push(&turn, "agent", Some(json!({ "n": n })), None);
        }
        let value = api.events_json("P0001", 0, RING);
        let events = value["events"].as_array().unwrap();
        assert_eq!(events.len(), RING);
        let first = events[0]["seq"].as_u64().unwrap();
        let last = events[RING - 1]["seq"].as_u64().unwrap();
        assert_eq!(last, (RING + 20) as u64);
        assert_eq!(first, 21);
        assert_eq!(value["seq"], last);
        // Дыр в нумерации нет: UI просит `after=<seq>`.
        for pair in events.windows(2) {
            let a = pair[0]["seq"].as_u64().unwrap();
            let b = pair[1]["seq"].as_u64().unwrap();
            assert_eq!(b, a + 1);
        }
    }

    #[test]
    fn events_respect_after_and_limit() {
        let api = AgentApi::for_tests();
        let turn = api.turn("P0002");
        for n in 0..10 {
            api.push(&turn, "agent", Some(json!({ "n": n })), None);
        }
        let value = api.events_json("P0002", 7, 2);
        let events = value["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["seq"], 8);
        assert_eq!(events[1]["seq"], 9);
        // `after` за пределами буфера — пусто, последний номер сохраняется.
        let empty = api.events_json("P0002", 99, EVENTS_DEFAULT);
        assert!(empty["events"].as_array().unwrap().is_empty());
        assert_eq!(empty["seq"], 10);
        // Проекта нет — пустой ответ, а не ошибка.
        let none = api.events_json("P9999", 0, EVENTS_DEFAULT);
        assert_eq!(none["seq"], 0);
        assert_eq!(none["busy"], false);
    }

    #[test]
    fn text_limit_and_empty_message() {
        assert!(check_text("Подготовь письмо").is_ok());
        assert_eq!(check_text("   ").unwrap_err().status, 400);
        assert_eq!(check_text("").unwrap_err().status, 400);
        let long = "я".repeat(MAX_TEXT + 1);
        let err = check_text(&long).unwrap_err();
        assert_eq!(err.status, 400);
        assert!(err.message.contains("20000"), "{}", err.message);
        assert!(check_text(&"я".repeat(MAX_TEXT)).is_ok());
    }

    #[test]
    fn relative_to_compares_components() {
        let root = std::env::temp_dir().join("hds-agent-rel");
        let inside = root.join("Протоколы").join("Встреча.docx");
        assert_eq!(
            relative_to(&root, &inside).as_deref(),
            Some("Протоколы\\Встреча.docx")
        );
        assert!(relative_to(&root, &root).is_none(), "сама папка — не файл");
        let outside = std::env::temp_dir().join("другой").join("файл.txt");
        assert!(relative_to(&root, &outside).is_none());
    }
}
