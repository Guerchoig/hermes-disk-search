//! Вкладка «Агент» (A5.1): проекты, состояние зеркала и файлы реестра.
//!
//! Границы под-этапа: только чтение и создание проекта; чат, лента событий,
//! решения, метки и настройки — A5.2…A5.6. В **серверном профиле** вкладка
//! недоступна (D20): маршруты отвечают 403, кнопка вкладки не рисуется.
//!
//! Значения документов здесь не появляются: `projects.json` содержит пути
//! пользователя (зона пользователя), реестр — относительные пути и статусы,
//! журнал демона — пути/счётчики/причины. Значения из базы меток в A5.1 не
//! читаются вообще (I8).

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hds_agent::factory::{anonymizer_factory, extractor_factory};
use hds_agent::labels::{self, LabelsError, LabelsReport};
use hds_agent::mirror::{LazyAnonymizer, LazyExtractor, Mirror};
use hds_agent::registry_db::{ArtifactRegistry, Status};
use hds_agent::results::{
    self, append_message, Decision, DecisionReport, MessageKind, ResultsError, BACKUP_SUBDIR,
    CONFLICTS_DIR, SERVICE_DIR,
};
use hds_agent::secrets::Secrets;
use hds_agent::{projects, settings};
use hds_core::config::{self, Config, Profile};
use serde_json::{json, Value};

use crate::agent_session::{self, AgentApi, ApiError};
use crate::{Reply, ReqHeaders};

/// Сколько живёт один поток SSE: дальше сервер закрывает соединение, а
/// `EventSource` подключается заново с `Last-Event-ID` (A5.3 §4.1).
pub const SSE_MAX_SEC: u64 = 300;
/// Период heartbeat-комментария `: ping`, если событий нет.
pub const SSE_HEARTBEAT_SEC: u64 = 15;
/// Пауза переподключения, которую получает `EventSource` (`retry:`).
pub const SSE_RETRY_MS: u64 = 1500;

/// Сколько последних строк журнала отдаёт панель состояния.
pub const JOURNAL_TAIL: usize = 20;
/// Больше этого журнал не читаем (штатный ротируется при 1 МБ).
const JOURNAL_MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Провайдер и модель по умолчанию — те же, что у `hds agent project add` (В-12).
pub const DEFAULT_PROVIDER: &str = hds_agent::projects::DEFAULT_PROVIDER;
pub const DEFAULT_MODEL: &str = hds_agent::projects::DEFAULT_MODEL;

fn ok(v: Value) -> Reply {
    (200, "application/json", v.to_string())
}

fn err(status: u16, message: impl Into<String>) -> Reply {
    let message: String = message.into();
    (
        status,
        "application/json",
        json!({ "error": message }).to_string(),
    )
}

/// Доступна ли вкладка: в серверном профиле — нет (D20).
pub fn available(cfg: &Config) -> bool {
    config::profile(cfg) != Profile::Server
}

/// Гейт маршрутов вкладки: `Err` — готовый ответ.
///
/// Конфиг читается на каждый запрос (ядро кэширует его по mtime), поэтому смена
/// `agent.root` и профиля подхватывается без перезапуска UI.
pub fn gate() -> Result<Config, Reply> {
    let cfg = match config::load() {
        Ok(cfg) => cfg,
        Err(e) => return Err(err(500, format!("конфиг: {}", e.message()))),
    };
    if !available(&cfg) {
        return Err(err(
            403,
            "вкладка «Агент» недоступна в серверном профиле (D20: функция рабочего стола)",
        ));
    }
    Ok(cfg)
}

/// `GET /api/agent/projects`: список проектов и состояние зоны.
pub fn projects_json(cfg: &Config) -> Reply {
    let agent = settings::settings(cfg);
    let list = match projects::load(cfg) {
        Ok(registry) => registry.projects,
        Err(e) => return err(500, e),
    };
    let items: Vec<Value> = list.iter().map(|p| project_item(cfg, p)).collect();
    ok(json!({
        "enabled": agent.enabled,
        "root": agent.root.as_ref().map(|p| p.display().to_string()),
        "projects": items,
    }))
}

fn project_item(cfg: &Config, p: &projects::Project) -> Value {
    let (files, pending) = registry_counts(cfg, &p.id);
    json!({
        "id": p.id,
        "name": p.name,
        "path": p.path,
        "provider": p.provider,
        "model": p.model,
        "files": files,
        "pending": pending,
    })
}

/// Числа реестра: файлов и ждущих решения. Нет БД — нули (ещё не зеркалился).
fn registry_counts(cfg: &Config, id: &str) -> (i64, i64) {
    let Some(db) = settings::registry_db(cfg, id) else {
        return (0, 0);
    };
    if !db.is_file() {
        return (0, 0);
    }
    match ArtifactRegistry::open(&db) {
        Ok(registry) => (
            registry.count().unwrap_or(0),
            registry.list_pending().map(|v| v.len() as i64).unwrap_or(0),
        ),
        Err(_) => (0, 0),
    }
}

/// `P` + 4 цифры (D21) — та же проверка, что у CLI и `settings::registry_db`.
pub fn is_project_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 5 && bytes[0] == b'P' && bytes[1..].iter().all(|b| b.is_ascii_digit())
}

/// Следующий свободный идентификатор (кнопка «Новый проект»).
pub fn next_id<'a>(used: impl Iterator<Item = &'a str>) -> String {
    let mut max = 0u32;
    for id in used {
        if is_project_id(id) {
            if let Ok(n) = id[1..].parse::<u32>() {
                max = max.max(n);
            }
        }
    }
    format!("P{:04}", max + 1)
}

/// `POST /api/agent/projects`: создать проект (имя + папка; id — автоматически).
pub fn project_add_json(cfg: &Config, body: &str) -> Reply {
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(400, format!("тело запроса не JSON: {e}")),
    };
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if path.is_empty() {
        return err(400, "нужно поле path — папка проекта");
    }
    let dir = PathBuf::from(&path);
    if !dir.is_absolute() {
        return err(400, "path должен быть абсолютным путём папки проекта");
    }
    if !dir.is_dir() {
        return err(400, format!("папка проекта не найдена: {path}"));
    }
    // Проект — папка пользователя; `agent.root` — зона агента (TR-8.1).
    if let Some(zone) = settings::zone_root(cfg) {
        if dir.starts_with(&zone) {
            return err(400, "папка проекта не может лежать внутри agent.root");
        }
    }
    let registry = match projects::load(cfg) {
        Ok(registry) => registry,
        Err(e) => return err(500, e),
    };
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| next_id(registry.projects.iter().map(|p| p.id.as_str())));
    if !is_project_id(&id) {
        return err(
            400,
            format!("идентификатор проекта — P + 4 цифры (D21): {id}"),
        );
    }
    if registry.projects.iter().any(|p| p.id == id) {
        return err(409, format!("проект {id} уже есть"));
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            dir.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.trim().is_empty())
        })
        .unwrap_or_else(|| id.clone());
    let provider = value
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_PROVIDER)
        .to_string();
    let model = value
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MODEL)
        .to_string();
    let project = projects::Project {
        id: id.clone(),
        name,
        path,
        provider,
        model,
        created: hds_core::timefmt::now_epoch() as i64,
    };
    if let Err(e) = projects::add(cfg, project.clone()) {
        return err(500, e);
    }
    ok(json!({ "ok": true, "project": project_item(cfg, &project) }))
}

/// `GET /api/agent/state?project=<P>`: демон зеркала, журнал, числа реестра.
pub fn state_json(cfg: &Config, project: &str) -> Reply {
    let id = project.trim();
    if id.is_empty() {
        return err(400, "нет параметра project");
    }
    let Some(p) = projects::get(cfg, id) else {
        return err(404, format!("проект {id} не зарегистрирован"));
    };
    let state_dir = settings::state_dir_for(cfg, id);
    let lock = state_dir.as_ref().map(|d| d.join("sync.lock"));
    let daemon = lock
        .as_ref()
        .map(|l| l.is_file() && !hds_index::watch::lock_is_stale(l))
        .unwrap_or(false);
    let journal = state_dir
        .as_ref()
        .map(|d| tail_lines(&d.join("sync.log"), JOURNAL_TAIL))
        .unwrap_or_default();
    let (files, pending) = registry_counts(cfg, id);
    let agent = settings::settings(cfg);
    ok(json!({
        "id": p.id,
        "name": p.name,
        "path": p.path,
        "provider": p.provider,
        "model": p.model,
        "enabled": agent.enabled,
        "root": agent.root.as_ref().map(|r| r.display().to_string()),
        "daemon": daemon,
        "lock": lock.as_ref().map(|l| l.display().to_string()),
        "journal": journal,
        "files": files,
        "pending": pending,
    }))
}

