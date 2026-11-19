//! `data\agent\secrets.json` — единственное место, где живут ключи провайдеров,
//! MCP и токены проекта (TR-8.3, TR-3.3, TR-3.6).
//!
//! Схема аддитивна к A2 (там был только `projects`), поэтому читаем как
//! `serde_json::Value`, а не типизированной структурой: ключи появляются по
//! мере подключения провайдеров и MCP, и старые файлы должны продолжать
//! работать.
//!
//! ```json
//! {
//!   "projects": {
//!     "P0001": { "token_sha256": "...", "token": "...", "provider": "openrouter",
//!                "model": "..." }
//!   },
//!   "providers": {
//!     "openrouter": { "api_key": "...", "base_url": "https://openrouter.ai/api/v1" }
//!   },
//!   "mcp": {
//!     "tavily": { "api_key": "..." },
//!     "onec_buddy": { "command": "...", "args": ["--env-file", "..."] }
//!   }
//! }
//! ```
//!
//! Значения из этого файла **никогда** не попадают в журналы, события и
//! `cline_mcp_settings.json` (кроме токена проекта — он нужен SDK, но файл
//! настроек получает ACL «только владелец», В-1).

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Реквизиты провайдера из `secrets.json` (без модели — она в `projects.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCreds {
    /// Имя ключа в `providers.<name>` (оно же `kind` для SDK).
    pub name: String,
    /// База API; `None` — взять значение по умолчанию для этого провайдера.
    pub base_url: Option<String>,
    /// Ключ API.
    pub api_key: String,
}

/// Ошибка чтения секретов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsError {
    /// Файл не найден.
    Missing(PathBuf),
    /// Файл не разбирается как JSON.
    Invalid(String),
    /// Нет нужного ключа.
    NoKey(String),
}

impl std::fmt::Display for SecretsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretsError::Missing(p) => write!(f, "нет файла секретов: {}", p.display()),
            SecretsError::Invalid(e) => write!(f, "secrets.json не разбирается: {e}"),
            SecretsError::NoKey(k) => write!(f, "в secrets.json нет ключа {k}"),
        }
    }
}

impl std::error::Error for SecretsError {}

/// Прочитанный `secrets.json`.
#[derive(Debug, Clone)]
pub struct Secrets {
    path: PathBuf,
    raw: Value,
}

impl Secrets {
    /// Прочитать файл; отсутствие — ошибка (без ключей агент не запускается).
    pub fn load(path: &Path) -> Result<Secrets, SecretsError> {
        let text =
            std::fs::read_to_string(path).map_err(|_| SecretsError::Missing(path.to_path_buf()))?;
        let raw: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| SecretsError::Invalid(e.to_string()))?;
        Ok(Secrets {
            path: path.to_path_buf(),
            raw,
        })
    }

    /// Путь, откуда прочитан файл.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Провайдер проекта: `projects.<p>.provider` + `providers.<provider>`.
    pub fn provider_for(&self, project: &str) -> Result<ProviderCreds, SecretsError> {
        let name = self
            .project_str(project, "provider")
            .ok_or_else(|| SecretsError::NoKey(format!("projects.{project}.provider")))?;
        self.provider(&name)
    }

    /// Реквизиты провайдера по имени (`providers.<name>`): ключ и `base_url`.
    ///
    /// Имя берётся из реестра проектов (`projects.json`), а не только из
    /// `secrets.json`: CLI A3.3 читает провайдера/модель из реестра и подставляет
    /// ключ по имени (TR-3.3), поэтому нужен доступ без записи `projects.<p>`.
    pub fn provider(&self, name: &str) -> Result<ProviderCreds, SecretsError> {
        let key = self
            .raw
            .pointer(&format!("/providers/{name}/api_key"))
            .and_then(Value::as_str)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| SecretsError::NoKey(format!("providers.{name}.api_key")))?;
        let base_url = self
            .raw
            .pointer(&format!("/providers/{name}/base_url"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|u| !u.trim().is_empty());
        Ok(ProviderCreds {
            name: name.to_string(),
            base_url,
            api_key: key.to_string(),
        })
    }

    /// Модель проекта (`projects.<p>.model`).
    pub fn project_model(&self, project: &str) -> Option<String> {
        self.project_str(project, "model")
    }

    /// Провайдер проекта из `projects.<p>.provider` (если записан в секретах).
    pub fn project_provider(&self, project: &str) -> Option<String> {
        self.project_str(project, "provider")
            .filter(|name| !name.trim().is_empty())
    }

    /// Токен проекта в открытом виде (для `cline_mcp_settings.json`).
    pub fn project_token(&self, project: &str) -> Option<String> {
        self.project_str(project, "token")
    }

    /// Ключ MCP-сервера: `mcp.<name>.api_key`.
    pub fn mcp_key(&self, name: &str) -> Option<String> {
        self.raw
            .pointer(&format!("/mcp/{name}/api_key"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Строка из `mcp.<name>.<field>`.
    pub fn mcp_str(&self, name: &str, field: &str) -> Option<String> {
        self.raw
            .pointer(&format!("/mcp/{name}/{field}"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Массив строк из `mcp.<name>.<field>`.
    pub fn mcp_list(&self, name: &str, field: &str) -> Vec<String> {
        self.raw
            .pointer(&format!("/mcp/{name}/{field}"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Дополнительные MCP-серверы из общего блока `mcp.servers.<имя>` (A3.7):
    /// готовый элемент `cline_mcp_settings.json` (stdio/http), порядок — по имени.
    /// Не-объекты пропускаются. Встроенные `disk-search`/`tavily`/`1c-buddy`
    /// задаются отдельными ключами `mcp.<имя>` и здесь не участвуют.
    pub fn mcp_servers(&self) -> Vec<(String, Value)> {
        let Some(map) = self.raw.pointer("/mcp/servers").and_then(Value::as_object) else {
            return Vec::new();
        };
        let mut out: Vec<(String, Value)> = map
            .iter()
            .filter(|(_, value)| value.is_object())
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Все секретные строки (ключи провайдеров, MCP, токены проектов) — для
    /// словаря исходящего фильтра (TR-6.2 д).
    ///
    /// Собираются только **значения секретных полей** (`api_key`, `token`,
    /// `password`, `authorization`, все значения `headers`/`env`) и токены
    /// проектов. Пути (`command`), аргументы (`args`), адреса (`url`,
    /// `base_url`) и тип транспорта секретами не считаются: иначе безобидная
    /// строка вроде `--env-file` блокировала бы каждый запрос к модели.
    pub fn secret_strings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for group in ["providers", "mcp"] {
            if let Some(map) = self.raw.get(group).and_then(Value::as_object) {
                for value in map.values() {
                    collect_secret_strings(value, &mut out);
                }
            }
        }
        if let Some(projects) = self.raw.get("projects").and_then(Value::as_object) {
            for value in projects.values() {
                if let Some(token) = value.get("token").and_then(Value::as_str) {
                    if token.len() >= 8 {
                        out.push(token.to_string());
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    fn project_str(&self, project: &str, field: &str) -> Option<String> {
        self.raw
            .pointer(&format!("/projects/{project}/{field}"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// Ключ объекта, значение которого — секрет.
fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key" | "apikey" | "token" | "password" | "authorization"
    )
}

fn push_secret(value: &Value, out: &mut Vec<String>) {
    let Some(text) = value.as_str() else {
        return;
    };
    let text = text.trim();
    if text.len() >= 8 && !out.iter().any(|s| s == text) {
        out.push(text.to_string());
    }
    // `Authorization: Bearer <token>`: сам токен тоже должен попадать в словарь.
    if let Some(rest) = text
        .strip_prefix("Bearer ")
        .or_else(|| text.strip_prefix("bearer "))
    {
        let rest = rest.trim();
        if rest.len() >= 8 && !out.iter().any(|s| s == rest) {
            out.push(rest.to_string());
        }
    }
}

fn collect_secret_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let lower = key.to_ascii_lowercase();
                if is_secret_key(&lower) {
                    push_secret(item, out);
                } else if lower == "headers" || lower == "env" {
                    // Все значения заголовков/переменных окружения MCP —
                    // потенциальные секреты (Authorization, ключи серверов).
                    if let Some(inner) = item.as_object() {
                        for v in inner.values() {
                            push_secret(v, out);
                        }
                    }
                } else if item.is_object() || item.is_array() {
                    collect_secret_strings(item, out);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_secret_strings(item, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_secrets(dir: &Path) -> PathBuf {
        let path = dir.join("secrets.json");
        std::fs::write(
            &path,
            r#"{
              "projects": { "P0001": { "token_sha256": "abc", "token": "tok-P0001-123456", "provider": "openrouter", "model": "openai/gpt-4o-mini" } },
              "providers": { "openrouter": { "api_key": "sk-or-v1-abcdefgh", "base_url": "https://openrouter.ai/api/v1" } },
              "mcp": { "tavily": { "api_key": "tvly-abcdefgh" }, "onec_buddy": { "command": "C:\\bin\\1c-buddy.exe", "args": ["--env-file", "x"] } }
            }"#,
        )
        .unwrap();
        path
    }

    #[test]
    fn provider_is_read_from_project_and_providers() {
        let dir = std::env::temp_dir().join(format!("hds-agent-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_secrets(&dir);
        let secrets = Secrets::load(&path).unwrap();
        let creds = secrets.provider_for("P0001").unwrap();
        assert_eq!(creds.name, "openrouter");
        assert_eq!(creds.api_key, "sk-or-v1-abcdefgh");
        assert_eq!(
            creds.base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            secrets.project_model("P0001").as_deref(),
            Some("openai/gpt-4o-mini")
        );
        assert_eq!(
            secrets.project_token("P0001").as_deref(),
            Some("tok-P0001-123456")
        );
        assert_eq!(secrets.mcp_key("tavily").as_deref(), Some("tvly-abcdefgh"));
        assert_eq!(
            secrets.mcp_list("onec_buddy", "args"),
            vec!["--env-file", "x"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_strings_include_keys_and_tokens() {
        let dir = std::env::temp_dir().join(format!("hds-agent-secrets2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_secrets(&dir);
        let secrets = Secrets::load(&path).unwrap();
        let strings = secrets.secret_strings();
        assert!(strings.iter().any(|s| s == "sk-or-v1-abcdefgh"));
        assert!(strings.iter().any(|s| s == "tvly-abcdefgh"));
        assert!(strings.iter().any(|s| s == "tok-P0001-123456"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_and_key_are_reported() {
        let missing = Path::new("C:\\nope\\secrets.json");
        assert!(matches!(
            Secrets::load(missing),
            Err(SecretsError::Missing(_))
        ));
        let dir = std::env::temp_dir().join(format!("hds-agent-secrets3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = write_secrets(&dir);
        let secrets = Secrets::load(&path).unwrap();
        assert!(matches!(
            secrets.provider_for("P9999"),
            Err(SecretsError::NoKey(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_secret_fields_are_not_in_dictionary() {
        let dir = std::env::temp_dir().join(format!("hds-agent-secrets5-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secrets.json");
        std::fs::write(
            &path,
            r#"{
              "providers": { "openrouter": { "api_key": "sk-or-secret-123456", "base_url": "https://openrouter.ai/api/v1" } },
              "mcp": {
                "servers": {
                  "1c-buddy": { "type": "stdio", "command": "C:\\tools\\1c-buddy-mcp.exe",
                                "args": ["--env-file", "C:\\tools\\.env"], "env": {} },
                  "example-http": { "type": "streamableHttp", "url": "https://example.invalid/mcp",
                                    "headers": { "Authorization": "Bearer mcp-secret-123456" } }
                }
              }
            }"#,
        )
        .unwrap();
        let secrets = Secrets::load(&path).unwrap();
        let strings = secrets.secret_strings();
        assert!(strings.iter().any(|s| s == "sk-or-secret-123456"));
        assert!(strings.iter().any(|s| s == "mcp-secret-123456"));
        assert!(strings.iter().any(|s| s == "Bearer mcp-secret-123456"));
        // Пути, аргументы и адреса — не секреты: иначе ложные блокировки.
        for needle in [
            "--env-file",
            "1c-buddy-mcp.exe",
            "example.invalid",
            "/api/v1",
        ] {
            assert!(
                !strings.iter().any(|s| s.contains(needle)),
                "{needle} не должен попадать в словарь: {strings:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mcp_servers_reads_named_block_sorted() {
        let dir = std::env::temp_dir().join(format!("hds-agent-secrets4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secrets.json");
        std::fs::write(
            &path,
            r#"{
              "mcp": {
                "servers": {
                  "z-http": { "type": "streamableHttp", "url": "https://example.invalid/mcp", "headers": { "Authorization": "Bearer secret-token-123" } },
                  "1c-buddy": { "type": "stdio", "command": "C:\\tools\\1c-buddy.exe", "args": ["--env-file", "C:\\tools\\.env"], "env": { "TOKEN": "secret-token-123" } }
                },
                "tavily": { "api_key": "tvly-secret-123456" }
              }
            }"#,
        )
        .unwrap();
        let secrets = Secrets::load(&path).unwrap();
        let servers = secrets.mcp_servers();
        let names: Vec<&str> = servers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["1c-buddy", "z-http"]);
        assert_eq!(servers[0].1["type"], "stdio");
        assert_eq!(servers[0].1["args"][0], "--env-file");
        // Секреты новых серверов попадают в словарь фильтра.
        let strings = secrets.secret_strings();
        assert!(strings.iter().any(|s| s == "secret-token-123"));
        assert!(strings.iter().any(|s| s == "Bearer secret-token-123"));
        assert!(strings.iter().any(|s| s == "tvly-secret-123456"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
