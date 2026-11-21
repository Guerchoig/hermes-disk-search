//! Маршрут агента `POST <mcp-path>/agent/<P>` (TR-4.3, TR-4.4, D9).
//!
//! Контракт:
//! * токен проекта лежит **не** в `data/access.yaml`, а в
//!   `data/agent/secrets.json` (хэш sha256, как у пользовательских токенов):
//!   `{"projects": {"P0001": {"token_sha256": "<hex>", "expires": 1798761600}}}`;
//!   `expires: 0` — токен бессрочный (MVP: агент проекта живёт, пока жив проект);
//! * неверный/просроченный токен или чужой `P` → 401 (решает `http::route`);
//! * на маршруте виден **только** свой проект: контекст [`project_ctx`] даёт
//!   `projects = [P]`, поэтому `search_local_files` фильтрует выдачу по проекту
//!   и в FTS, и в векторной ветке, и в CLIP (инвариант I5);
//! * `ask_my_files` и инструменты управления индексацией на маршруте не
//!   публикуются (TR-4.4) — это делает `schema::tool_list_scoped`.
//!
//! Путь хранения: `<state_dir>/data/agent/secrets.json`. Переменная окружения
//! `HDS_AGENT_SECRETS` перекрывает путь — ею пользуются тесты и стенды, чтобы
//! не писать в каталог состояния рабочей установки.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use hds_core::access::{token_matches, AuthCtx};
use hds_core::config::{dig, replace_file, Config};
use hds_core::roots::is_agent_project;

/// Токен проекта анонимной зоны.
///
/// В файле лежат **и** хэш (для сверки маршрутом `/mcp/agent/<P>`), **и**
/// plain-токен: агенту нужен `Authorization: Bearer …` в
/// `cline_mcp_settings.json`, а собрать его из хэша нельзя. Файл защищён ACL
/// «только владелец» (В-1), как и остальные секреты.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentToken {
    /// hex-строка sha256(plain-токен).
    pub token_sha256: String,
    /// Истечение, unix-секунды; `0` — бессрочный.
    #[serde(default)]
    pub expires: i64,
    /// Plain-токен для настроек MCP агента (`None` — выдан прежней версией,
    /// только хэш: маршрут работает, `disk-search` у агента — нет).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Хранилище токенов проектов (`data/agent/secrets.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSecrets {
    /// `P0001 → {token_sha256, expires}`.
    #[serde(default)]
    pub projects: BTreeMap<String, AgentToken>,
}

/// Путь `data/agent/secrets.json` для готового конфига.
///
/// `HDS_AGENT_SECRETS` (тесты/стенды) → иначе `<state_dir>/data/agent/secrets.json`.
pub fn secrets_path_from(cfg: &Config) -> PathBuf {
    if let Some(p) = std::env::var_os("HDS_AGENT_SECRETS") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    hds_core::config::state_dir(cfg)
        .join("data")
        .join("agent")
        .join("secrets.json")
}

/// Загрузка хранилища; отсутствие файла — пустое (токенов ещё не выдавали).
pub fn load_at(path: &Path) -> Result<AgentSecrets, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AgentSecrets::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Атомарное сохранение (временный файл рядом + `replace_file`).
///
/// **AG-22.** Файл общий: в нём же лежат `providers` (A5.6), `mcp` и
/// `mcp.servers`. Слепая сериализация `AgentSecrets` (только `projects`)
/// стирала их — поэтому новое состояние **сливается** с тем, что уже на диске:
/// чужие блоки сохраняются, наши ключи сильнее.
pub fn save_at(path: &Path, secrets: &AgentSecrets) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut merged = match std::fs::read_to_string(path) {
        Ok(text) => {
            match serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) {
                Ok(value) if value.is_object() => value,
                Ok(_) => serde_json::Value::Object(serde_json::Map::new()),
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(serde_json::Map::new())
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let ours = serde_json::to_value(secrets).map_err(|e| e.to_string())?;
    merge_json(&mut merged, &ours);
    let text = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    replace_file(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.message()
    })
}

/// Слияние JSON: объекты сливаются рекурсивно, остальное перекрывается.
fn merge_json(target: &mut serde_json::Value, source: &serde_json::Value) {
    match (target, source) {
        (serde_json::Value::Object(into), serde_json::Value::Object(from)) => {
            for (key, value) in from {
                merge_json(
                    into.entry(key.clone()).or_insert(serde_json::Value::Null),
                    value,
                );
            }
        }
        (slot, value) => *slot = value.clone(),
    }
}

/// Проект маршрута из пути запроса: `<mp>/agent/P0001` → `Some("P0001")`.
///
/// Принимается только формат `P` + 4 цифры (D21) — тот же, что у каталогов
/// зоны: иначе агенту достался бы «проект», которого нет в индексе.
pub fn project_route<'a>(req_path: &'a str, mcp_path: &str) -> Option<&'a str> {
    let p = req_path
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let prefix = format!("{}/agent/", mcp_path.trim_end_matches('/'));
    let project = p.strip_prefix(&prefix)?;
    is_agent_project(project).then_some(project)
}

/// Проверка токена проекта: хэш совпал и срок не истёк (0 — бессрочный).
pub fn token_ok(secrets: &AgentSecrets, project: &str, plain: &str, now_unix: i64) -> bool {
    let Some(tok) = secrets.projects.get(project) else {
        return false;
    };
    if tok.expires != 0 && tok.expires <= now_unix {
        return false;
    }
    token_matches(plain, &tok.token_sha256)
}