/// `GET /api/agent/files?project=<P>`: файлы проекта по реестру артефактов.
pub fn files_json(cfg: &Config, project: &str) -> Reply {
    let id = project.trim();
    if id.is_empty() {
        return err(400, "нет параметра project");
    }
    if projects::get(cfg, id).is_none() {
        return err(404, format!("проект {id} не зарегистрирован"));
    }
    let Some(db) = settings::registry_db(cfg, id) else {
        return err(
            400,
            format!("'{id}' не похож на идентификатор проекта (D21)"),
        );
    };
    if !db.is_file() {
        // Проект ещё не зеркалился — пустая таблица, а не ошибка.
        return ok(json!({ "count": 0, "pending": 0, "files": [] }));
    }
    let registry = match ArtifactRegistry::open(&db) {
        Ok(registry) => registry,
        Err(e) => return err(500, format!("{e}")),
    };
    let rows = match registry.list() {
        Ok(rows) => rows,
        Err(e) => return err(500, format!("{e}")),
    };
    let pending = registry.list_pending().map(|v| v.len()).unwrap_or(0);
    let files: Vec<Value> = rows
        .iter()
        .map(|a| {
            json!({
                "orig": a.orig_rel,
                "anon": a.anon_rel,
                "kind": a.kind.as_str(),
                "status": a.status.as_str(),
                "ts": a.ts,
            })
        })
        .collect();
    ok(json!({
        "count": rows.len(),
        "pending": pending,
        "files": files,
    }))
}

/// Строковое поле JSON-тела (`{"text": "..."}`); нет поля — пусто.
fn body_str(body: &str, key: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get(key).and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}

/// `ApiError` вкладки → HTTP-ответ с текстом (§4.8: ошибки 400/403/409/503).
fn api_err(e: ApiError) -> Reply {
    err(e.status, e.message)
}

