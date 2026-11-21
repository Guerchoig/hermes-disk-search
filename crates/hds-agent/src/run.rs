//! План хода агента: реестр проектов, секреты, каталоги, окружение процесса,
//! MCP, словарь исходящего и клиент анонимизации (A5.2).
//!
//! Раньше этот код жил в `crates/hds-cli/src/cmd/agent_run.rs`. `hds-cli`
//! зависит от `hds-ui` (CLI поднимает веб-сервер), поэтому вкладка «Агент» не
//! могла им пользоваться. Теперь план хода — библиотека: CLI (`hds agent run`,
//! `hds agent history clear`) и `hds ui` собирают сессию одинаково:
//! [`plan_with`] → [`prepare`] → [`open_client`] → `Gateway::spawn`/`open`/`send`.
//!
//! Ключ провайдера живёт только в памяти: поле `RunPlan::secrets` приватное,
//! [`plan_json`] печатает план без ключей и без пароля MCP.
//!
//! Границы: `run` (один ход с печатью в stdout) остаётся в CLI — здесь только
//! сборка плана, подготовка каталогов, клиент и «Очистить историю».

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use hds_anonymize::protocol::Snapshot;
use hds_anonymize::{Client, ClientConfig, Settings as AnonSettings};
use hds_core::config::{dig, project_root, Config};

use crate::env;
use crate::filter::Filter;
use crate::history::{self, HistoryOutcome};
use crate::mcp::{self, McpSpec};
use crate::projects::{self, Project};
use crate::protocol::Provider;
use crate::rules;
use crate::secrets::Secrets;
use crate::settings::{self, AgentSettings};
use crate::worker::WorkerSpec;

// Провайдер и модель нового проекта по умолчанию (В-12) — одна копия на весь
// workspace: `crate::projects`. Здесь реэкспорт, чтобы вызовы не менялись.
pub use crate::projects::{DEFAULT_MODEL, DEFAULT_PROVIDER};

/// Параметры одного прогона.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Реплика; `None` — читается со stdin (только CLI).
    pub prompt: Option<String>,
    /// Возобновить сессию (TR-3.9): `session_id` из `start` плюс история из
    /// `<P>\agent-state\.cline\data\sessions\<id>\<id>.messages.json`.
    pub session: Option<String>,
    /// Потолок итераций агента (`start.max_iterations`).
    pub max_iterations: u32,
    /// Суб-агенты (`agent.subagents`), по умолчанию выключены (TR-3.7).
    pub subagents: bool,
    /// NDJSON-события в stdout (для тестов и внешних обвязок).
    pub json: bool,
    /// Только ответ: без строк сессии и инструментов.
    pub quiet: bool,
}

impl RunOptions {
    /// Значения по умолчанию: 12 итераций, суб-агенты выключены (TR-3.7;
    /// `agent.subagents` в конфиге включает их и без флага), новая сессия.
    pub fn new(prompt: Option<String>) -> RunOptions {
        RunOptions {
            prompt,
            session: None,
            max_iterations: 12,
            subagents: false,
            json: false,
            quiet: false,
        }
    }
}

/// Всё, что нужно для запуска сессии (собирается без процессов).
///
/// `secrets` — только в памяти: [`plan_json`] их не печатает, доступ к строкам
/// словаря — через [`RunPlan::secret_strings`].
pub struct RunPlan {
    pub project: Project,
    pub provider: Provider,
    pub workspace: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    /// Реестр артефактов `data\agent\registry\<P>.db`, если зона настроена
    /// (AG-26: деанонимайзер берёт из него карту «образ → оригинал»).
    pub registry_db: Option<PathBuf>,
    pub node: PathBuf,
    pub worker: PathBuf,
    pub spec: WorkerSpec,
    /// Системные строки фильтра (пользователь, компьютер, профиль, папка).
    pub system: Vec<String>,
    /// Имена и пути проектов реестра.
    pub project_names: Vec<String>,
    pub settings: AgentSettings,
    pub mcp: McpSpec,
    /// Журнал аудита (`audit.enabled`, TR-6.4); `None` — выключен.
    pub audit: Option<hds_core::audit::Audit>,
    secrets: Secrets,
}

impl RunPlan {
    /// Секреты проекта. Наружу отдавать только строки словаря —
    /// [`RunPlan::secret_strings`], а не сам объект с ключом API.
    pub fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    /// Строки секретов для словаря исходящего (TR-6.2): ключи провайдеров,
    /// токены проектов, пароли внешних MCP.
    pub fn secret_strings(&self) -> Vec<String> {
        self.secrets.secret_strings()
    }
}

