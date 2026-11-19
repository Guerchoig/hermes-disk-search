//! Секция `agent:` конфига (TR-8.3) и производные пути проекта.
//!
//! Конфиг читается через `hds_core::config::dig` (как в `hds_core::roots`), а не
//! через типизированную структуру: секция `agent:` описана в `config.example.yaml`
//! и должна оставаться совместимой с конфигами прежних версий.

use std::path::{Path, PathBuf};

use hds_core::config::{dig, state_dir, Config};

/// Политика для `others` (формы прочих сущностей базы): `label` или `block`
/// (TR-6.2 б).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownPolicy {
    /// Выдать новую метку проекта через `label_for_value` и заменить значение.
    Label,
    /// Заблокировать запрос (`privacy_filter_unavailable`).
    Block,
}

impl UnknownPolicy {
    pub fn parse(s: &str) -> UnknownPolicy {
        match s.trim().to_ascii_lowercase().as_str() {
            "block" => UnknownPolicy::Block,
            _ => UnknownPolicy::Label,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            UnknownPolicy::Label => "label",
            UnknownPolicy::Block => "block",
        }
    }
}

/// Настройки агента из секции `agent:` (TR-8.3).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSettings {
    pub enabled: bool,
    pub root: Option<PathBuf>,
    pub preview_outgoing: bool,
    pub keep_terms: Vec<String>,
    pub subagents: bool,
    pub unknown: UnknownPolicy,
    pub proxy: Option<String>,
    pub egress_log: bool,
    pub max_live_projects: usize,
    pub idle_secs: u64,
    pub request_timeout_secs: u64,
    /// `agent.filter.max_stale_sec`: снимок vault старше — фильтр отказывает
    /// (fail-closed, TR-6.5). По умолчанию 300 с (В-14).
    pub filter_max_stale_secs: u64,
    pub node_path: Option<PathBuf>,
    pub disk_search_enabled: bool,
    pub onec_enabled: bool,
    pub tavily_enabled: bool,
    pub onec_command: Option<String>,
    pub tavily_command: Option<String>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        AgentSettings {
            enabled: false,
            root: None,
            preview_outgoing: true,
            keep_terms: Vec::new(),
            subagents: false,
            unknown: UnknownPolicy::Label,
            proxy: None,
            egress_log: false,
            max_live_projects: 2,
            idle_secs: 30 * 60,
            request_timeout_secs: 120,
            filter_max_stale_secs: 300,
            node_path: None,
            disk_search_enabled: true,
            onec_enabled: true,
            tavily_enabled: true,
            onec_command: None,
            tavily_command: None,
        }
    }
}

fn str_at(cfg: &Config, path: &str) -> Option<String> {
    dig(cfg, path)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn bool_at(cfg: &Config, path: &str, dflt: bool) -> bool {
    dig(cfg, path).and_then(|v| v.as_bool()).unwrap_or(dflt)
}

fn u64_at(cfg: &Config, path: &str, dflt: u64) -> u64 {
    dig(cfg, path)
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0)
        .unwrap_or(dflt)
}