/// `POST /api/agent/preview` `{project,text}` — что уйдёт модели (TR-5.1).
pub fn preview_json(cfg: &Config, body: &str) -> Reply {
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    match api.preview(cfg, &body_str(body, "project"), &body_str(body, "text")) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `POST /api/agent/label` `{project,value,type?}` — «Скрыть ещё…».
pub fn label_json(cfg: &Config, body: &str) -> Reply {
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    match api.label(
        cfg,
        &body_str(body, "project"),
        &body_str(body, "value"),
        &body_str(body, "type"),
    ) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `POST /api/agent/keep` `{project,value}` — «Не скрывать» (глобальная база).
pub fn keep_json(cfg: &Config, body: &str) -> Reply {
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    match api.keep(cfg, &body_str(body, "project"), &body_str(body, "value")) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `POST /api/agent/attach` `{project,path}` — образ файла проекта или копия
/// вложения в `workspace\_attachments` (TR-5.2).
pub fn attach_json(cfg: &Config, body: &str) -> Reply {
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    match api.attach(cfg, &body_str(body, "project"), &body_str(body, "path")) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `POST /api/agent/message` `{project,text,session?}` — отправить ход.
pub fn message_json(cfg: &Config, body: &str) -> Reply {
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    let session = body_str(body, "session");
    let session = session.trim();
    let session = (!session.is_empty()).then_some(session);
    match api.message(
        cfg,
        &body_str(body, "project"),
        &body_str(body, "text"),
        session,
    ) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `POST /api/agent/abort` `{project}` — прервать текущий ход.
pub fn abort_json(cfg: &Config, body: &str) -> Reply {
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    match api.abort(cfg, &body_str(body, "project")) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `POST /api/agent/stop` `{project}` — мягкая остановка сессии.
pub fn stop_json(cfg: &Config, body: &str) -> Reply {
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    match api.stop(cfg, &body_str(body, "project")) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `POST /api/agent/answer` `{project, ask_id, answer}` — ответ на вопрос
/// агента (`ask_user`, A5.3 §4.3). `ask_id` — номер из события `ask` в ленте.
pub fn answer_json(cfg: &Config, body: &str) -> Reply {
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(400, format!("тело запроса не JSON: {e}")),
    };
    let project = value
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let answer = value.get("answer").and_then(Value::as_str).unwrap_or("");
    // `ask_id` принимаем числом или строкой: страница шлёт число, curl — как удобно.
    let ask_id = value.get("ask_id").and_then(|v| match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    });
    let Some(ask_id) = ask_id else {
        return err(400, "нужно поле ask_id — номер вопроса из ленты");
    };
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    match api.answer(cfg, &project, ask_id, answer) {
        Ok(v) => ok(v),
        Err(e) => api_err(e),
    }
}

/// `ResultsError` → HTTP-ответ вкладки (A5.4): 400 — плохой путь,
/// 404 — нет записи/варианта, 409 — решать нечего, файл занят, конфликт,
/// 500 — сбой ввода-вывода/реестра/воркера.
fn results_err(e: ResultsError) -> Reply {
    match &e {
        ResultsError::BadPath(_) => err(400, e.to_string()),
        ResultsError::NotFound(_) | ResultsError::NoVariant(_) => err(404, e.to_string()),
        ResultsError::WrongStatus { .. }
        | ResultsError::OriginalChanged(_)
        | ResultsError::NothingToDelegate(_)
        | ResultsError::Kind(_) => err(409, e.to_string()),
        // Текст A4: пользователь должен понять, что закрыть и что не потеряно.
        ResultsError::Busy(path) => err(
            409,
            format!(
                "файл занят: закройте {} в Word/Excel и повторите — вариант остался в pending, \
                 статус не изменён",
                path.display()
            ),
        ),
        ResultsError::Io(..) | ResultsError::Registry(_) | ResultsError::Mirror(_) => {
            err(500, e.to_string())
        }
    }
}

/// Поднять зеркало по конфигу и выполнить действие над ним.
///
/// Воркеры ленивые (`hds_agent::factory`): для «Сравнить» `.md`/`.txt` Python
/// не запускается вообще, для docx поднимается только извлекатель. Второй
/// копии фабрик в UI нет (§5 «Воркер агента»).
fn with_mirror<T>(
    cfg: &Config,
    id: &str,
    action: impl for<'m> FnOnce(&mut Mirror<'m>) -> Result<T, ResultsError>,
) -> Result<T, Reply> {
    let Some(rec) = projects::get(cfg, id) else {
        return Err(err(404, format!("проект {id} не зарегистрирован")));
    };
    let project_path = PathBuf::from(&rec.path);
    if !project_path.is_dir() {
        return Err(err(404, format!("папка проекта не найдена: {}", rec.path)));
    }
    let Some(db) = settings::registry_db(cfg, id) else {
        return Err(err(
            400,
            format!("'{id}' не похож на идентификатор проекта (D21)"),
        ));
    };
    if !db.is_file() {
        return Err(err(
            404,
            format!("проект {id} ещё не зеркалился — запустите hds agent sync"),
        ));
    }
    let registry = ArtifactRegistry::open(&db).map_err(|e| err(500, format!("{e}")))?;
    let root = hds_core::config::project_root();
    let mut anonymizer = LazyAnonymizer::new(anonymizer_factory(cfg, &root));
    let mut extractor = LazyExtractor::new(extractor_factory(&root));
    let mut mirror = Mirror::from_config(
        cfg,
        id,
        &project_path,
        &registry,
        &mut anonymizer,
        &mut extractor,
    )
    .map_err(|e| err(500, format!("{e}")))?;
    action(&mut mirror).map_err(results_err)
}

/// Поднять зеркало поверх **общего** воркера анонимизации UI (A5.5).
///
/// `with_mirror` (A5.4) поднимает ленивый `LazyAnonymizer` — для панели меток
/// это дало бы второй Python-процесс на тот же `anonymize.db` (A5.4 §6 п.37).
/// Здесь берётся клиент `AgentApi::client` (один на процесс UI), замок
/// держится только на пересборку образов и проход зеркала.
fn with_client_mirror<T>(
    cfg: &Config,
    id: &str,
    action: impl for<'m> FnOnce(&mut Mirror<'m>) -> Result<T, Reply>,
) -> Result<T, Reply> {
    let Some(rec) = projects::get(cfg, id) else {
        return Err(err(404, format!("проект {id} не зарегистрирован")));
    };
    let project_path = PathBuf::from(&rec.path);
    if !project_path.is_dir() {
        return Err(err(404, format!("папка проекта не найдена: {}", rec.path)));
    }
    let Some(db) = settings::registry_db(cfg, id) else {
        return Err(err(
            400,
            format!("'{id}' не похож на идентификатор проекта (D21)"),
        ));
    };
    if !db.is_file() {
        return Err(err(
            404,
            format!("проект {id} ещё не зеркалился — запустите hds agent sync"),
        ));
    }
    let registry = ArtifactRegistry::open(&db).map_err(|e| err(500, format!("{e}")))?;
    let Some(state_dir) = settings::state_dir_for(cfg, id) else {
        return Err(err(503, "agent.root не задан: анонимная зона не настроена"));
    };
    let client = agent_session::global(cfg)
        .client(cfg, &state_dir)
        .map_err(api_err)?;
    let mut guard = client
        .lock()
        .map_err(|_| err(503, "воркер анонимизации занят"))?;
    let root = hds_core::config::project_root();
    let mut extractor = LazyExtractor::new(extractor_factory(&root));
    let mut mirror = Mirror::from_config(
        cfg,
        id,
        &project_path,
        &registry,
        &mut *guard,
        &mut extractor,
    )
    .map_err(|e| err(500, format!("{e}")))?;
    action(&mut mirror)
}

/// `LabelsError` → HTTP-ответ вкладки (A5.5): 500 — сбой реестра, воркера,
/// извлечения или ввода-вывода (404 отдаёт `with_client_mirror`).
fn labels_err(e: LabelsError) -> Reply {
    err(500, e.to_string())
}

/// Тело `GET /api/agent/labels`: к строке метки добавлены `files` и
/// `forms_count` — по ним страница показывает «где встречается» и выбирает
/// старшую метку.
fn labels_body(report: &LabelsReport) -> Value {
    let labels: Vec<Value> = report
        .labels
        .iter()
        .map(|row| {
            json!({
                "label": row.label,
                "type": row.entity_type,
                "forms": row
                    .forms
                    .iter()
                    .map(|form| json!({
                        "value": form.value,
                        "gram": form.gram,
                        "origin": form.origin,
                    }))
                    .collect::<Vec<_>>(),
                "aliases": row.aliases,
                "occurrences": row
                    .occurrences
                    .iter()
                    .map(|o| json!({ "rel": o.rel, "anon_rel": o.anon_rel, "count": o.count }))
                    .collect::<Vec<_>>(),
                "files": row.files(),
                "forms_count": row.forms_count(),
            })
        })
        .collect();
    let merges: Vec<Value> = report
        .merges
        .iter()
        .map(|merge| {
            json!({
                "id": merge.id,
                "ts": merge.ts,
                "keep": merge.keep,
                "dropped": merge.dropped,
                "forms_moved": merge.forms_moved,
            })
        })
        .collect();
    json!({
        "rev": report.rev,
        "labels": labels,
        "merges": merges,
        "keep": report.keep,
    })
}

/// `GET /api/agent/labels?project=<P>` — панель «Метки проекта» (TR-2.6).
pub fn labels_json(cfg: &Config, project: &str) -> Reply {
    let id = project.trim();
    if id.is_empty() {
        return err(400, "нет параметра project");
    }
    match with_client_mirror(cfg, id, |mirror| mirror.labels_report().map_err(labels_err)) {
        Ok(report) => ok(labels_body(&report)),
        Err(reply) => reply,
    }
}

/// Метка проекта: `[PERSON_3|дат]` → (`PERSON`, 3). Разбор — `hds_core::labels`.
fn label_key(token: &str) -> Option<(String, u32)> {
    hds_core::labels::label_parts(token).map(|(kind, n, _gram)| (kind, n))
}

/// Разбор тела `POST /api/agent/labels/merge`: `(project, keep, drop)`.
///
/// Проверки 400 — здесь, до Python: пустое тело, нет `keep`/`drop`, метка не
/// разобрана, разные типы, «склеить одну метку».
fn merge_request(body: &str) -> Result<(String, String, Vec<String>), Reply> {
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return Err(err(400, format!("тело запроса не JSON: {e}"))),
    };
    let project = value
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if project.is_empty() {
        return Err(err(400, "нужно поле project"));
    }
    let keep = value
        .get("keep")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if keep.is_empty() {
        return Err(err(400, "нужно поле keep — метка, которая останется"));
    }
    let Some((keep_type, keep_n)) = label_key(&keep) else {
        return Err(err(400, format!("не разобрана метка '{keep}'")));
    };
    let list: Vec<String> = value
        .get("drop")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if list.is_empty() {
        return Err(err(400, "нужно поле drop — метки, которые склеиваются"));
    }
    let mut drop: Vec<String> = Vec::new();
    for token in list {
        let Some((kind, n)) = label_key(&token) else {
            return Err(err(400, format!("не разобрана метка '{token}'")));
        };
        if kind != keep_type {
            return Err(err(
                400,
                format!("нельзя объединить метки разных типов: {keep} и {token}"),
            ));
        }
        if (kind.as_str(), n) == (keep_type.as_str(), keep_n) {
            continue;
        }
        if !drop.contains(&token) {
            drop.push(token);
        }
    }
    if drop.is_empty() {
        return Err(err(400, "выберите минимум две разные метки"));
    }
    Ok((project, keep, drop))
}

/// `merge_id` из тела: число или строка (`curl.exe` шлёт и так, и так).
fn merge_id_of(value: &Value) -> Option<i64> {
    match value.get("merge_id") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `POST /api/agent/labels/merge` `{project, keep, drop}` — «Объединить» (TR-2.6).
///
/// Порядок: проверка меток по снимку (404/409) → `merge_entities` (пишет
/// `merge_log`) → пересборка образов со младшими метками → **один**
/// `sync_once` → сообщение агенту и `notice` в ленту.
pub fn labels_merge_json(cfg: &Config, body: &str) -> Reply {
    let (project, keep, drop) = match merge_request(body) {
        Ok(parts) => parts,
        Err(reply) => return reply,
    };
    if projects::get(cfg, &project).is_none() {
        return err(404, format!("проект {project} не зарегистрирован"));
    }
    if !settings::settings(cfg).enabled {
        return err(
            503,
            "анонимная зона выключена (agent.enabled: false) — склейка недоступна",
        );
    }
    let Some(state_dir) = settings::state_dir_for(cfg, &project) else {
        return err(503, "agent.root не задан: анонимная зона не настроена");
    };
    let api = agent_session::global(cfg);
    let client = match api.client(cfg, &state_dir) {
        Ok(client) => client,
        Err(e) => return api_err(e),
    };
    let outcome = {
        let mut guard = match client.lock() {
            Ok(guard) => guard,
            Err(_) => return err(503, "воркер анонимизации занят"),
        };
        // Что есть в проекте: 404 «метки нет» и 409 «уже псевдоним»
        // отличаются до записи, а не по тексту ошибки Python.
        let known = match guard.project_labels(&project) {
            Ok(labels) => labels,
            Err(e) => return err(503, format!("метки проекта: {e}")),
        };
        let primary: BTreeSet<&str> = known.labels.iter().map(|row| row.label.as_str()).collect();
        let aliases: BTreeSet<&str> = known
            .labels
            .iter()
            .flat_map(|row| row.aliases.iter().map(String::as_str))
            .collect();
        for token in std::iter::once(&keep).chain(drop.iter()) {
            if primary.contains(token.as_str()) {
                continue;
            }
            if aliases.contains(token.as_str()) {
                return err(
                    409,
                    format!("метка {token} уже псевдоним другой метки — выберите действующую"),
                );
            }
            return err(
                404,
                format!("метка {token} не принадлежит проекту {project}"),
            );
        }
        match guard.merge_entities(&project, &keep, &drop) {
            Ok(outcome) => outcome,
            Err(e) => return err(500, format!("склейка: {e}")),
        }
    };

    let refresh = match with_client_mirror(cfg, &project, |mirror| {
        let refresh = mirror
            .refresh_label_images(&outcome.dropped)
            .map_err(results_err)?;
        mirror
            .sync_once()
            .map_err(|e| err(500, format!("проход зеркала: {e}")))?;
        Ok(refresh)
    }) {
        Ok(refresh) => refresh,
        Err(reply) => return reply,
    };

    let Some(rec) = projects::get(cfg, &project) else {
        return err(404, format!("проект {project} не зарегистрирован"));
    };
    let message = match append_message(
        Path::new(&rec.path),
        MessageKind::LabelsMerged,
        "",
        labels::merge_message(&outcome.keep, &outcome.dropped),
    ) {
        Ok(message) => message,
        Err(e) => return results_err(e),
    };
    let notice = format!(
        "метки склеены: {} → {}; пересобрано образов: {}",
        joined_or_dash(&outcome.dropped),
        outcome.keep,
        refresh.rebuilt.len()
    );
    api.notice(&project, &notice);
    ok(json!({
        "ok": true,
        "keep": outcome.keep,
        "dropped": outcome.dropped,
        "merge_id": outcome.merge_id,
        "forms_moved": outcome.forms_moved,
        "rebuilt": rebuilt_json(&refresh.rebuilt),
        "skipped": refresh.skipped,
        "message": agent_message_json(&message),
    }))
}

/// Список меток через запятую; пусто — прочерк (текст `notice`).
fn joined_or_dash(labels: &[String]) -> String {
    if labels.is_empty() {
        "—".to_string()
    } else {
        labels.join(", ")
    }
}

/// Пересобранные образы для ответа вкладки: путь, образ, статус.
fn rebuilt_json(rebuilt: &[hds_agent::results::RebuiltImage]) -> Vec<Value> {
    rebuilt
        .iter()
        .map(|image| {
            json!({
                "rel": image.rel,
                "anon": image.anon_rel,
                "status": image.status.as_str(),
            })
        })
        .collect()
}

/// Сообщение агенту в ответе вкладки: вид, путь и текст (только метки).
fn agent_message_json(message: &hds_agent::results::Message) -> Value {
    json!({
        "kind": message.kind.as_str(),
        "rel": message.rel,
        "text": message.text,
    })
}

/// `POST /api/agent/labels/unmerge` `{project, merge_id}` — «Разъединить».
///
/// Отменяются только склейки из `merge_log` (A5.5+); после отмены образы
/// пересобираются по **старшей** метке: NER снова выдаст младшую.
pub fn labels_unmerge_json(cfg: &Config, body: &str) -> Reply {
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(400, format!("тело запроса не JSON: {e}")),
    };
    let project = value
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if project.is_empty() {
        return err(400, "нужно поле project");
    }
    let Some(merge_id) = merge_id_of(&value) else {
        return err(400, "нужно поле merge_id — номер склейки из журнала");
    };
    if projects::get(cfg, &project).is_none() {
        return err(404, format!("проект {project} не зарегистрирован"));
    }
    if !settings::settings(cfg).enabled {
        return err(
            503,
            "анонимная зона выключена (agent.enabled: false) — разъединение недоступно",
        );
    }
    let Some(state_dir) = settings::state_dir_for(cfg, &project) else {
        return err(503, "agent.root не задан: анонимная зона не настроена");
    };
    let api = agent_session::global(cfg);
    let client = match api.client(cfg, &state_dir) {
        Ok(client) => client,
        Err(e) => return api_err(e),
    };
    let outcome = {
        let mut guard = match client.lock() {
            Ok(guard) => guard,
            Err(_) => return err(503, "воркер анонимизации занят"),
        };
        let known = match guard.project_labels(&project) {
            Ok(labels) => labels,
            Err(e) => return err(503, format!("метки проекта: {e}")),
        };
        if !known.merges.iter().any(|merge| merge.id == merge_id) {
            return err(
                404,
                format!(
                    "склейка {merge_id} не найдена — «Разъединить» возможно только для \
                     склеек, записанных после A5.5"
                ),
            );
        }
        match guard.unmerge_entities(&project, merge_id) {
            Ok(outcome) => outcome,
            Err(e) => return err(409, format!("разъединить не удалось: {e}")),
        }
    };

    let refresh = match with_client_mirror(cfg, &project, |mirror| {
        let refresh = mirror
            .refresh_label_images(std::slice::from_ref(&outcome.keep))
            .map_err(results_err)?;
        mirror
            .sync_once()
            .map_err(|e| err(500, format!("проход зеркала: {e}")))?;
        Ok(refresh)
    }) {
        Ok(refresh) => refresh,
        Err(reply) => return reply,
    };

    let Some(rec) = projects::get(cfg, &project) else {
        return err(404, format!("проект {project} не зарегистрирован"));
    };
    let message = match append_message(
        Path::new(&rec.path),
        MessageKind::LabelsUnmerged,
        "",
        labels::unmerge_message(&outcome.restored),
    ) {
        Ok(message) => message,
        Err(e) => return results_err(e),
    };
    let notice = format!(
        "склейка меток отменена: {} снова самостоятельные; пересобрано образов: {}",
        joined_or_dash(&outcome.restored),
        refresh.rebuilt.len()
    );
    api.notice(&project, &notice);
    ok(json!({
        "ok": true,
        "merge_id": outcome.merge_id,
        "keep": outcome.keep,
        "restored": outcome.restored,
        "forms_removed": outcome.forms_removed,
        "rebuilt": rebuilt_json(&refresh.rebuilt),
        "skipped": refresh.skipped,
        "message": agent_message_json(&message),
    }))
}

/// `GET /api/agent/pending?project=<P>` — варианты, ждущие решения (TR-7.6).
///
/// Источник — реестр: `list_pending()` (`pending_accept`) плюс
/// `list_by_status(Conflict)`. Для каждого варианта — отчёт валидатора
/// (TR-7.8) и признаки наличия варианта/основания/копий в `_conflicts`.
/// Значения документа сюда не попадают: только пути, статусы и вердикты.
pub fn pending_json(cfg: &Config, project: &str) -> Reply {
    let id = project.trim();
    if id.is_empty() {
        return err(400, "нет параметра project");
    }
    let Some(rec) = projects::get(cfg, id) else {
        return err(404, format!("проект {id} не зарегистрирован"));
    };
    let Some(db) = settings::registry_db(cfg, id) else {
        return err(
            400,
            format!("'{id}' не похож на идентификатор проекта (D21)"),
        );
    };
    if !db.is_file() {
        // Проект ещё не зеркалился — пустой список, а не ошибка (как `files`).
        return ok(json!({ "count": 0, "pending": 0, "conflicts": false, "items": [] }));
    }
    let registry = match ArtifactRegistry::open(&db) {
        Ok(registry) => registry,
        Err(e) => return err(500, format!("{e}")),
    };
    let mut rows = match registry.list_pending() {
        Ok(rows) => rows,
        Err(e) => return err(500, format!("{e}")),
    };
    match registry.list_by_status(Status::Conflict) {
        Ok(conflicts) => rows.extend(conflicts),
        Err(e) => return err(500, format!("{e}")),
    }
    rows.sort_by(|a, b| {
        a.ts.total_cmp(&b.ts)
            .then_with(|| a.orig_rel.cmp(&b.orig_rel))
    });
    let project_dir = PathBuf::from(&rec.path);
    let conflicts_root = settings::workspace_dir(cfg, id).map(|w| w.join(CONFLICTS_DIR));
    let has_conflicts = conflicts_root
        .as_ref()
        .map(|dir| {
            fs::read_dir(dir)
                .map(|mut it| it.next().is_some())
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let items: Vec<Value> = rows
        .iter()
        .map(|row| {
            let paths = results::variant_paths(&project_dir, &row.orig_rel);
            let report = hds_agent::validator::Report::read(&paths.pending);
            json!({
                "rel": row.orig_rel,
                "anon_rel": row.anon_rel,
                "kind": row.kind.as_str(),
                "status": row.status.as_str(),
                "ts": row.ts,
                "conflict": row.status == Status::Conflict,
                "variant": paths.pending.is_file(),
                "base": paths.base.is_file(),
                "conflicts": has_conflicts,
                "variant_path": paths.pending.display().to_string(),
                "report": report.map(|r| json!({
                    "verdict": r.verdict.as_str(),
                    "checks": r.checks.len(),
                    "rev": r.rev,
                })),
            })
        })
        .collect();
    ok(json!({
        "count": items.len(),
        "pending": items.len(),
        "conflicts": has_conflicts,
        "items": items,
    }))
}

/// `GET /api/agent/compare?project=<P>&rel=<отн. путь>` — `Mirror::compare`.
pub fn compare_json(cfg: &Config, project: &str, rel: &str) -> Reply {
    let id = project.trim();
    if id.is_empty() {
        return err(400, "нет параметра project");
    }
    let rel = rel.trim();
    if rel.is_empty() {
        return err(400, "нет параметра rel — относительный путь оригинала");
    }
    match with_mirror(cfg, id, |mirror| mirror.compare(rel)) {
        Ok(report) => match serde_json::to_value(&report) {
            Ok(value) => ok(value),
            Err(e) => err(500, format!("сравнение не сериализовано: {e}")),
        },
        Err(reply) => reply,
    }
}

/// Строка решения для ленты: путь и метки, без содержимого документа (I8).
fn decision_notice(decision: Decision, report: &DecisionReport) -> String {
    let action = match decision {
        Decision::Accept => "принято",
        Decision::Reject => "отклонено",
        Decision::Conflict(results::ConflictChoice::Agent) => "конфликт: оставлен вариант агента",
        Decision::Conflict(results::ConflictChoice::New) => "конфликт: взята новая версия",
        Decision::Conflict(results::ConflictChoice::Delegate) => "конфликт: поручено агенту",
    };
    let mut text = format!("решение по «{}»: {action}", report.orig_rel);
    if report.backup.is_some() {
        text.push_str(&format!(
            "; копия прежней версии — {SERVICE_DIR}\\{BACKUP_SUBDIR}"
        ));
    }
    if let Some(copy) = &report.conflict_copy {
        text.push_str(&format!("; правки для переноса — {copy}"));
    }
    text
}

/// `POST /api/agent/decision` `{project, rel, decision, choice?}`.
///
/// Решения A4 не переписываются: ядро — `Mirror::decide`. Коды ответа:
/// 400 (тело/решение/путь), 404 (проект/запись/вариант), 409 (файл занят,
/// решать нечего, конфликт), 503 (зона выключена), 500 (сбой).
pub fn decision_json(cfg: &Config, body: &str) -> Reply {
    let value: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return err(400, format!("тело запроса не JSON: {e}")),
    };
    let project = value
        .get("project")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let rel = value
        .get("rel")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let action = value
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let choice = value.get("choice").and_then(Value::as_str);
    if project.is_empty() {
        return err(400, "нужно поле project");
    }
    if rel.is_empty() {
        return err(400, "нужно поле rel — относительный путь оригинала");
    }
    let Some(decision) = Decision::parse(&action, choice) else {
        return err(
            400,
            format!(
                "decision '{action}' не разобрано: accept|reject|conflict \
                 (для conflict — choice agent|new|delegate)"
            ),
        );
    };
    if projects::get(cfg, &project).is_none() {
        return err(404, format!("проект {project} не зарегистрирован"));
    }
    if !settings::settings(cfg).enabled {
        return err(
            503,
            "анонимная зона выключена (agent.enabled: false) — решения недоступны",
        );
    }
    match with_mirror(cfg, &project, |mirror| mirror.decide(&rel, decision)) {
        Ok(report) => {
            // Лента узнаёт о решении так же, как о «Стоп» — событием `notice`.
            agent_session::global(cfg).notice(&project, &decision_notice(decision, &report));
            let mut body = json!({
                "ok": true,
                "decision": decision.as_str(),
                "rel": report.orig_rel,
                "anon": report.anon_rel,
                "status": report.status.as_str(),
                "warnings": report.warnings,
            });
            if let Some(backup) = &report.backup {
                body["backup"] = json!(backup.display().to_string());
            }
            if let Some(copy) = &report.conflict_copy {
                body["conflict_copy"] = json!(copy);
            }
            if let Some(message) = &report.message {
                body["message"] = json!({
                    "kind": message.kind.as_str(),
                    "rel": message.rel,
                    "text": message.text,
                });
            }
            ok(body)
        }
        Err(reply) => reply,
    }
}

/// `Accept: text/event-stream` — клиент ждёт поток, а не JSON.
pub fn wants_stream(h: &ReqHeaders) -> bool {
    h.accept
        .as_deref()
        .map(|a| a.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false)
}

/// Кадр события: `id:` (для `Last-Event-ID` при переподключении), `event: item`,
/// `data:` — JSON одного события в одну строку.
///
/// `serde_json` экранирует переводы строк внутри строк, поэтому кадр не рвётся
/// даже на тексте с `\n` (иначе SSE-клиент получил бы обрезанный `data:`).
pub fn sse_frame(seq: u64, item: &Value) -> String {
    format!("id: {seq}\nevent: item\ndata: {item}\n\n")
}

/// Первый кадр потока — состояние хода: `{busy, session, seq}`.
pub fn sse_state_frame(state: &Value) -> String {
    format!("event: state\ndata: {state}\n\n")
}

/// `after` для потока: `Last-Event-ID` (EventSource при переподключении) важнее
/// query `?after=`; нет ни того, ни другого — с нуля (В-A5.3-4).
pub fn stream_after(h: &ReqHeaders, query: &str) -> u64 {
    h.last_event_id
        .as_deref()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .or_else(|| crate::qp(query, "after").and_then(|v| v.trim().parse::<u64>().ok()))
        .unwrap_or(0)
}

/// Поток событий проекта (`GET /api/agent/events` + `Accept: text/event-stream`).
///
/// Пишет ответ **без `Content-Length`**: обычный `write_reply_with` для потока не
/// годится (грабля §6 п.1). Источник данных — тот же `events_json`, поэтому
/// JSON-фолбэк, тесты и `curl.exe` видят то же самое.
///
/// Поток закрывается по ошибке записи (клиент ушёл), по [`SSE_MAX_SEC`] (дальше
/// `EventSource` переподключится) и когда ход проекта исчез из состояния
/// (`reset_for_tests`): `None` из `existing_turn` — конец потока, а не паника
/// (грабля §6 п.15).
pub fn stream_events(stream: &mut TcpStream, cfg: &Config, project: &str, after: u64) {
    // Мёртвый клиент не должен держать поток вечно (грабля §4.1).
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let head = "HTTP/1.1 200 OK\r\n\
                Content-Type: text/event-stream; charset=utf-8\r\n\
                Cache-Control: no-cache\r\n\
                Connection: close\r\n\
                X-Accel-Buffering: no\r\n\r\n";
    let retry = format!("retry: {SSE_RETRY_MS}\n\n");
    if stream.write_all(head.as_bytes()).is_err() || stream.write_all(retry.as_bytes()).is_err() {
        return;
    }
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    if stream
        .write_all(sse_state_frame(&api.state(project)).as_bytes())
        .is_err()
    {
        return;
    }
    let _ = stream.flush();
    let deadline = Instant::now() + Duration::from_secs(SSE_MAX_SEC);
    let mut after = after;
    // Был ли ход в кольце: подписка до первого хода не должна закрываться
    // (браузер подключается раньше, чем пользователь отправляет реплику).
    let mut seen_turn = false;
    let mut last_write = Instant::now();
    while Instant::now() < deadline {
        let Some(turn) = api.existing_turn(project) else {
            if seen_turn {
                // Ход исчез (сброс состояния) — закрываем поток состоянием, а не 500.
                let _ = stream.write_all(sse_state_frame(&api.state(project)).as_bytes());
                break;
            }
            // Хода ещё не было — ждём, изредка напоминая о себе `: ping`.
            std::thread::sleep(Duration::from_millis(250));
            if last_write.elapsed() >= Duration::from_secs(SSE_HEARTBEAT_SEC) {
                if stream.write_all(b": ping\n\n").is_err() {
                    return;
                }
                let _ = stream.flush();
                last_write = Instant::now();
            }
            continue;
        };
        seen_turn = true;
        // Событие приходит сразу (будильник из `push`), таймаут — повод для ping.
        turn.wait_for_change(Duration::from_secs(SSE_HEARTBEAT_SEC));
        let page = api.events_json(project, after, agent_session::RING);
        let events = page["events"].as_array().cloned().unwrap_or_default();
        if events.is_empty() {
            if stream.write_all(b": ping\n\n").is_err() {
                return;
            }
            let _ = stream.flush();
            last_write = Instant::now();
            continue;
        }
        for item in events {
            let seq = item["seq"].as_u64().unwrap_or(after);
            if stream.write_all(sse_frame(seq, &item).as_bytes()).is_err() {
                return;
            }
            after = seq;
        }
        if stream.flush().is_err() {
            return;
        }
        last_write = Instant::now();
    }
}

/// `GET /api/agent/events?project=<P>&after=<seq>&limit=200` — события без SSE
/// (JSON-фолбэк для тестов, `curl.exe` и страницы без `EventSource`).
pub fn events_json(cfg: &Config, project: &str, after: Option<u64>, limit: Option<usize>) -> Reply {
    // Проект проверяем мягко: событий могло не быть вовсе — пустой ответ, а не
    // 404. Зона выключена — 503/RU-текст (как у остальных маршрутов вкладки).
    if let Err(e) = projects::get(cfg, project.trim())
        .map(|_| ())
        .ok_or_else(|| ApiError::new(400, "нет параметра project"))
    {
        return api_err(e);
    }
    let api: &'static Arc<AgentApi> = agent_session::global(cfg);
    ok(api.events_json(
        project.trim(),
        after.unwrap_or(0),
        limit.unwrap_or(agent_session::EVENTS_DEFAULT),
    ))
}

// --- Настройки агента (A5.6): провайдер/модель/ключ, фильтр, MCP ----------

/// Пределы числовых настроек панели: вне диапазона — 400, а не тихая подмена.
pub const SETTINGS_MAX_LIVE: u64 = 8;
pub const SETTINGS_MIN_TIMEOUT_SEC: u64 = 10;
pub const SETTINGS_MIN_ASK_SEC: u64 = 30;
pub const SETTINGS_MIN_STALE_SEC: u64 = 30;

/// YAML-литерал строки (одинарные кавычки, `''` — экранирование).
fn yaml_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn yaml_bool(value: bool) -> String {
    if value {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

/// Строковое поле тела; `None` — поля нет. Не строка — 400.
fn field_str(value: &Value, key: &str) -> Result<Option<String>, Reply> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.trim().to_string())),
        Some(_) => Err(err(400, format!("поле {key} должно быть строкой"))),
    }
}

/// Логическое поле тела; не `true`/`false` — 400.
fn field_bool(value: &Value, key: &str) -> Result<Option<bool>, Reply> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(err(400, format!("поле {key} должно быть true/false"))),
    }
}