/// Итог хода (по `GatewayEvent::TurnEnd`).
#[derive(Debug, Clone, PartialEq)]
pub struct TurnReport {
    pub ok: bool,
    pub reason: String,
    pub text: String,
    pub error: Option<String>,
    pub tools: Vec<String>,
    /// Сессия, в которой прошёл ход: возобновлённая (`--session`) или выданная
    /// воркером (`start.result.session_id`, A3.4). Её принимает
    /// `hds agent history clear --session`.
    pub session_id: Option<String>,
}

/// Вид провайдера для SDK: имя из `secrets.json` → `providerId` (TR-3.3).
pub fn provider_kind(name: &str) -> &'static str {
    match name.trim().to_ascii_lowercase().as_str() {
        "openrouter" => "openrouter",
        "anthropic" => "anthropic",
        _ => "openai-compatible",
    }
}

/// База API по умолчанию. OpenAI-совместимые — **с** `/v1`, Anthropic — без
/// (SDK зовёт `/messages`, П-9).
pub fn default_base_url(kind: &str) -> &'static str {
    match kind {
        "openrouter" => "https://openrouter.ai/api/v1",
        "anthropic" => "https://api.anthropic.com",
        _ => "https://api.openai.com/v1",
    }
}

/// Абсолютный путь без verbatim-префикса `\\?\` (иначе Node не стартует,
/// §4 п.13 карты под-этапов).
fn clean_path(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let text = abs.to_string_lossy().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => abs,
    }
}

/// Интерпретатор Node: `agent.node.path` → `sidecar/node/node(.exe)` → PATH.
pub fn resolve_node(s: &AgentSettings, sidecar_root: &Path) -> Result<PathBuf, String> {
    if let Some(path) = s.node_path.as_ref() {
        let path = clean_path(path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "agent.node.path: файл не найден: {}",
            path.display()
        ));
    }
    let name = if cfg!(windows) { "node.exe" } else { "node" };
    let bundled = sidecar_root.join("sidecar").join("node").join(name);
    if bundled.is_file() {
        return Ok(clean_path(&bundled));
    }
    // Системный node из PATH процесса (env::build переносит PATH).
    Ok(PathBuf::from("node"))
}

/// Собранный воркер агента (`sidecar/hds_agent/dist/worker.mjs`).
pub fn resolve_worker(sidecar_root: &Path) -> Result<PathBuf, String> {
    let path = sidecar_root
        .join("sidecar")
        .join("hds_agent")
        .join("dist")
        .join("worker.mjs");
    if path.is_file() {
        return Ok(clean_path(&path));
    }
    Err(format!(
        "не найден воркер агента: {} (соберите `cd sidecar/hds_agent && npm run build` \
         или запустите installers/build_agent_sidecar.ps1)",
        path.display()
    ))
}

fn dedup_keep(list: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|value| seen.insert(value.to_ascii_lowercase()));
}

/// Системные строки для фильтра (TR-6.2 в): пользователь, компьютер, профиль,
/// каталог проекта; отдельно — имена и пути проектов реестра.
pub fn system_values(cfg: &Config, project: &Project) -> (Vec<String>, Vec<String>) {
    let mut system: Vec<String> = Vec::new();
    for key in [
        "USERNAME",
        "USER",
        "COMPUTERNAME",
        "HOSTNAME",
        "USERPROFILE",
        "HOME",
    ] {
        if let Some(value) = std::env::var_os(key) {
            let value = value.to_string_lossy().trim().to_string();
            if value.len() >= 3 {
                system.push(value);
            }
        }
    }
    if let Some(dir) = settings::project_dir(cfg, &project.id) {
        system.push(dir.to_string_lossy().to_string());
    }
    if !project.path.trim().is_empty() {
        system.push(project.path.clone());
    }
    let mut names: Vec<String> = Vec::new();
    if let Ok(registry) = projects::load(cfg) {
        for entry in registry.projects {
            names.push(entry.name);
            names.push(entry.id);
            if !entry.path.trim().is_empty() {
                names.push(entry.path);
            }
        }
    }
    for list in [&mut system, &mut names] {
        list.retain(|value| value.trim().len() >= 3);
        dedup_keep(list);
    }
    (system, names)
}

/// Порт `mcp_http` для адреса disk-search в `cline_mcp_settings.json`.
pub fn mcp_port(cfg: &Config) -> u16 {
    dig(cfg, "mcp_http.port")
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0 && *v <= u16::MAX as u64)
        .unwrap_or(8787) as u16
}