fn list_at(cfg: &Config, path: &str) -> Vec<String> {
    let Some(v) = dig(cfg, path) else {
        return Vec::new();
    };
    if let Some(items) = v.as_sequence() {
        return items
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if let Some(s) = v.as_str() {
        return s
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    Vec::new()
}

/// Прочитать секцию `agent:` (TR-8.3) с дефолтами.
pub fn settings(cfg: &Config) -> AgentSettings {
    AgentSettings {
        enabled: bool_at(cfg, "agent.enabled", false),
        root: str_at(cfg, "agent.root").map(PathBuf::from),
        preview_outgoing: bool_at(cfg, "agent.preview_outgoing", true),
        keep_terms: list_at(cfg, "agent.keep_terms"),
        subagents: bool_at(cfg, "agent.subagents", false),
        unknown: UnknownPolicy::parse(&str_at(cfg, "agent.filter.unknown").unwrap_or_default()),
        proxy: str_at(cfg, "agent.network.proxy"),
        egress_log: bool_at(cfg, "agent.egress_log", false),
        max_live_projects: u64_at(cfg, "agent.max_live_projects", 2) as usize,
        idle_secs: u64_at(cfg, "agent.idle_unload_min", 30) * 60,
        request_timeout_secs: u64_at(cfg, "agent.request_timeout_sec", 120),
        filter_max_stale_secs: u64_at(cfg, "agent.filter.max_stale_sec", 300),
        node_path: str_at(cfg, "agent.node.path").map(PathBuf::from),
        disk_search_enabled: bool_at(cfg, "agent.mcp.disk_search.enabled", true),
        onec_enabled: bool_at(cfg, "agent.mcp.onec_buddy.enabled", true),
        tavily_enabled: bool_at(cfg, "agent.mcp.tavily.enabled", true),
        onec_command: str_at(cfg, "agent.mcp.onec_buddy.command"),
        tavily_command: str_at(cfg, "agent.mcp.tavily.command"),
    }
}

/// Корень зоны `<agent.root>`; `None`, если ключ не задан.
pub fn zone_root(cfg: &Config) -> Option<PathBuf> {
    settings(cfg).root
}

/// Каталог проекта `<P>` внутри зоны (TR-8.1: `P0001`).
pub fn project_dir(cfg: &Config, project: &str) -> Option<PathBuf> {
    zone_root(cfg).map(|r| r.join(project))
}

/// `workspace` проекта — область файловых инструментов агента (TR-3.4).
pub fn workspace_dir(cfg: &Config, project: &str) -> Option<PathBuf> {
    project_dir(cfg, project).map(|p| p.join("workspace"))
}

/// `agent-config` — HOME/APPDATA агента (TR-3.2).
pub fn config_dir(cfg: &Config, project: &str) -> Option<PathBuf> {
    project_dir(cfg, project).map(|p| p.join("agent-config"))
}

/// `agent-state` — каталог данных SDK и история (TR-3.2, TR-6.6).
pub fn state_dir_for(cfg: &Config, project: &str) -> Option<PathBuf> {
    project_dir(cfg, project).map(|p| p.join("agent-state"))
}

/// `projects.json` в зоне пользователя: `<state_dir>/data/agent/projects.json`.
pub fn projects_file(cfg: &Config) -> PathBuf {
    state_dir(cfg)
        .join("data")
        .join("agent")
        .join("projects.json")
}

/// Каталог реестров артефактов: `<state_dir>/data/agent/registry` (TR-7.2).
///
/// Реестр — зона пользователя: он хранит пути проекта (с настоящими именами),
/// поэтому в анонимную зону (`<agent.root>`) не попадает.
pub fn registry_dir(cfg: &Config) -> PathBuf {
    state_dir(cfg).join("data").join("agent").join("registry")
}

/// БД реестра артефактов проекта: `.../registry/<P>.db` (TR-7.2).
///
/// `None`, если `project` не похож на идентификатор проекта (`P` + 4 цифры,
/// D21): иначе значение вида `..\..\x` построило бы путь за пределы каталога.
pub fn registry_db(cfg: &Config, project: &str) -> Option<PathBuf> {
    if !hds_core::roots::is_agent_project(project) {
        return None;
    }
    Some(registry_dir(cfg).join(format!("{project}.db")))
}

/// Путь к `secrets.json` (провайдеры, MCP, токены) — `data\agent\secrets.json`.
/// `HDS_AGENT_SECRETS` перекрывает путь (как в `hds_mcp::agent::secrets_path_from`).
pub fn secrets_file(cfg: &Config) -> PathBuf {
    if let Some(p) = std::env::var_os("HDS_AGENT_SECRETS") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    state_dir(cfg)
        .join("data")
        .join("agent")
        .join("secrets.json")
}

/// Существует ли путь как каталог (мелкая утилита для сообщений об ошибках).
pub fn is_dir(p: &Path) -> bool {
    p.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::from_str;

    fn cfg(yaml: &str) -> Config {
        from_str(yaml).unwrap()
    }

    #[test]
    fn defaults_when_section_missing() {
        let s = settings(&cfg("{}"));
        assert!(!s.enabled);
        assert!(s.preview_outgoing);
        assert_eq!(s.unknown, UnknownPolicy::Label);
        assert_eq!(s.max_live_projects, 2);
        assert_eq!(s.idle_secs, 1800);
        assert!(s.disk_search_enabled);
    }

    #[test]
    fn reads_section_values() {
        let s = settings(&cfg(r#"
agent:
  enabled: true
  root: 'D:\hds-anon'
  preview_outgoing: false
  subagents: true
  keep_terms: ['ЗУП', '1С']
  filter:
    unknown: block
  network:
    proxy: 'http://127.0.0.1:8080'
  egress_log: true
  max_live_projects: 3
  idle_unload_min: 5
  request_timeout_sec: 45
  node:
    path: 'C:\node\node.exe'
  mcp:
    disk_search:
      enabled: false
    tavily:
      enabled: false
"#));
        assert!(s.enabled);
        assert_eq!(s.root, Some(PathBuf::from(r"D:\hds-anon")));
        assert!(!s.preview_outgoing);
        assert!(s.subagents);
        assert_eq!(s.keep_terms, vec!["ЗУП".to_string(), "1С".to_string()]);
        assert_eq!(s.unknown, UnknownPolicy::Block);
        assert_eq!(s.proxy.as_deref(), Some("http://127.0.0.1:8080"));
        assert!(s.egress_log);
        assert_eq!(s.max_live_projects, 3);
        assert_eq!(s.idle_secs, 300);
        assert_eq!(s.request_timeout_secs, 45);
        assert_eq!(s.filter_max_stale_secs, 300, "дефолт В-14");
        assert_eq!(s.node_path, Some(PathBuf::from(r"C:\node\node.exe")));
        assert!(!s.disk_search_enabled);
        assert!(!s.tavily_enabled);
        assert!(s.onec_enabled);
    }

    #[test]
    fn filter_stale_is_configurable() {
        let s = settings(&cfg("agent:\n  filter:\n    max_stale_sec: 60\n"));
        assert_eq!(s.filter_max_stale_secs, 60);
    }

    #[test]
    fn project_paths_are_under_root() {
        let c = cfg("agent:\n  root: 'D:\\hds-anon'\n");
        let dir = project_dir(&c, "P0001").unwrap();
        assert!(dir.ends_with("P0001"));
        assert!(workspace_dir(&c, "P0001").unwrap().ends_with("workspace"));
        assert!(config_dir(&c, "P0001").unwrap().ends_with("agent-config"));
        assert!(state_dir_for(&c, "P0001").unwrap().ends_with("agent-state"));
    }

    #[test]
    fn registry_path_is_per_project_db_in_user_zone() {
        let c = cfg("agent:\n  root: 'D:\\hds-anon'\n");
        let db = registry_db(&c, "P0001").expect("P0001");
        assert!(db.ends_with("P0001.db"));
        assert!(db
            .parent()
            .unwrap()
            .ends_with(Path::new("data").join("agent").join("registry")));
        // Не идентификатор проекта — путь не строится (защита от обхода).
        assert!(registry_db(&c, "..\\..\\evil").is_none());
        assert!(registry_db(&c, "P1").is_none());
    }
}