/// Числовое поле с границами; границы попадают в текст ошибки.
fn field_u64(value: &Value, key: &str, min: u64, max: u64) -> Result<Option<u64>, Reply> {
    let Some(raw) = value.get(key) else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let number = raw
        .as_u64()
        .ok_or_else(|| err(400, format!("поле {key} должно быть целым числом")))?;
    if number < min || number > max {
        return Err(err(
            400,
            format!("поле {key}: допустимо от {min} до {max} ({number})"),
        ));
    }
    Ok(Some(number))
}

/// Имя провайдера — оно же ключ `providers.<name>` в `secrets.json`.
fn check_provider(name: &str) -> Result<(), Reply> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(err(
            400,
            format!("недопустимое имя провайдера {name:?}: буквы, цифры, '-', '_', '.' (до 64)"),
        ))
    }
}

/// Строка без управляющих символов (модель, ключ, команда, адрес API).
fn check_line(field: &str, text: &str, max: usize) -> Result<(), Reply> {
    if text.chars().any(char::is_control) {
        return Err(err(
            400,
            format!("в поле {field} есть управляющие символы или перевод строки"),
        ));
    }
    let len = text.chars().count();
    if len > max {
        return Err(err(
            400,
            format!("поле {field} длиннее {max} символов ({len})"),
        ));
    }
    Ok(())
}