/// Контекст авторизации маршрута агента: `projects = [P]` **всегда**.
///
/// `admin: false` — чтобы `project_filter` не отключил фильтр, а управляющие
/// инструменты (если бы их кто-то вызвал напрямую) отказали.
pub fn project_ctx(project: &str) -> AuthCtx {
    AuthCtx {
        user: format!("agent:{project}"),
        admin: false,
        projects: vec![project.to_string()],
    }
}

/// Режим агента включён (`agent.enabled`) — нужен для 404 на маршруте.
pub fn agent_mode(cfg: &Config) -> bool {
    dig(cfg, "agent.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(json: &str) -> Config {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn route_project_parses_only_pnnnn() {
        assert_eq!(project_route("/mcp/agent/P0001", "/mcp"), Some("P0001"));
        assert_eq!(project_route("/mcp/agent/P0001/", "/mcp"), Some("P0001"));
        assert_eq!(project_route("/mcp/agent/P0001?x=1", "/mcp"), Some("P0001"));
        // Не проект: короткий номер, буквы, без префикса, лишний сегмент.
        assert_eq!(project_route("/mcp/agent/P001", "/mcp"), None);
        assert_eq!(project_route("/mcp/agent/PXXXX", "/mcp"), None);
        assert_eq!(project_route("/mcp/agent/0001", "/mcp"), None);
        assert_eq!(project_route("/mcp/P0001", "/mcp"), None);
        assert_eq!(project_route("/mcp/agent/P0001/x", "/mcp"), None);
        // Нестандартный путь сервера.
        assert_eq!(
            project_route("/api/mcp/agent/P0002", "/api/mcp"),
            Some("P0002")
        );
    }

    #[test]
    fn token_ok_checks_hash_project_and_expiry() {
        let mut secrets = AgentSecrets::default();
        let (plain, token) = hds_core::access::new_token("P0001", 90, 1_700_000_000).unwrap();
        let at = AgentToken {
            token_sha256: token.token_sha256.clone(),
            expires: token.expires,
            token: None,
        };
        secrets.projects.insert("P0001".into(), at);
        assert!(token_ok(&secrets, "P0001", &plain, 1_700_000_000));
        // Чужой проект — отказ, даже с верным токеном.
        assert!(!token_ok(&secrets, "P0002", &plain, 1_700_000_000));
        // Просроченный токен.
        assert!(!token_ok(&secrets, "P0001", &plain, token.expires + 1));
        // Бессрочный (expires: 0).
        secrets.projects.insert(
            "P0003".into(),
            AgentToken {
                token_sha256: token.token_sha256.clone(),
                expires: 0,
                token: None,
            },
        );
        assert!(token_ok(&secrets, "P0003", &plain, i64::MAX / 2));
        // Подделка.
        assert!(!token_ok(&secrets, "P0001", "deadbeef", 1_700_000_000));
    }

    #[test]
    fn save_at_keeps_foreign_blocks() {
        // AG-22: файл общий с панелью настроек — `providers`/`mcp` не теряются
        // при выдаче токена, а неизвестные ключи переживают запись.
        let dir = std::env::temp_dir().join(format!(
            "hds-agent-token-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secrets.json");
        std::fs::write(
            &path,
            r#"{"providers":{"mock":{"api_key":"sk-test"}},"mcp":{"tavily":{"api_key":"tv"}},"future":1}"#,
        )
        .unwrap();
        let mut secrets = AgentSecrets::default();
        secrets.projects.insert(
            "P0001".into(),
            AgentToken {
                token_sha256: "hash".into(),
                expires: 0,
                token: Some("plain-token".into()),
            },
        );
        save_at(&path, &secrets).expect("запись");
        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value
                .pointer("/providers/mock/api_key")
                .and_then(|v| v.as_str()),
            Some("sk-test"),
            "ключ провайдера потерян: {text}"
        );
        assert_eq!(
            value
                .pointer("/mcp/tavily/api_key")
                .and_then(|v| v.as_str()),
            Some("tv")
        );
        assert_eq!(value.get("future").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(
            value
                .pointer("/projects/P0001/token_sha256")
                .and_then(|v| v.as_str()),
            Some("hash")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_ctx_filters_only_own_project() {
        let ctx = project_ctx("P0001");
        assert!(!ctx.admin, "admin отключил бы фильтр проектов");
        assert_eq!(ctx.projects, vec!["P0001".to_string()]);
        assert_eq!(
            crate::auth::project_filter(Some(&ctx)),
            Some(vec!["P0001".to_string()])
        );
        assert!(crate::auth::admin_required(Some(&ctx)).is_err());
    }

    #[test]
    fn secrets_roundtrip_and_missing_file() {
        let dir = std::env::temp_dir().join(format!(
            "hds-agent-secrets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("secrets.json");
        assert_eq!(load_at(&path).unwrap(), AgentSecrets::default());
        let mut s = AgentSecrets::default();
        s.projects.insert(
            "P0001".into(),
            AgentToken {
                token_sha256: "ab".repeat(32),
                expires: 0,
                token: None,
            },
        );
        save_at(&path, &s).unwrap();
        assert_eq!(load_at(&path).unwrap(), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secrets_path_honours_env_override() {
        let c = cfg(r#"{"agent": {"enabled": true}}"#);
        let prev = std::env::var_os("HDS_AGENT_SECRETS");
        std::env::set_var("HDS_AGENT_SECRETS", "C:/tmp/hds-secrets.json");
        assert_eq!(
            secrets_path_from(&c),
            PathBuf::from("C:/tmp/hds-secrets.json")
        );
        match prev {
            Some(v) => std::env::set_var("HDS_AGENT_SECRETS", v),
            None => std::env::remove_var("HDS_AGENT_SECRETS"),
        }
    }
}