/// База HTTP MCP (`mcp_http.path`, по умолчанию `/mcp`).
pub fn mcp_base_path(cfg: &Config) -> String {
    dig(cfg, "mcp_http.path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("/mcp")
        .to_string()
}

/// Скрипт `tavily-mcp`: `agent.mcp.tavily.command` или `node_modules` воркера.
pub fn tavily_script(agent: &AgentSettings, sidecar_root: &Path) -> Option<PathBuf> {
    if let Some(command) = agent.tavily_command.as_ref() {
        return Some(clean_path(Path::new(command)));
    }
    let path = sidecar_root
        .join("sidecar")
        .join("hds_agent")
        .join("node_modules")
        .join("tavily-mcp")
        .join("build")
        .join("index.js");
    path.is_file().then(|| clean_path(&path))
}

/// Собрать план сессии: реестр, секреты, каталоги, окружение, MCP, node.
///
/// Ничего не пишет на диск — записи делает [`prepare`]. Ключ провайдера живёт
/// только в памяти и в [`plan_json`] не попадает.
pub fn plan(cfg: &Config, project_id: &str, opts: &RunOptions) -> Result<RunPlan, String> {
    plan_with(cfg, project_id, opts, &project_root())
}

/// То же с явным корнем поставки (`sidecar/`, портативный Node): тесты держат
/// данные в `HDS_ROOT`, а воркер берут из репозитория.
pub fn plan_with(
    cfg: &Config,
    project_id: &str,
    opts: &RunOptions,
    sidecar_root: &Path,
) -> Result<RunPlan, String> {
    let mut agent = settings::settings(cfg);
    if agent.root.is_none() {
        return Err(
            "agent.root не задан: анонимная зона не настроена (hds config init / config.yaml)"
                .to_string(),
        );
    }
    let project = projects::get(cfg, project_id).ok_or_else(|| {
        format!(
            "проект {project_id} не зарегистрирован \
             (hds agent project add {project_id} --path DIR)"
        )
    })?;
    let project_path = PathBuf::from(project.path.clone());
    if !project_path.is_dir() {
        return Err(format!(
            "папка проекта не найдена: {}",
            project_path.display()
        ));
    }
    let workspace = settings::workspace_dir(cfg, &project.id)
        .ok_or_else(|| "agent.root не задан".to_string())?;
    let config_dir =
        settings::config_dir(cfg, &project.id).ok_or_else(|| "agent.root не задан".to_string())?;
    let state_dir = settings::state_dir_for(cfg, &project.id)
        .ok_or_else(|| "agent.root не задан".to_string())?;

    let secrets = Secrets::load(&settings::secrets_file(cfg)).map_err(|e| e.to_string())?;
    let provider_name = if !project.provider.trim().is_empty() {
        project.provider.trim().to_string()
    } else {
        secrets
            .project_provider(&project.id)
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string())
    };
    let creds = secrets
        .provider(&provider_name)
        .map_err(|e| e.to_string())?;
    let kind = provider_kind(&provider_name);
    let model = if !project.model.trim().is_empty() {
        project.model.trim().to_string()
    } else {
        secrets.project_model(&project.id).ok_or_else(|| {
            format!(
                "у проекта {} не задана модель (hds agent project add ... --model M)",
                project.id
            )
        })?
    };
    let provider = Provider {
        kind: kind.to_string(),
        base_url: creds
            .base_url
            .clone()
            .unwrap_or_else(|| default_base_url(kind).to_string()),
        model,
        api_key: creds.api_key.clone(),
    };

    let node = resolve_node(&agent, sidecar_root)?;
    let worker = resolve_worker(sidecar_root)?;
    let child = env::build(&config_dir, &state_dir, &workspace, agent.proxy.as_deref());
    let spec = WorkerSpec {
        program: node.clone(),
        script: worker.clone(),
        cwd: child.cwd.clone(),
        env: child.vars.clone(),
        stderr_log: Some(state_dir.join("worker-stderr.log")),
        timeout: Duration::from_secs(agent.request_timeout_secs.max(30)),
    };

    // Внешние серверы из `secrets.json` (`mcp.servers.<имя>`) попадают в
    // `extra_servers` внутри `from_secrets`; встроенные имена сильнее (A3.7).
    let mut mcp = McpSpec::from_secrets(
        &project.id,
        mcp_port(cfg),
        &mcp_base_path(cfg),
        &node,
        tavily_script(&agent, sidecar_root),
        &secrets,
    );
    if !agent.disk_search_enabled {
        mcp.token = None;
    }
    if !agent.tavily_enabled {
        mcp.tavily_script = None;
        mcp.tavily_key = None;
    }
    if !agent.onec_enabled {
        mcp.onec_command = None;
        mcp.onec_args.clear();
    }
    if let Some(command) = agent.onec_command.as_ref() {
        mcp.onec_command = Some(PathBuf::from(command));
    }

    let (system, project_names) = system_values(cfg, &project);
    agent.subagents = agent.subagents || opts.subagents;

    Ok(RunPlan {
        registry_db: settings::registry_db(cfg, &project.id),
        project,
        provider,
        workspace,
        config_dir,
        state_dir,
        node,
        worker,
        spec,
        system,
        project_names,
        settings: agent,
        mcp,
        audit: hds_core::audit::Audit::from_config(cfg),
        secrets,
    })
}