/// Разобранное тело `POST /api/agent/settings`.
#[derive(Debug, Default)]
struct SettingsRequest {
    project: String,
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    /// Правки `config.yaml`: путь внутри `agent` и готовый YAML-литерал.
    config: Vec<(&'static str, String)>,
    /// Что меняется — для ответа `applied`.
    applied: Vec<String>,
    /// Нужен новый процесс: ключ, провайдер, модель, фильтр, MCP, таймауты.
    reopen: bool,
}

impl SettingsRequest {
    /// Ни одного поля: 400, а не пустая запись.
    fn is_empty(&self) -> bool {
        self.provider.is_none()
            && self.model.is_none()
            && self.api_key.is_none()
            && self.base_url.is_none()
            && self.config.is_empty()
    }
}

/// Разбор тела `POST /api/agent/settings`; все проверки — до записи файлов.
///
/// Своих регулярок нет: только сравнения строк и подсчёт символов (AG-1).
fn settings_request(body: &str) -> Result<SettingsRequest, Reply> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| err(400, format!("тело запроса не JSON: {e}")))?;
    let project = field_str(&value, "project")?.unwrap_or_default();
    if project.is_empty() {
        return Err(err(400, "нужно поле project"));
    }
    let mut request = SettingsRequest {
        project,
        ..SettingsRequest::default()
    };

    if let Some(provider) = field_str(&value, "provider")? {
        check_provider(&provider)?;
        request.provider = Some(provider);
        request.applied.push("provider".to_string());
        request.reopen = true;
    }
    if let Some(model) = field_str(&value, "model")? {
        if model.is_empty() {
            return Err(err(400, "модель не может быть пустой"));
        }
        check_line("model", &model, 200)?;
        request.model = Some(model);
        request.applied.push("model".to_string());
        request.reopen = true;
    }
    if let Some(key) = field_str(&value, "api_key")? {
        if key.is_empty() {
            return Err(err(
                400,
                "пустой ключ: введите значение или не отправляйте поле api_key",
            ));
        }
        check_line("api_key", &key, 4096)?;
        request.api_key = Some(key);
        request.applied.push("api_key".to_string());
        request.reopen = true;
    }
    if let Some(url) = field_str(&value, "base_url")? {
        if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(err(
                400,
                "base_url должен начинаться с http:// или https:// (пусто — база по умолчанию)",
            ));
        }
        check_line("base_url", &url, 500)?;
        request.base_url = Some(url);
        request.applied.push("base_url".to_string());
        request.reopen = true;
    }

    // Общие настройки — в `config.yaml` (TR-8.3). Каждая правка означает новый
    // процесс: план сессии и MCP собираются при открытии шлюза.
    if let Some(flag) = field_bool(&value, "enabled")? {
        request.config.push(("enabled", yaml_bool(flag)));
        request.applied.push("enabled".to_string());
        request.reopen = true;
    }
    if let Some(flag) = field_bool(&value, "preview_outgoing")? {
        request.config.push(("preview_outgoing", yaml_bool(flag)));
        request.applied.push("preview_outgoing".to_string());
    }
    if let Some(flag) = field_bool(&value, "subagents")? {
        request.config.push(("subagents", yaml_bool(flag)));
        request.applied.push("subagents".to_string());
        request.reopen = true;
    }
    if let Some(flag) = field_bool(&value, "egress_log")? {
        request.config.push(("egress_log", yaml_bool(flag)));
        request.applied.push("egress_log".to_string());
        request.reopen = true;
    }
    if let Some(policy) = field_str(&value, "unknown")? {
        let policy = policy.to_ascii_lowercase();
        if !matches!(policy.as_str(), "label" | "block") {
            return Err(err(400, "поле unknown: допустимо 'label' или 'block'"));
        }
        request.config.push(("filter.unknown", yaml_str(&policy)));
        request.applied.push("unknown".to_string());
        request.reopen = true;
    }
    if let Some(root) = field_str(&value, "root")? {
        if root.is_empty() {
            return Err(err(400, "корень зоны не может быть пустым"));
        }
        if !crate::config_edit::looks_abs(&root) {
            return Err(err(400, "корень зоны должен быть абсолютным путём"));
        }
        if !Path::new(&root).is_dir() {
            return Err(err(400, format!("папка зоны не найдена: {root}")));
        }
        request.config.push(("root", yaml_str(&root)));
        request.applied.push("root".to_string());
        request.reopen = true;
    }
    if let Some(n) = field_u64(&value, "max_live_projects", 1, SETTINGS_MAX_LIVE)? {
        request.config.push(("max_live_projects", n.to_string()));
        request.applied.push("max_live_projects".to_string());
        request.reopen = true;
    }
    if let Some(n) = field_u64(
        &value,
        "request_timeout_sec",
        SETTINGS_MIN_TIMEOUT_SEC,
        3600,
    )? {
        request.config.push(("request_timeout_sec", n.to_string()));
        request.applied.push("request_timeout_sec".to_string());
        request.reopen = true;
    }
    if let Some(n) = field_u64(&value, "ask_timeout_sec", SETTINGS_MIN_ASK_SEC, 3600)? {
        request.config.push(("ask_timeout_sec", n.to_string()));
        request.applied.push("ask_timeout_sec".to_string());
        request.reopen = true;
    }
    if let Some(n) = field_u64(
        &value,
        "filter_max_stale_sec",
        SETTINGS_MIN_STALE_SEC,
        86_400,
    )? {
        request.config.push(("filter.max_stale_sec", n.to_string()));
        request.applied.push("filter_max_stale_sec".to_string());
        request.reopen = true;
    }
    if let Some(proxy) = field_str(&value, "proxy")? {
        check_line("proxy", &proxy, 500)?;
        request.config.push(("network.proxy", yaml_str(&proxy)));
        request.applied.push("proxy".to_string());
        request.reopen = true;
    }
    if let Some(flag) = field_bool(&value, "disk_search_enabled")? {
        request
            .config
            .push(("mcp.disk_search.enabled", yaml_bool(flag)));
        request.applied.push("disk_search_enabled".to_string());
        request.reopen = true;
    }
    if let Some(flag) = field_bool(&value, "onec_enabled")? {
        request
            .config
            .push(("mcp.onec_buddy.enabled", yaml_bool(flag)));
        request.applied.push("onec_enabled".to_string());
        request.reopen = true;
    }
    if let Some(command) = field_str(&value, "onec_command")? {
        check_line("onec_command", &command, 500)?;
        request
            .config
            .push(("mcp.onec_buddy.command", yaml_str(&command)));
        request.applied.push("onec_command".to_string());
        request.reopen = true;
    }
    if let Some(flag) = field_bool(&value, "tavily_enabled")? {
        request.config.push(("mcp.tavily.enabled", yaml_bool(flag)));
        request.applied.push("tavily_enabled".to_string());
        request.reopen = true;
    }
    if let Some(command) = field_str(&value, "tavily_command")? {
        check_line("tavily_command", &command, 500)?;
        request
            .config
            .push(("mcp.tavily.command", yaml_str(&command)));
        request.applied.push("tavily_command".to_string());
        request.reopen = true;
    }
    Ok(request)
}

