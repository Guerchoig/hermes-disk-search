//! Окружение дочернего процесса Node (TR-3.2, D15).
//!
//! Требования:
//! * `HOME`, `USERPROFILE` → `<P>\agent-config`; `APPDATA`, `LOCALAPPDATA`,
//!   `TEMP` → его подкаталоги (создаются заранее); данные SDK —
//!   `CLINE_DATA_DIR = <P>\agent-state\.cline\data`;
//! * ключи провайдеров и MCP в env **не** передаются; остаются только
//!   разрешённые системные переменные (`PATH`, `SystemRoot`, `ComSpec`,
//!   `NUMBER_OF_PROCESSORS`, `PATHEXT`, `WINDIR`);
//! * `HTTP_PROXY`/`HTTPS_PROXY`/`NO_PROXY` — только из `agent.network.proxy`;
//! * рабочий каталог процесса — `<P>\workspace` (П-3: встроенные инструменты
//!   SDK 0.2.0 резолвят относительные пути от `cwd` процесса).
//!
//! Ничего не наследуется: `Command::env_clear()` + этот список.

use std::path::{Path, PathBuf};

/// Готовое окружение и рабочий каталог процесса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildEnv {
    /// Рабочий каталог процесса (`<P>\workspace`).
    pub cwd: PathBuf,
    /// Переменные окружения (уже без родительских).
    pub vars: Vec<(String, String)>,
}

/// Разрешённые системные переменные, которые переносятся как есть.
const PASSTHROUGH: [&str; 7] = [
    "PATH",
    "SystemRoot",
    "ComSpec",
    "NUMBER_OF_PROCESSORS",
    "PATHEXT",
    "WINDIR",
    "PROCESSOR_ARCHITECTURE",
];

/// Каталог данных SDK внутри `agent-state`: сессии, `sessions.db`
/// (П-8: `<P>\agent-state\.cline\data\sessions\<id>\…`).
pub fn sdk_data_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(".cline").join("data")
}

/// Собрать окружение процесса агента.
///
/// * `config_dir` — `<P>\agent-config`: `HOME`/`USERPROFILE` процесса, там же
///   `.cline\data\settings\cline_mcp_settings.json` и `rules\`;
/// * `state_dir` — `<P>\agent-state`: каталог данных SDK (`CLINE_DATA_DIR`);
/// * `workspace` — `<P>\workspace`;
/// * `proxy` — `agent.network.proxy` (пусто/`None` — переменные не ставятся).
///
/// SDK 0.2.0 выводит все каталоги из `HOME`, поэтому сессии без явного
/// `CLINE_DATA_DIR` легли бы в `agent-config`. А при `CLINE_DATA_DIR` настройки
/// MCP переехали бы в `agent-state`, поэтому путь к ним тоже задаётся явно
/// (`CLINE_MCP_SETTINGS_PATH`): его пишет [`crate::mcp::write_settings`].
pub fn build(
    config_dir: &Path,
    state_dir: &Path,
    workspace: &Path,
    proxy: Option<&str>,
) -> ChildEnv {
    let mut vars: Vec<(String, String)> = Vec::new();
    for name in PASSTHROUGH {
        if let Some(value) = std::env::var_os(name) {
            vars.push((name.to_string(), value.to_string_lossy().into_owned()));
        }
    }
    let home = config_dir.to_path_buf();
    let app_data = config_dir.join("AppData").join("Roaming");
    let local_app_data = config_dir.join("AppData").join("Local");
    let temp = config_dir.join("Temp");
    for (name, value) in [
        ("HOME", &home),
        ("USERPROFILE", &home),
        ("APPDATA", &app_data),
        ("LOCALAPPDATA", &local_app_data),
        ("TEMP", &temp),
        ("TMP", &temp),
    ] {
        vars.push((name.to_string(), value.to_string_lossy().into_owned()));
    }
    // HOMEDRIVE/HOMEPATH выводятся из подменённого профиля: без них часть
    // библиотек берёт настоящий профиль из этих переменных.
    let drive = home
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    if !drive.is_empty() {
        let full = home.to_string_lossy().into_owned();
        let path = full
            .strip_prefix(drive.as_str())
            .unwrap_or(&full)
            .to_string();
        vars.push(("HOMEDRIVE".to_string(), drive));
        vars.push(("HOMEPATH".to_string(), path));
    }
    vars.push((
        "CLINE_DATA_DIR".to_string(),
        sdk_data_dir(state_dir).to_string_lossy().into_owned(),
    ));
    vars.push((
        "CLINE_MCP_SETTINGS_PATH".to_string(),
        crate::mcp::settings_path(config_dir)
            .to_string_lossy()
            .into_owned(),
    ));
    if let Some(proxy) = proxy.map(str::trim).filter(|p| !p.is_empty()) {
        vars.push(("HTTP_PROXY".to_string(), proxy.to_string()));
        vars.push(("HTTPS_PROXY".to_string(), proxy.to_string()));
        vars.push(("NO_PROXY".to_string(), "127.0.0.1,localhost".to_string()));
    }
    ChildEnv {
        cwd: workspace.to_path_buf(),
        vars,
    }
}