/// План сессии без секретов (`--dry-run`, тесты, диагностика).
pub fn plan_json(plan: &RunPlan) -> Value {
    let servers: Vec<String> = plan
        .mcp
        .to_json()
        .get("mcpServers")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    json!({
        "project": plan.project.id,
        "name": plan.project.name,
        "path": plan.project.path,
        "workspace": plan.workspace.to_string_lossy(),
        "agent_config": plan.config_dir.to_string_lossy(),
        "agent_state": plan.state_dir.to_string_lossy(),
        "node": plan.node.to_string_lossy(),
        "worker": plan.worker.to_string_lossy(),
        "provider": {
            "name": plan.provider.kind,
            "model": plan.provider.model,
            "base_url": plan.provider.base_url,
        },
        "mcp": {
            "settings": mcp::settings_path(&plan.config_dir).to_string_lossy(),
            "servers": servers,
        },
        "subagents": plan.settings.subagents,
        "filter": {
            "unknown": plan.settings.unknown.as_str(),
            "max_stale_sec": plan.settings.filter_max_stale_secs,
            "system_values": plan.system.len(),
            "project_names": plan.project_names.len(),
        },
    })
}

/// Создать каталоги проекта, записать правила и `cline_mcp_settings.json`.
/// Возвращает путь файла настроек MCP.
pub fn prepare(plan: &RunPlan) -> Result<PathBuf, String> {
    env::prepare_dirs(&plan.config_dir, &plan.state_dir, &plan.workspace)
        .map_err(|e| format!("каталоги проекта: {e}"))?;
    rules::write_rules(&plan.config_dir.join("rules"))
        .map_err(|e| format!("правила агента: {e}"))?;
    mcp::write_settings(&plan.config_dir, &plan.mcp).map_err(|e| format!("MCP-настройки: {e}"))
}

/// Словарь исходящего проекта по снимку vault: одна сборка на все точки —
/// `run` (сессия), `history clear` (TR-6.6) и вкладка «Агент» (A5.2).
pub fn build_filter(
    project: &str,
    settings: &AgentSettings,
    audit: Option<hds_core::audit::Audit>,
    system: &[String],
    project_names: &[String],
    secrets: &[String],
    snapshot: &Snapshot,
) -> Filter {
    let filter = Filter::new(
        project,
        settings.keep_terms.clone(),
        settings.unknown,
        Duration::from_secs(settings.filter_max_stale_secs.max(1)),
        audit,
    );
    filter.set_snapshot(snapshot, system, project_names, secrets);
    filter
}

/// Поднять воркер анонимизации для сессии: Python ищется в `root`
/// (`HDS_ANONYMIZE_PYTHON` → `sidecar/venv-anonymize` → `sidecar/python`),
/// параметры — из секции `anonymize:` конфига.
///
/// `HDS_ANONYMIZE_WORKER` подменяет скрипт воркера: диагностика и тесты
/// (стенд проверяет проводку на mock-воркере, не поднимая NER-модели).
pub fn open_client(
    cfg: &Config,
    plan: &RunPlan,
    root: &Path,
) -> Result<Arc<Mutex<Client>>, String> {
    open_client_at(cfg, &plan.state_dir, root)
}

/// То же по каталогу состояния: `history clear` и вкладка не собирают сессию
/// целиком (провайдер и модель для очистки истории не нужны).
pub fn open_client_at(
    cfg: &Config,
    state_dir: &Path,
    root: &Path,
) -> Result<Arc<Mutex<Client>>, String> {
    let anonymize = AnonSettings::from_config(cfg);
    let Some(python) = hds_anonymize::discover_python(root) else {
        return Err("не найден интерпретатор воркера анонимизации \
             (HDS_ANONYMIZE_PYTHON / sidecar\\venv-anonymize / sidecar\\python)"
            .to_string());
    };
    let mut client_cfg = ClientConfig::from_settings(&python, root, &anonymize);
    // Каталог состояния может быть ещё не создан (open_client зовут до prepare).
    let _ = std::fs::create_dir_all(state_dir);
    client_cfg.stderr_log = Some(state_dir.join("anonymize-stderr.log"));
    if let Some(script) = std::env::var_os("HDS_ANONYMIZE_WORKER") {
        if !script.is_empty() {
            client_cfg.script = Some(PathBuf::from(script));
        }
    }
    Client::spawn(client_cfg)
        .map(|client| Arc::new(Mutex::new(client)))
        .map_err(|e| format!("воркер анонимизации: {e}"))
}