/// Тело панели настроек: проект, ключи (только «задан/нет»), секция `agent:`,
/// живые процессы и пути файлов.
fn settings_body(cfg: &Config, rec: &projects::Project, secrets: &Secrets) -> Value {
    let agent = settings::settings(cfg);
    let provider = if !rec.provider.trim().is_empty() {
        rec.provider.trim().to_string()
    } else {
        secrets
            .project_provider(&rec.id)
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string())
    };
    let model = if !rec.model.trim().is_empty() {
        rec.model.trim().to_string()
    } else {
        secrets.project_model(&rec.id).unwrap_or_default()
    };
    let providers: Vec<Value> = secrets
        .provider_names()
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "key": { "set": secrets.has_provider_key(name) },
                "base_url": secrets.provider_base_url(name),
            })
        })
        .collect();
    let live: Vec<Value> = agent_session::global(cfg)
        .live()
        .iter()
        .map(|(project, idle)| json!({ "project": project, "idle_sec": idle }))
        .collect();
    json!({
        "project": {
            "id": rec.id,
            "name": rec.name,
            "provider": provider,
            "model": model,
            // Маски значения нет: только признак (В-A5.6-2).
            "key": { "set": secrets.has_provider_key(&provider) },
            "base_url": secrets.provider_base_url(&provider),
        },
        "providers": providers,
        "config": {
            "enabled": agent.enabled,
            "root": agent.root.as_ref().map(|p| p.display().to_string()),
            "preview_outgoing": agent.preview_outgoing,
            "subagents": agent.subagents,
            "unknown": agent.unknown.as_str(),
            "egress_log": agent.egress_log,
            "max_live_projects": agent.max_live_projects,
            "request_timeout_sec": agent.request_timeout_secs,
            "ask_timeout_sec": agent.ask_timeout_secs,
            "filter_max_stale_sec": agent.filter_max_stale_secs,
            "proxy": agent.proxy,
            "disk_search_enabled": agent.disk_search_enabled,
            "onec_enabled": agent.onec_enabled,
            "onec_command": agent.onec_command,
            "tavily_enabled": agent.tavily_enabled,
            "tavily_command": agent.tavily_command,
        },
        "live": live,
        "config_path": config::config_path().display().to_string(),
        "secrets_path": settings::secrets_file(cfg).display().to_string(),
    })
}

/// `GET /api/agent/settings?project=<P>` — панель «Настройки агента».
///
/// Ключи отдаются признаком «задан/нет»; значения не попадают ни в ответ, ни
/// в журналы (TR-8.3, I8).
pub fn settings_json(cfg: &Config, project: &str) -> Reply {
    let id = project.trim();
    if id.is_empty() {
        return err(400, "нет параметра project");
    }
    let Some(rec) = projects::get(cfg, id) else {
        return err(404, format!("проект {id} не зарегистрирован"));
    };
    let secrets = match Secrets::load_or_empty(&settings::secrets_file(cfg)) {
        Ok(secrets) => secrets,
        Err(e) => return err(500, e.to_string()),
    };
    ok(settings_body(cfg, &rec, &secrets))
}

