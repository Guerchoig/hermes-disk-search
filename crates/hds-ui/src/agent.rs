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

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::{projects, settings};
use hds_core::config::{self, Config, Profile};
use serde_json::{json, Value};

use crate::agent_session::{self, AgentApi, ApiError};
use crate::Reply;

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

/// `GET /api/agent/events?project=<P>&after=<seq>&limit=200` — события без SSE
/// (транспорт A5.3 добавит `text/event-stream` тем же маршрутом).
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
}