/// Почистить историю сессии проекта (TR-6.6). `session` не задан — берётся
/// последняя сессия из `<P>\agent-state\.cline\data\sessions`.
///
/// Провайдер и модель здесь не нужны: очистка — это словарь (снимок vault) и
/// перезапись файлов, поэтому [`plan_with`] не собирается и Node не запускается.
pub fn clear_history(
    cfg: &Config,
    project_id: &str,
    session: Option<&str>,
    root: &Path,
) -> Result<(String, HistoryOutcome), String> {
    let project = projects::get(cfg, project_id).ok_or_else(|| {
        format!(
            "проект {project_id} не зарегистрирован \
             (hds agent project add {project_id} --path DIR)"
        )
    })?;
    let state_dir = settings::state_dir_for(cfg, &project.id)
        .ok_or_else(|| "agent.root не задан: анонимная зона не настроена".to_string())?;
    if !state_dir.is_dir() {
        return Err(format!(
            "нет состояния проекта: {} (сессий ещё не было)",
            state_dir.display()
        ));
    }
    let session = match session.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => id.to_string(),
        None => history::latest_session(&state_dir)
            .map_err(|e| format!("{e} — укажите --session <ID>"))?,
    };
    // Секреты только расширяют словарь: без secrets.json очистка работает.
    let secrets = Secrets::load(&settings::secrets_file(cfg))
        .map(|s| s.secret_strings())
        .unwrap_or_default();
    let (system, project_names) = system_values(cfg, &project);
    let client = open_client_at(cfg, &state_dir, root)?;
    let snapshot = {
        let mut client = client
            .lock()
            .map_err(|_| "воркер анонимизации занят".to_string())?;
        client
            .snapshot(&project.id)
            .map_err(|e| format!("снимок vault: {e}"))?
    };
    let filter = build_filter(
        &project.id,
        &settings::settings(cfg),
        hds_core::audit::Audit::from_config(cfg),
        &system,
        &project_names,
        &secrets,
        &snapshot,
    );
    let outcome = history::clear(&state_dir, &session, &filter).map_err(|e| e.to_string())?;
    if let Ok(client) = Arc::try_unwrap(client) {
        if let Ok(client) = client.into_inner() {
            client.shutdown();
        }
    }
    Ok((session, outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_follows_secrets_name() {
        assert_eq!(provider_kind("openrouter"), "openrouter");
        assert_eq!(provider_kind(" Anthropic "), "anthropic");
        assert_eq!(provider_kind("openai-compatible"), "openai-compatible");
        assert_eq!(provider_kind("что-то ещё"), "openai-compatible");
    }

    #[test]
    fn default_base_url_keeps_v1_only_for_openai() {
        assert_eq!(
            default_base_url("openrouter"),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(default_base_url("anthropic"), "https://api.anthropic.com");
        assert_eq!(
            default_base_url("openai-compatible"),
            "https://api.openai.com/v1"
        );
        assert_eq!(default_base_url(""), "https://api.openai.com/v1");
    }

    #[test]
    fn clean_path_drops_verbatim_prefix() {
        let dir = std::env::temp_dir();
        let cleaned = clean_path(&dir);
        assert!(
            !cleaned.to_string_lossy().starts_with(r"\\?\"),
            "{}",
            cleaned.display()
        );
        assert_eq!(cleaned, std::path::absolute(&dir).unwrap());
    }

    #[test]
    fn dedup_keep_is_ascii_case_insensitive() {
        // Так же, как в фильтре: сравнение по `to_ascii_lowercase` (кириллица
        // регистр не складывает — формы значений приходят из vault в одном
        // регистре, дубли по регистру не критичны).
        let mut list = vec![
            "Ivanov Ivan".to_string(),
            "ivanov ivan".to_string(),
            "Petrov".to_string(),
        ];
        dedup_keep(&mut list);
        assert_eq!(list, vec!["Ivanov Ivan".to_string(), "Petrov".to_string()]);
    }
}
