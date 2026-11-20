//! `hds agent project add|list`, `hds agent run` и `hds agent history clear` —
//! точка входа анонимной зоны в CLI (A3.3, A5.2).
//!
//! С A5.2 план хода переехал в `hds_agent::run` — тот же код собирает вкладка
//! «Агент»: реестр проектов (`projects.json`), секреты (`secrets.json`),
//! каталоги `<P>\workspace|agent-config|agent-state`, окружение процесса
//! (TR-3.2), правила (`rules::write_rules`), `cline_mcp_settings.json`, снимок
//! vault, словарный фильтр и [`Gateway`] поверх Node-воркера. Здесь — тонкая
//! обёртка: разбор аргументов, чтение реплики, печать событий, коды возврата.
//!
//! Успех хода определяется событием `GatewayEvent::TurnEnd` (A3.2), а не
//! ответом `send`; срабатывания фильтра печатаются предупреждениями.
//!
//! `--dry-run` печатает план (без ключей) и не запускает ни Python, ни Node —
//! этим пользуются тесты и диагностика.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use hds_agent::deanon::DeAnonymizer;
use hds_agent::filter::sys_values;
use hds_agent::gateway::{Gateway, GatewayEvent, GatewayHost, NoAskUser, RefreshState};
use hds_agent::history;
use hds_agent::projects::{self, Project};
use hds_anonymize::Client;
use hds_core::config::{project_root, Config};

// План хода живёт в `hds_agent::run` (A5.2): CLI и вкладка «Агент» собирают
// сессию одинаково. Здесь реэкспорт, чтобы тесты и `cmd_*` не менялись.
pub use hds_agent::run::{
    build_filter, clear_history, open_client, open_client_at, plan, plan_json, plan_with, prepare,
    provider_kind, resolve_node, resolve_worker, system_values, tavily_script, RunOptions, RunPlan,
    TurnReport,
};

// Провайдер и модель нового проекта по умолчанию (В-12) — одна копия на весь
// workspace: `hds_agent::projects`. Здесь реэкспорт, чтобы не менять вызовы.
pub use hds_agent::projects::{DEFAULT_MODEL, DEFAULT_PROVIDER};
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
    let secrets = plan.secret_strings();
    let filter = Arc::new(build_filter(
        &plan.project.id,
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
        Path::new(&plan.project.path),
    )));
    let egress_log = plan
        .settings
        .egress_log
        .then(|| plan.state_dir.join("egress"));
    // TR-6.5: снимок сверяется по `vault_rev` перед каждым запросом. Без этого
    // метки, созданные репликой этого же хода, не деанонимизируются, а ход
    // дольше `max_stale` упирается в fail-closed.
    let refresh = Arc::new(RefreshState {
        system: plan.system.clone(),
        projects: plan.project_names.clone(),
        secrets: secrets.clone(),
        workspace: plan.workspace.clone(),
        project_root: PathBuf::from(&plan.project.path),
        deanon: Arc::clone(&deanon),
    });
    let host = Arc::new(
        GatewayHost::new(
            plan.project.id.clone(),
            &plan.workspace,
            Arc::clone(&filter),
            Arc::clone(&client),
            Arc::new(NoAskUser),
            egress_log,
        )
        .with_refresh(refresh),
    );
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
    if plan.settings.disk_search_enabled && plan.mcp.token.is_none() {
        eprintln!(
            "[warn] у проекта {project_id} нет plain-токена (projects.{project_id}.token в \
             secrets.json): сервер disk-search не будет подключён. Выдайте токен заново \
             `hds agent token {project_id}` (он сохраняет и хэш, и plain-токен)."
        );
    }
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

fn run_cli(cfg: &Config, plan: RunPlan, opts: RunOptions) -> i32 {
    let root = project_root();
    // `prepare` делает `run` (каталоги, правила, MCP-настройки): второй вызов
    // здесь писал бы те же файлы и вызывал `icacls` дважды.
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
