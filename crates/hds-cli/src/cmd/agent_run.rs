//! `hds agent project add|list` и `hds agent run` — точка входа анонимной зоны
//! до появления вкладки (A3.3, В-16).
//!
//! `run` собирает сессию из конфига, как это будет делать UI (A5):
//! реестр проектов (`projects.json`), секреты (`secrets.json`), каталоги
//! `<P>\workspace|agent-config|agent-state`, окружение процесса (TR-3.2),
//! правила (`rules::write_rules`), `cline_mcp_settings.json`, снимок vault
//! (Python-воркер), словарный фильтр и [`Gateway`] поверх Node-воркера.
//!
//! Успех хода определяется событием `GatewayEvent::TurnEnd` (A3.2), а не
//! ответом `send`; срабатывания фильтра печатаются предупреждениями.
//!
//! `--dry-run` печатает план (без ключей) и не запускает ни Python, ни Node —
//! этим пользуются тесты и диагностика.
//!
//! Границы A3.3: один проект на процесс, один ход; реестр живых процессов,
//! простои и возобновление — A3.4; вопросы пользователю (`ask_user`) — A5
//! (сейчас отказ, модель получает понятную ошибку).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use hds_agent::deanon::DeAnonymizer;
use hds_agent::env;
use hds_agent::filter::{sys_values, Filter};
use hds_agent::gateway::{Gateway, GatewayEvent, GatewayHost, NoAskUser};
use hds_agent::history::{self, HistoryOutcome};
use hds_agent::mcp::{self, McpSpec};
use hds_agent::projects::{self, Project};
use hds_agent::protocol::Provider;
use hds_agent::rules;
use hds_agent::secrets::Secrets;
use hds_agent::settings::{self, AgentSettings};
use hds_agent::worker::WorkerSpec;
use hds_anonymize::protocol::Snapshot;
use hds_anonymize::{Client, ClientConfig, Settings as AnonSettings};
use hds_core::config::{dig, project_root, Config};

/// Провайдер нового проекта по умолчанию (В-12: предложение исполнителя).
pub const DEFAULT_PROVIDER: &str = "openrouter";
/// Модель нового проекта по умолчанию (В-12: как в дымовом прогоне A0).
pub const DEFAULT_MODEL: &str = "~deepseek/deepseek-flash-latest";

/// Параметры одного прогона.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Реплика; `None` — читается со stdin.
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
/// `secrets` и `api_key` — только в памяти: [`plan_json`] их не печатает.
pub struct RunPlan {
    pub project: Project,
    pub provider: Provider,
    pub workspace: PathBuf,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub node: PathBuf,
    pub worker: PathBuf,
    pub spec: WorkerSpec,
    pub system: Vec<String>,
    pub project_names: Vec<String>,
    pub settings: AgentSettings,
    pub mcp: McpSpec,
    /// Журнал аудита (`audit.enabled`, TR-6.4); `None` — выключен.
    pub audit: Option<hds_core::audit::Audit>,
    secrets: Secrets,
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
fn provider_kind(name: &str) -> &'static str {
    match name.trim().to_ascii_lowercase().as_str() {
        "openrouter" => "openrouter",
        "anthropic" => "anthropic",
        _ => "openai-compatible",
    }
}