/// `POST /api/agent/settings` `{project, provider?, model?, api_key?, …}`.
///
/// Порядок: проверки → `projects.json` (провайдер и модель проекта) →
/// `secrets.json` (ключ; только этот файл, TR-8.3) → `config.yaml` (общие
/// настройки, редактор сохраняет комментарии) → `unload` шлюза и `notice`.
pub fn settings_save_json(cfg: &Config, body: &str) -> Reply {
    let request = match settings_request(body) {
        Ok(request) => request,
        Err(reply) => return reply,
    };
    if request.is_empty() {
        return err(
            400,
            "нечего менять: укажите provider/model/api_key или настройки agent.*",
        );
    }
    let Some(mut rec) = projects::get(cfg, &request.project) else {
        return err(
            404,
            format!("проект {} не зарегистрирован", request.project),
        );
    };
    let mut changed = false;
    if let Some(provider) = &request.provider {
        if rec.provider != *provider {
            rec.provider = provider.clone();
            changed = true;
        }
    }
    if let Some(model) = &request.model {
        if rec.model != *model {
            rec.model = model.clone();
            changed = true;
        }
    }
    if changed {
        let mut registry = match projects::load(cfg) {
            Ok(registry) => registry,
            Err(e) => return err(500, e),
        };
        registry.upsert(rec.clone());
        if let Err(e) = projects::save(cfg, &registry) {
            return err(500, e);
        }
    }

    let provider_name = rec.provider.trim().to_string();
    let mut secrets = match Secrets::load_or_empty(&settings::secrets_file(cfg)) {
        Ok(secrets) => secrets,
        Err(e) => return err(500, e.to_string()),
    };
    if let Some(key) = &request.api_key {
        secrets.set_provider_key(&provider_name, key);
    }
    if let Some(url) = &request.base_url {
        secrets.set_provider_base_url(&provider_name, url);
    }
    if request.api_key.is_some() || request.base_url.is_some() {
        if let Err(e) = secrets.save() {
            return err(500, format!("ключ не сохранён: {e}"));
        }
    }
    if !request.config.is_empty() {
        let values: Vec<(&str, &str)> = request
            .config
            .iter()
            .map(|(path, literal)| (*path, literal.as_str()))
            .collect();
        if let Err(e) = crate::config_edit::set_agent_values(&values) {
            return err(500, format!("config.yaml: {e}"));
        }
    }

    // Конфиг читаем заново: ответ должен показывать то, что записано на диске.
    let fresh = config::load().unwrap_or_else(|_| cfg.clone());
    let api = agent_session::global(&fresh);
    let unloaded = if request.reopen {
        api.unload(&rec.id)
    } else {
        false
    };
    if request.reopen {
        api.notice(
            &rec.id,
            "настройки применены — следующий ход откроет сессию заново",
        );
    }
    let mut warnings: Vec<String> = Vec::new();
    if request
        .applied
        .iter()
        .any(|field| field == "max_live_projects")
    {
        warnings.push("max_live_projects применится после перезапуска hds ui".to_string());
    }
    let secrets_after = Secrets::load_or_empty(&settings::secrets_file(&fresh)).unwrap_or(secrets);
    let mut body = settings_body(&fresh, &rec, &secrets_after);
    if let Some(map) = body.as_object_mut() {
        map.insert("ok".to_string(), json!(true));
        map.insert("applied".to_string(), json!(request.applied));
        map.insert("reopened".to_string(), json!(request.reopen));
        map.insert("unloaded".to_string(), json!(unloaded));
        map.insert("warnings".to_string(), json!(warnings));
    }
    ok(body)
}

/// Разбор тела `POST /api/agent/history/clear`: `(project, session?)`.
///
/// Идентификатор сессии проверяется `hds_agent::history::valid_session_id`
/// (буквы/цифры/`-`/`_`, до 128): он подставляется в путь файлов истории.
fn history_request(body: &str) -> Result<(String, Option<String>), Reply> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| err(400, format!("тело запроса не JSON: {e}")))?;
    let project = field_str(&value, "project")?.unwrap_or_default();
    if project.is_empty() {
        return Err(err(400, "нужно поле project"));
    }
    let session = match value.get("session") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => {
            let id = id.trim();
            if id.is_empty() {
                None
            } else if hds_agent::history::valid_session_id(id) {
                Some(id.to_string())
            } else {
                return Err(err(
                    400,
                    format!("недопустимый идентификатор сессии {id:?}: буквы, цифры, '-' и '_' (до 128)"),
                ));
            }
        }
        Some(_) => return Err(err(400, "поле session должно быть строкой")),
    };
    Ok((project, session))
}

/// Ошибка `run::clear_history` → HTTP-ответ (A5.6).
fn history_err(message: &str) -> Reply {
    if message.contains("недопустимый идентификатор") {
        err(400, message)
    } else if message.contains("не зарегистрирован")
        || message.contains("нет состояния проекта")
        || message.contains("не найден")
        || message.contains("укажите --session")
    {
        err(404, message)
    } else if message.contains("словарь недоступен") || message.contains("воркер анонимизации")
    {
        err(503, message)
    } else {
        err(500, message)
    }
}

/// `POST /api/agent/history/clear` `{project, session?}` — TR-6.6.
///
/// Порядок: если ход идёт — `stop` (не удалось — 409), затем `unload` шлюза
/// (SDK не должен держать файлы сессии), затем `run::clear_history` (тот же
/// путь, что у CLI: словарь фильтра и бэкапы) и `notice` в ленту. В ответе —
/// `{session, files, backups, replaced}` без значений (В-A5.6-4).
pub fn history_clear_json(cfg: &Config, body: &str) -> Reply {
    let (project, session) = match history_request(body) {
        Ok(parts) => parts,
        Err(reply) => return reply,
    };
    let Some(rec) = projects::get(cfg, &project) else {
        return err(404, format!("проект {project} не зарегистрирован"));
    };
    if !settings::settings(cfg).enabled {
        return err(
            503,
            "анонимная зона выключена (agent.enabled: false) — очистка истории недоступна",
        );
    }
    let api = agent_session::global(cfg);
    if let Some(turn) = api.existing_turn(&rec.id) {
        if turn.is_busy() {
            if let Err(e) = api.stop(cfg, &rec.id) {
                return err(
                    409,
                    format!(
                        "ход идёт и остановить его не удалось: {} — нажмите «Стоп» и повторите",
                        e.message
                    ),
                );
            }
        }
    }
    let unloaded = api.unload(&rec.id);
    let root = hds_core::config::project_root();
    let (session, outcome) =
        match hds_agent::run::clear_history(cfg, &rec.id, session.as_deref(), &root) {
            Ok(result) => result,
            Err(message) => return history_err(&message),
        };
    let files: Vec<String> = outcome
        .files
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let backups: Vec<String> = outcome
        .backups
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    api.notice(
        &rec.id,
        &format!(
            "история очищена: сессия {session}, файлов {}, замен {}, бэкапов {} — \
             значения в истории заменены метками",
            files.len(),
            outcome.replaced,
            backups.len()
        ),
    );
    ok(json!({
        "ok": true,
        "session": session,
        "files": files,
        "backups": backups,
        "files_count": files.len(),
        "backups_count": backups.len(),
        "replaced": outcome.replaced,
        "unloaded": unloaded,
    }))
}