/// Создать каталоги профиля агента и вернуть путь к `agent-config`.
pub fn prepare_dirs(
    config_dir: &Path,
    state_dir: &Path,
    workspace: &Path,
) -> std::io::Result<PathBuf> {
    for dir in [
        config_dir.to_path_buf(),
        config_dir.join("AppData").join("Roaming"),
        config_dir.join("AppData").join("Local"),
        config_dir.join("Temp"),
        config_dir.join(".cline").join("data").join("settings"),
        sdk_data_dir(state_dir),
        workspace.to_path_buf(),
    ] {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(config_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_redirected_and_secrets_are_not_inherited() {
        let dir = std::env::temp_dir().join(format!("hds-agent-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("agent-config");
        let state = dir.join("agent-state");
        let ws = dir.join("workspace");
        prepare_dirs(&config, &state, &ws).unwrap();
        let env = build(&config, &state, &ws, None);
        assert_eq!(env.cwd, ws);
        let get = |name: &str| {
            env.vars
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        let home = config.to_string_lossy().into_owned();
        assert_eq!(get("USERPROFILE"), Some(home.clone()));
        assert_eq!(get("HOME"), Some(home.clone()));
        let drive = get("HOMEDRIVE").unwrap();
        assert_eq!(format!("{drive}{}", get("HOMEPATH").unwrap()), home);
        // Данные SDK — в agent-state, настройки MCP — в agent-config.
        assert_eq!(
            get("CLINE_DATA_DIR"),
            Some(sdk_data_dir(&state).to_string_lossy().into_owned())
        );
        assert_eq!(
            get("CLINE_MCP_SETTINGS_PATH"),
            Some(
                crate::mcp::settings_path(&config)
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert!(sdk_data_dir(&state).is_dir());
        assert!(get("APPDATA").unwrap().contains("AppData"));
        assert!(get("TEMP").unwrap().contains("Temp"));
        // Ключи провайдеров не попадают в окружение ни при каких условиях.
        for name in [
            "OPENROUTER_API_KEY",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "TAVILY_API_KEY",
        ] {
            assert_eq!(get(name), None, "{name} не должен наследоваться");
        }
        assert_eq!(get("HTTP_PROXY"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn proxy_is_taken_only_from_config() {
        let dir = std::env::temp_dir().join(format!("hds-agent-env-proxy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = dir.join("agent-config");
        let state = dir.join("agent-state");
        let ws = dir.join("workspace");
        prepare_dirs(&config, &state, &ws).unwrap();
        let env = build(&config, &state, &ws, Some("http://127.0.0.1:3128"));
        let get = |name: &str| {
            env.vars
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("HTTP_PROXY").as_deref(), Some("http://127.0.0.1:3128"));
        assert_eq!(get("HTTPS_PROXY").as_deref(), Some("http://127.0.0.1:3128"));
        assert_eq!(get("NO_PROXY").as_deref(), Some("127.0.0.1,localhost"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