/// База API по умолчанию. OpenAI-совместимые — **с** `/v1`, Anthropic — без
/// (SDK зовёт `/messages`, П-9).
fn default_base_url(kind: &str) -> &'static str {
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
fn resolve_node(s: &AgentSettings, sidecar_root: &Path) -> Result<PathBuf, String> {
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

/// Собранный воркер агента.
fn resolve_worker(sidecar_root: &Path) -> Result<PathBuf, String> {
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
fn system_values(cfg: &Config, project: &Project) -> (Vec<String>, Vec<String>) {
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

fn mcp_port(cfg: &Config) -> u16 {
    dig(cfg, "mcp_http.port")
        .and_then(|v| v.as_u64())
        .filter(|v| *v > 0 && *v <= u16::MAX as u64)
        .unwrap_or(8787) as u16
}

fn mcp_base_path(cfg: &Config) -> String {
    dig(cfg, "mcp_http.path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .unwrap_or("/mcp")
        .to_string()
}

/// Скрипт `tavily-mcp`: `agent.mcp.tavily.command` или `node_modules` воркера.
fn tavily_script(agent: &AgentSettings, sidecar_root: &Path) -> Option<PathBuf> {
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
    let child = env::build(
        cfg,
        &config_dir,
        &state_dir,
        &workspace,
        agent.proxy.as_deref(),
    );
    let spec = WorkerSpec {
        program: node.clone(),
        script: worker.clone(),
        cwd: child.cwd.clone(),
        env: child.vars.clone(),
        stderr_log: Some(state_dir.join("worker-stderr.log")),
        timeout: Duration::from_secs(agent.request_timeout_secs.max(30)),
    };

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

fn read_prompt(opts: &RunOptions) -> Result<String, String> {
    if let Some(prompt) = opts.prompt.as_ref() {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err("пустая реплика (--prompt)".to_string());
        }
        return Ok(prompt.to_string());
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| format!("stdin: {e}"))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("пустая реплика: передайте --prompt TEXT или текст в stdin".to_string());
    }
    Ok(text)
}

fn clip(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        format!("{}…", value.chars().take(max).collect::<String>())
    }
}

/// Словарь исходящего проекта по снимку vault: одна сборка на все точки —
/// `run` (сессия) и `history clear` (очистка сохранённой истории, TR-6.6).
fn build_filter(
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

/// Провести один ход на готовом плане. `client` — поднятый воркер анонимизации
/// (снимок vault и `anonymize_text`, TR-5.1); события печатаются в `out`,
/// предупреждения фильтра — в `err`.
pub fn run(
    plan: &RunPlan,
    client: Arc<Mutex<Client>>,
    opts: &RunOptions,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<TurnReport, String> {
    let prompt = read_prompt(opts)?;
    prepare(plan)?;
    let snapshot = {
        let mut client = client
            .lock()
            .map_err(|_| "воркер анонимизации занят".to_string())?;
        client
            .snapshot(&plan.project.id)
            .map_err(|e| format!("снимок vault: {e}"))?
    };
    let filter = Arc::new(build_filter(
        &plan.project.id,
        &plan.settings,
        plan.audit.clone(),
        &plan.system,
        &plan.project_names,
        &plan.secrets.secret_strings(),
        &snapshot,
    ));
    let deanon = Arc::new(Mutex::new(DeAnonymizer::new(
        &snapshot,
        &sys_values(&plan.system, &plan.project_names),
        &plan.workspace,
        Path::new(&plan.project.path),
    )));
    let egress_log = plan
        .settings
        .egress_log
        .then(|| plan.state_dir.join("egress"));
    let host = Arc::new(GatewayHost::new(
        plan.project.id.clone(),
        &plan.workspace,
        Arc::clone(&filter),
        Arc::clone(&client),
        Arc::new(NoAskUser),
        egress_log,
    ));
    let mut gateway = Gateway::spawn(
        &plan.spec,
        host,
        plan.project.id.clone(),
        &plan.workspace,
        &plan.state_dir,
        filter,
        deanon,
        Arc::clone(&client),
    )
    .map_err(|e| format!("воркер агента: {e}"))?;

    let result = (|| -> Result<TurnReport, String> {
        gateway.hello().map_err(|e| format!("hello: {e}"))?;
        // Возобновление (TR-3.9): один `session_id` историю не грузит (A0 §3.6) —
        // сообщения читаются из файлов сессии и уходят в `initialMessages`.
        let (history, session) = match opts.session.as_deref() {
            Some(id) => {
                let messages = history::read_messages(&plan.state_dir, id)
                    .map_err(|e| format!("история сессии {id}: {e}"))?;
                (messages, Some(id.to_string()))
            }
            None => (Vec::new(), None),
        };
        let resumed = session.is_some();
        let declared = gateway
            .open(
                plan.provider.clone(),
                history,
                session,
                opts.max_iterations.max(1),
                plan.settings.subagents,
            )
            .map_err(|e| format!("start: {e}"))?;
        let session_id = gateway.session_id().map(str::to_string);
        if !opts.quiet && !opts.json {
            writeln!(
                out,
                "[сессия] {} ({}) → {} · {}; инструменты: {}; сессия {}{}",
                plan.project.id,
                plan.project.name,
                plan.provider.kind,
                plan.provider.model,
                declared.join(", "),
                session_id.as_deref().unwrap_or("-"),
                if resumed {
                    " (возобновлена)"
                } else {
                    ""
                }
            )
            .ok();
        } else if opts.json {
            let line = json!({
                "kind": "session",
                "session_id": session_id,
                "resumed": resumed,
                "tools": declared,
            });
            writeln!(out, "{line}").ok();
        }
        let anonymized = gateway.send(&prompt).map_err(|e| e.to_string())?;
        if opts.json {
            let labels: Vec<Value> = anonymized
                .labels
                .iter()
                .map(|l| json!({ "label": l.token, "type": l.entity_type, "count": l.count }))
                .collect();
            let line = json!({
                "kind": "prompt",
                "text": anonymized.text,
                "labels": labels,
            });
            writeln!(out, "{line}").ok();
        } else if plan.settings.preview_outgoing && !opts.quiet {
            writeln!(out, "[фильтр] к модели: {}", anonymized.text).ok();
        }
        let timeout = Duration::from_secs(plan.settings.request_timeout_secs.max(30));
        loop {
            let event = gateway.next_event(timeout).map_err(|e| e.to_string())?;
            if let Some(mut report) = print_event(event, opts, out, err) {
                report.session_id = session_id.clone();
                return Ok(report);
            }
        }
    })();
    gateway.shutdown();
    result
}

fn json_str(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Напечатать событие. `Some` — это `turn_end` (ход завершён).
fn print_event(
    event: GatewayEvent,
    opts: &RunOptions,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Option<TurnReport> {
    match event {
        GatewayEvent::Redacted { hits } => {
            if opts.json {
                let line = json!({
                    "kind": "redacted",
                    "hits": hits.iter().map(|h| json!({
                        "label": h.label, "source": h.source.as_str()
                    })).collect::<Vec<_>>(),
                });
                writeln!(out, "{line}").ok();
            } else if !opts.quiet {
                let text = hits
                    .iter()
                    .map(|h| format!("{} ({})", h.label, h.source.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ");
                writeln!(err, "[фильтр] заменено при отправке: {text}").ok();
            }
            None
        }
        GatewayEvent::Agent { text, .. } => {
            let kind = json_str(&text, "kind");
            if opts.json {
                let line = json!({ "kind": "agent", "event": text });
                writeln!(out, "{line}").ok();
            } else if opts.quiet {
                // `--quiet`: только ответ модели, без шагов и инструментов.
                if kind == "text" {
                    writeln!(out, "{}", json_str(&text, "text")).ok();
                }
            } else {
                match kind.as_str() {
                    "text" => {
                        writeln!(out, "{}", json_str(&text, "text")).ok();
                    }
                    "tool_use" => {
                        let name = json_str(&text, "tool");
                        let input = text.get("input").map(|v| v.to_string()).unwrap_or_default();
                        writeln!(out, "[tool] {name} {}", clip(&input, 300)).ok();
                    }
                    "tool_result" => {
                        let name = json_str(&text, "tool");
                        let failed = text.get("error").is_some();
                        writeln!(
                            out,
                            "[tool] {name} {}",
                            if failed {
                                "ошибка"
                            } else {
                                "готово"
                            }
                        )
                        .ok();
                    }
                    "error" => {
                        writeln!(err, "[ошибка] {}", json_str(&text, "text")).ok();
                    }
                    "notice" => {
                        writeln!(err, "[i] {}", json_str(&text, "text")).ok();
                    }
                    _ => {}
                }
            }
            None
        }
        GatewayEvent::TurnEnd {
            ok,
            reason,
            text,
            error,
            tools,
            ..
        } => {
            if opts.json {
                let line = json!({
                    "kind": "turn_end",
                    "ok": ok,
                    "reason": reason,
                    "text": text,
                    "error": error,
                    "tools": tools,
                });
                writeln!(out, "{line}").ok();
            } else if !opts.quiet {
                match (&error, ok) {
                    (Some(message), _) => writeln!(err, "[сбой] {reason}: {message}").ok(),
                    (None, true) => writeln!(err, "[готово]").ok(),
                    (None, false) => writeln!(err, "[сбой] {reason}").ok(),
                };
            }
            Some(TurnReport {
                ok,
                reason,
                text,
                error,
                tools,
                // Идентификатор сессии знает `run` (после `start`).
                session_id: None,
            })
        }
        GatewayEvent::Ready { .. } | GatewayEvent::Done { .. } => None,
    }
}

fn flag_u32(args: &[String], name: &str) -> Option<u32> {
    crate::cmd::agent::flag_val(args, name).and_then(|v| v.trim().parse().ok())
}

/// `hds agent project add|list` — минимальный реестр для стенда (TR-8.1, A5 —
/// полный CRUD и UI).
pub fn cmd_project(cfg: &Config, args: &[String]) -> i32 {
    match args.split_first() {
        Some((sub, rest)) if sub == "add" => project_add(cfg, rest),
        Some((sub, rest)) if sub == "list" => project_list(cfg, rest),
        Some((sub, _)) => {
            eprintln!("agent project: неизвестная подкоманда '{sub}' (add|list)");
            2
        }
        None => {
            eprintln!("agent project: нужна подкоманда (add|list)");
            2
        }
    }
}

fn project_add(cfg: &Config, args: &[String]) -> i32 {
    let Some(id) = args.iter().find(|a| !a.starts_with("--")).cloned() else {
        eprintln!("agent project add: нужен идентификатор проекта (P0001)");
        return 2;
    };
    let Some(path) = crate::cmd::agent::flag_val(args, "--path") else {
        eprintln!("agent project add: нужен --path DIR (папка проекта)");
        return 2;
    };
    let dir = PathBuf::from(&path);
    if !dir.is_dir() {
        eprintln!("agent project add: папка проекта не найдена: {path}");
        return 1;
    }
    let name = crate::cmd::agent::flag_val(args, "--name")
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            dir.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.trim().is_empty())
        })
        .unwrap_or_else(|| id.clone());
    let project = Project {
        id: id.clone(),
        name: name.clone(),
        path,
        provider: crate::cmd::agent::flag_val(args, "--provider")
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string()),
        model: crate::cmd::agent::flag_val(args, "--model")
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        created: crate::support::now_epoch() as i64,
    };
    let provider = project.provider.clone();
    let model = project.model.clone();
    let path = project.path.clone();
    match projects::add(cfg, project) {
        Ok(()) => {
            println!("[ok] проект {id}: {name} → {path} ({provider} · {model})");
            0
        }
        Err(e) => {
            eprintln!("agent project add: {e}");
            1
        }
    }
}

fn project_list(cfg: &Config, args: &[String]) -> i32 {
    let registry = match projects::load(cfg) {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("agent project list: {e}");
            return 1;
        }
    };
    if crate::cmd::agent::has(args, "--json") {
        match serde_json::to_string_pretty(&registry) {
            Ok(text) => {
                println!("{text}");
                return 0;
            }
            Err(e) => {
                eprintln!("agent project list: {e}");
                return 1;
            }
        }
    }
    if registry.projects.is_empty() {
        println!("проектов нет: hds agent project add <P> --path DIR");
        return 0;
    }
    for p in &registry.projects {
        println!(
            "{:<6} {:<28} {:<44} {}",
            p.id,
            p.name,
            format!("{} · {}", p.provider, p.model),
            p.path
        );
    }
    0
}

/// `hds agent run --project P [--prompt …]` — один ход в анонимной зоне.
pub fn cmd_run(cfg: &Config, args: &[String]) -> i32 {
    let Some(project_id) = crate::cmd::agent::flag_val(args, "--project") else {
        eprintln!("agent run: нужен --project <P>");
        return 2;
    };
    let opts = RunOptions {
        prompt: crate::cmd::agent::flag_val(args, "--prompt"),
        session: crate::cmd::agent::flag_val(args, "--session"),
        max_iterations: flag_u32(args, "--max-iterations").unwrap_or(12),
        subagents: crate::cmd::agent::has(args, "--subagents"),
        json: crate::cmd::agent::has(args, "--json"),
        quiet: crate::cmd::agent::has(args, "--quiet"),
    };
    let plan = match plan(cfg, &project_id, &opts) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("agent run: {e}");
            return 1;
        }
    };
    if crate::cmd::agent::has(args, "--dry-run") {
        return match serde_json::to_string_pretty(&plan_json(&plan)) {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(e) => {
                eprintln!("agent run: {e}");
                1
            }
        };
    }
    run_cli(cfg, plan, opts)
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

/// То же по каталогу состояния: `history clear` не собирает сессию целиком
/// (провайдер и модель для очистки истории не нужны).
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

fn run_cli(cfg: &Config, plan: RunPlan, opts: RunOptions) -> i32 {
    let root = project_root();
    if let Err(e) = prepare(&plan) {
        eprintln!("agent run: {e}");
        return 1;
    }
    let client = match open_client(cfg, &plan, &root) {
        Ok(client) => client,
        Err(e) => {
            eprintln!("[!!] {e}");
            return 1;
        }
    };
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let result = run(&plan, Arc::clone(&client), &opts, &mut out, &mut err);
    if let Ok(client) = Arc::try_unwrap(client) {
        if let Ok(client) = client.into_inner() {
            client.shutdown();
        }
    }
    match result {
        Ok(report) if report.ok => 0,
        Ok(report) => {
            eprintln!("agent run: ход завершён неуспешно ({})", report.reason);
            1
        }
        Err(e) => {
            eprintln!("agent run: {e}");
            1
        }
    }
}

/// `hds agent history clear --project P [--session ID]` — «Очистить историю»
/// (TR-6.6) без запуска сессии: словарь строится по снимку vault, файлы сессии
/// перезаписываются метками с бэкапами `.json.bak`.
pub fn cmd_history(cfg: &Config, args: &[String]) -> i32 {
    let Some((sub, rest)) = args.split_first() else {
        eprintln!("agent history: нужна подкоманда (clear)");
        return 2;
    };
    if sub != "clear" {
        eprintln!("agent history: неизвестная подкоманда '{sub}' (clear)");
        return 2;
    }
    let Some(project_id) = crate::cmd::agent::flag_val(rest, "--project") else {
        eprintln!("agent history clear: нужен --project <P>");
        return 2;
    };
    let session = crate::cmd::agent::flag_val(rest, "--session");
    match clear_history(cfg, &project_id, session.as_deref(), &project_root()) {
        Ok((session, outcome)) => {
            println!(
                "[ok] история сессии {session} очищена: замен {}, файлов {}, бэкапов {}",
                outcome.replaced,
                outcome.files.len(),
                outcome.backups.len()
            );
            for backup in &outcome.backups {
                println!("[бэкап] {}", backup.display());
            }
            0
        }
        Err(e) => {
            eprintln!("agent history clear: {e}");
            1
        }
    }
}

/// Почистить историю сессии проекта. `session` не задан — берётся последняя
/// сессия из `<P>\\agent-state\\.cline\\data\\sessions`.
///
/// Провайдер и модель здесь не нужны: очистка — это словарь (снимок vault) и
/// перезапись файлов, поэтому `plan` не собирается и Node не запускается.
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