/// Последние `n` строк файла; нет файла или слишком большой — пусто.
fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    if meta.len() > JOURNAL_MAX_BYTES {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("hds-ui-agent-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn project_ids_follow_d21() {
        assert!(is_project_id("P0001"));
        assert!(is_project_id("P9999"));
        for bad in [
            "p0001", "P001", "P00011", "P00a1", "P", "", "0001", "P0001 ",
        ] {
            assert!(!is_project_id(bad), "{bad}");
        }
    }

    #[test]
    fn next_id_takes_first_free() {
        assert_eq!(next_id(std::iter::empty()), "P0001");
        assert_eq!(next_id(["P0001", "P0003"].into_iter()), "P0004");
        assert_eq!(next_id(["мусор", "P0002"].into_iter()), "P0003");
    }

    #[test]
    fn tail_lines_returns_last_n() {
        let dir = temp_dir("tail");
        let path = dir.join("sync.log");
        std::fs::write(&path, "a\nb\nc\n").unwrap();
        assert_eq!(tail_lines(&path, 2), vec!["b".to_string(), "c".to_string()]);
        assert_eq!(tail_lines(&path, 10).len(), 3);
        assert!(tail_lines(&dir.join("нет.log"), 5).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Кадр SSE: `id:`/`event: item`/`data:` одной строкой и пустая строка в
    /// конце; переводы строк внутри текста не рвут кадр (их экранирует JSON).
    #[test]
    fn sse_frame_has_id_event_and_blank_line() {
        let item = json!({ "seq": 7, "kind": "agent", "text": { "text": "строка\nвторая" } });
        let frame = sse_frame(7, &item);
        assert!(frame.starts_with("id: 7\nevent: item\ndata: "), "{frame}");
        assert!(frame.ends_with("\n\n"), "{frame:?}");
        // Ровно одна строка `data:` — иначе SSE-клиент соберёт событие из двух.
        assert_eq!(
            frame.lines().filter(|l| l.starts_with("data: ")).count(),
            1,
            "{frame:?}"
        );
        let data = frame
            .lines()
            .find_map(|l| l.strip_prefix("data: "))
            .expect("data");
        let parsed: Value = serde_json::from_str(data).expect("JSON одного события");
        assert_eq!(parsed["seq"], 7);
        assert_eq!(parsed["text"]["text"], "строка\nвторая");
    }

    #[test]
    fn sse_state_frame_carries_state() {
        let state = json!({ "busy": false, "session": Value::Null, "seq": 3 });
        assert_eq!(
            sse_state_frame(&state),
            "event: state\ndata: {\"busy\":false,\"seq\":3,\"session\":null}\n\n"
        );
    }

    #[test]
    fn wants_stream_reads_accept() {
        let with = |accept: Option<&str>| ReqHeaders {
            accept: accept.map(str::to_string),
            ..ReqHeaders::default()
        };
        assert!(wants_stream(&with(Some("text/event-stream"))));
        assert!(wants_stream(&with(Some(
            "text/event-stream, application/json;q=0.9"
        ))));
        assert!(wants_stream(&with(Some("TEXT/EVENT-STREAM"))));
        assert!(!wants_stream(&with(Some("application/json"))));
        assert!(!wants_stream(&with(None)));
    }

    #[test]
    fn stream_after_prefers_last_event_id() {
        let h = |id: Option<&str>| ReqHeaders {
            last_event_id: id.map(str::to_string),
            ..ReqHeaders::default()
        };
        // Переподключение: `Last-Event-ID` важнее `?after=`.
        assert_eq!(stream_after(&h(Some("42")), "after=7"), 42);
        // Первое подключение: заголовка нет — берём `?after=`.
        assert_eq!(stream_after(&h(None), "after=7&limit=500"), 7);
        // Ни того, ни другого — с нуля.
        assert_eq!(stream_after(&h(None), ""), 0);
        // Мусор в заголовке не ломает поток — падаем на query.
        assert_eq!(stream_after(&h(Some("abc")), "after=5"), 5);
    }

    /// A5.4: `ResultsError` → код и текст HTTP-ответа вкладки.
    #[test]
    fn results_errors_map_to_http_codes() {
        let code = |e: ResultsError| {
            let (status, _, body) = results_err(e);
            let parsed: Value = serde_json::from_str(&body).expect("JSON ошибки");
            (status, parsed["error"].as_str().unwrap_or("").to_string())
        };
        assert_eq!(code(ResultsError::NotFound("Письмо.md".into())).0, 404);
        assert_eq!(code(ResultsError::NoVariant("Письмо.md".into())).0, 404);
        assert_eq!(
            code(ResultsError::WrongStatus {
                rel: "Письмо.md".into(),
                status: Status::Accepted,
            })
            .0,
            409
        );
        let (status, message) = code(ResultsError::OriginalChanged("Письмо.md".into()));
        assert_eq!(status, 409);
        assert!(message.contains("оригинал изменился"), "{message}");
        assert!(message.contains("conflict --choice"), "{message}");
        // Занятый файл — текст A4: что закрыть и что не потеряно.
        let (status, message) = code(ResultsError::Busy(PathBuf::from("C:/Проект/Письмо.docx")));
        assert_eq!(status, 409);
        assert!(message.contains("файл занят"), "{message}");
        assert!(message.contains("вариант остался в pending"), "{message}");
        assert!(message.contains("Письмо.docx"), "{message}");
        assert_eq!(code(ResultsError::BadPath("../вне.md".into())).0, 400);
        assert_eq!(
            code(ResultsError::Kind(hds_agent::registry_db::Kind::TextImage)).0,
            409
        );
        assert_eq!(
            code(ResultsError::Io(PathBuf::from("x"), "нет доступа".into())).0,
            500
        );
    }

    /// A5.5: разбор тела `merge` — «один тип», минимум две метки, коды 400.
    #[test]
    fn merge_request_accepts_one_type_and_rejects_bad_bodies() {
        let status = |body: &str| match merge_request(body) {
            Ok(_) => 200,
            Err((status, _, _)) => status,
        };
        let (project, keep, drop) = merge_request(
            r#"{"project":"P0001","keep":"[PERSON_1]","drop":["[PERSON_2]","[PERSON_3]"]}"#,
        )
        .expect("корректное тело");
        assert_eq!(project, "P0001");
        assert_eq!(keep, "[PERSON_1]");
        assert_eq!(
            drop,
            vec!["[PERSON_2]".to_string(), "[PERSON_3]".to_string()]
        );
        // `keep` в `drop` и повторы отбрасываются
        let (_, _, drop) = merge_request(
            r#"{"project":"P0001","keep":"[PERSON_1]","drop":["[PERSON_1]","[PERSON_2]","[PERSON_2]"]}"#,
        )
        .expect("повтор не мешает");
        assert_eq!(drop, vec!["[PERSON_2]".to_string()]);

        assert_eq!(status("не json"), 400);
        assert_eq!(status("{}"), 400);
        assert_eq!(
            status(r#"{"project":"","keep":"[PERSON_1]","drop":["[PERSON_2]"]}"#),
            400
        );
        assert_eq!(
            status(r#"{"project":"P0001","keep":"","drop":["[PERSON_2]"]}"#),
            400
        );
        assert_eq!(status(r#"{"project":"P0001","keep":"[PERSON_1]"}"#), 400);
        // метка не разобрана (нет скобок) и разные типы
        assert_eq!(
            status(r#"{"project":"P0001","keep":"PERSON_1","drop":["[PERSON_2]"]}"#),
            400
        );
        assert_eq!(
            status(r#"{"project":"P0001","keep":"[PERSON_1]","drop":["[ORG_2]"]}"#),
            400
        );
        // одна и та же метка — склеивать нечего
        assert_eq!(
            status(r#"{"project":"P0001","keep":"[PERSON_1]","drop":["[PERSON_1]"]}"#),
            400
        );
    }

    /// A5.5: `merge_id` принимается числом и строкой, мусор — 400 на входе.
    #[test]
    fn unmerge_merge_id_parses_number_and_string() {
        let value = |text: &str| -> serde_json::Value { serde_json::from_str(text).unwrap() };
        assert_eq!(merge_id_of(&value(r#"{"merge_id":7}"#)), Some(7));
        assert_eq!(merge_id_of(&value(r#"{"merge_id":" 7 "}"#)), Some(7));
        assert_eq!(merge_id_of(&value(r#"{"merge_id":"семь"}"#)), None);
        assert_eq!(merge_id_of(&value("{}")), None);
    }

    /// A5.6: разбор `POST /api/agent/settings` — 400 на всё, что не проходит
    /// проверку до записи файлов.
    #[test]
    fn settings_request_validates_body() {
        let status = |body: &str| match settings_request(body) {
            Ok(_) => 200,
            Err((status, _, _)) => status,
        };
        let ok = settings_request(
            r#"{"project":"P0001","model":"mock-model","api_key":"sk-test-123456",
                "unknown":"block","ask_timeout_sec":601,"egress_log":true}"#,
        )
        .expect("корректное тело");
        assert_eq!(ok.project, "P0001");
        assert_eq!(ok.model.as_deref(), Some("mock-model"));
        assert_eq!(ok.api_key.as_deref(), Some("sk-test-123456"));
        assert!(ok.reopen, "ключ/модель/фильтр — новый процесс");
        assert_eq!(
            ok.config,
            vec![
                ("egress_log", "true".to_string()),
                ("filter.unknown", "'block'".to_string()),
                ("ask_timeout_sec", "601".to_string()),
            ]
        );
        // Только `preview_outgoing` — процесс перезапускать не нужно.
        let quiet =
            settings_request(r#"{"project":"P0001","preview_outgoing":false}"#).expect("корректно");
        assert!(!quiet.reopen);
        assert_eq!(
            quiet.config,
            vec![("preview_outgoing", "false".to_string())]
        );

        assert_eq!(status("не json"), 400);
        assert_eq!(status("{}"), 400);
        assert_eq!(status(r#"{"project":""}"#), 400);
        assert_eq!(status(r#"{"project":7}"#), 400);
        // Тело разбирается, но менять нечего — 400 отдаёт маршрут (`is_empty`).
        assert!(settings_request(r#"{"project":"P0001"}"#)
            .unwrap()
            .is_empty());
        assert_eq!(status(r#"{"project":"P0001","api_key":""}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","api_key":"a\nb"}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","model":"  "}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","provider":"../evil"}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","unknown":"keep"}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","egress_log":"да"}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","ask_timeout_sec":5}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","max_live_projects":99}"#), 400);
        assert_eq!(
            status(r#"{"project":"P0001","filter_max_stale_sec":"300"}"#),
            400
        );
        assert_eq!(status(r#"{"project":"P0001","base_url":"ftp://x"}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","root":"relative"}"#), 400);
    }

    /// A5.6: разбор `POST /api/agent/history/clear` — 400 на плохую сессию.
    #[test]
    fn history_request_validates_session() {
        let status = |body: &str| match history_request(body) {
            Ok(_) => 200,
            Err((status, _, _)) => status,
        };
        let (project, session) =
            history_request(r#"{"project":"P0001","session":"S-OLD_1"}"#).expect("корректно");
        assert_eq!(project, "P0001");
        assert_eq!(session.as_deref(), Some("S-OLD_1"));
        let (_, session) = history_request(r#"{"project":"P0001","session":"  "}"#).unwrap();
        assert!(session.is_none(), "пустая сессия — последняя");
        let (_, session) = history_request(r#"{"project":"P0001"}"#).unwrap();
        assert!(session.is_none());

        assert_eq!(status("не json"), 400);
        assert_eq!(status("{}"), 400);
        assert_eq!(status(r#"{"project":"P0001","session":"..\\..\\x"}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","session":"a/b"}"#), 400);
        assert_eq!(status(r#"{"project":"P0001","session":7}"#), 400);
    }
}
