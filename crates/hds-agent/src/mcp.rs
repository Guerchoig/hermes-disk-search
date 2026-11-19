//! `<P>\agent-config\.cline\data\settings\cline_mcp_settings.json` (TR-3.6).
//!
//! Программной опции «вот серверы» у SDK 0.2.0 нет (A0 §3.7): список MCP
//! читается из файла настроек по пути `<HOME>/.cline/data/settings/…`, а `HOME`
//! процесса агента — это `<P>\agent-config`. Поэтому hds пишет файл сам при
//! каждом старте проекта.
//!
//! Что попадает в файл:
//! * `disk-search` — streamableHttp на маршрут A2 `/mcp/agent/<P>` с токеном
//!   проекта (заголовок `Authorization: Bearer …`);
//! * `tavily` — stdio через `node_modules/tavily-mcp/build/index.js`;
//! * `1c-buddy` — stdio, команда из `agent.mcp.onec_buddy.command` или
//!   `secrets.json` (путь без пробелов либо в кавычках — A0 §3.7).
//!
//! Файл содержит токен, поэтому после записи на него ставится ACL «только
//! владелец» (В-1); неудача ACL — предупреждение, не ошибка (как в A2).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::secrets::Secrets;

/// Параметры записи `cline_mcp_settings.json`.
#[derive(Debug, Clone, Default)]
pub struct McpSpec {
    /// Порт `hds mcp-http` (по умолчанию 8787).
    pub port: u16,
    /// Базовый путь MCP (`mcp_http.path`, по умолчанию `/mcp`).
    pub base_path: String,
    /// Проект (`P0001`).
    pub project: String,
    /// Токен проекта в открытом виде.
    pub token: Option<String>,
    /// Интерпретатор Node (для `tavily`).
    pub node: PathBuf,
    /// Скрипт `tavily-mcp` внутри `node_modules`.
    pub tavily_script: Option<PathBuf>,
    /// Ключ Tavily.
    pub tavily_key: Option<String>,
    /// Команда `1c-buddy` (путь к exe).
    pub onec_command: Option<PathBuf>,
    /// Аргументы `1c-buddy`.
    pub onec_args: Vec<String>,
    /// Переменные окружения `1c-buddy`.
    pub onec_env: Vec<(String, String)>,
}

impl McpSpec {
    /// Собрать спецификацию из секретов и настроек.
    pub fn from_secrets(
        project: &str,
        port: u16,
        base_path: &str,
        node: &Path,
        tavily_script: Option<PathBuf>,
        secrets: &Secrets,
    ) -> McpSpec {
        McpSpec {
            port,
            base_path: base_path.to_string(),
            project: project.to_string(),
            token: secrets.project_token(project),
            node: node.to_path_buf(),
            tavily_script,
            tavily_key: secrets.mcp_key("tavily"),
            onec_command: secrets.mcp_str("onec_buddy", "command").map(PathBuf::from),
            onec_args: secrets.mcp_list("onec_buddy", "args"),
            onec_env: Vec::new(),
        }
    }

    /// URL маршрута агента: `http://127.0.0.1:<port><base>/agent/<P>`.
    pub fn disk_search_url(&self) -> String {
        let base = self.base_path.trim_end_matches('/');
        format!(
            "http://127.0.0.1:{}{}/agent/{}",
            self.port, base, self.project
        )
    }

    /// Собрать JSON настроек. Отсутствующие серверы не попадают в файл.
    pub fn to_json(&self) -> Value {
        let mut servers = serde_json::Map::new();
        if let Some(token) = self.token.as_deref() {
            servers.insert(
                "disk-search".to_string(),
                json!({
                    "transport": {
                        "type": "streamableHttp",
                        "url": self.disk_search_url(),
                        "headers": { "Authorization": format!("Bearer {token}") }
                    },
                    "timeoutSeconds": 60
                }),
            );
        }
        if let (Some(script), Some(key)) = (self.tavily_script.as_ref(), self.tavily_key.as_deref())
        {
            servers.insert(
                "tavily".to_string(),
                json!({
                    "transport": {
                        "type": "stdio",
                        "command": self.node.to_string_lossy(),
                        "args": [script.to_string_lossy()],
                        "env": { "TAVILY_API_KEY": key }
                    },
                    "timeoutSeconds": 60
                }),
            );
        }
        if let Some(command) = self.onec_command.as_ref() {
            let mut env = serde_json::Map::new();
            for (key, value) in &self.onec_env {
                env.insert(key.clone(), json!(value));
            }
            servers.insert(
                "1c-buddy".to_string(),
                json!({
                    "transport": {
                        "type": "stdio",
                        "command": command.to_string_lossy(),
                        "args": self.onec_args,
                        "env": env
                    },
                    "timeoutSeconds": 120
                }),
            );
        }
        json!({ "mcpServers": servers })
    }
}

/// Путь файла настроек MCP внутри `agent-config`.
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir
        .join(".cline")
        .join("data")
        .join("settings")
        .join("cline_mcp_settings.json")
}

/// Записать настройки и выставить ACL «только владелец» (best-effort).
pub fn write_settings(config_dir: &Path, spec: &McpSpec) -> std::io::Result<PathBuf> {
    let path = settings_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&spec.to_json())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(&path, text)?;
    lock_down(&path);
    Ok(path)
}

/// ACL «только владелец» на Windows; вне Windows — права каталога.
#[cfg(windows)]
fn lock_down(path: &Path) {
    let Ok(user) = std::env::var("USERNAME") else {
        return;
    };
    if user.trim().is_empty() {
        return;
    }
    let _ = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(windows))]
fn lock_down(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> McpSpec {
        McpSpec {
            port: 8787,
            base_path: "/mcp".to_string(),
            project: "P0001".to_string(),
            token: Some("tok-123456".to_string()),
            node: PathBuf::from("node"),
            tavily_script: Some(PathBuf::from(
                "C:\\hds\\sidecar\\hds_agent\\node_modules\\tavily-mcp\\build\\index.js",
            )),
            tavily_key: Some("tvly-key".to_string()),
            onec_command: Some(PathBuf::from("C:\\bin\\1c-buddy.exe")),
            onec_args: vec!["--stdio".to_string()],
            onec_env: Vec::new(),
        }
    }

    #[test]
    fn disk_search_route_has_project_and_token() {
        let json = spec().to_json();
        let server = &json["mcpServers"]["disk-search"];
        assert_eq!(
            server["transport"]["url"],
            "http://127.0.0.1:8787/mcp/agent/P0001"
        );
        assert_eq!(
            server["transport"]["headers"]["Authorization"],
            "Bearer tok-123456"
        );
        assert_eq!(server["transport"]["type"], "streamableHttp");
    }

    #[test]
    fn missing_servers_are_omitted() {
        let mut s = spec();
        s.token = None;
        s.tavily_script = None;
        s.onec_command = None;
        let json = s.to_json();
        let servers = json["mcpServers"].as_object().unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn settings_file_is_written_to_cline_path() {
        let dir = std::env::temp_dir().join(format!("hds-agent-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_settings(&dir, &spec()).unwrap();
        assert!(path.ends_with("cline_mcp_settings.json"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("streamableHttp"));
        assert!(text.contains("P0001"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
