//! `hds agent result` — решения по вариантам агента (TR-7.6, A4.4) и
//! эксплуатационные операции A6.4.
//!
//! * `accept`   — «Принять»: backup оригинала → атомарная замена вариантом →
//!   статус `accepted` → образ пересоздаётся из нового оригинала → вариант и
//!   основание удаляются;
//! * `reject`   — «Отклонить»: образ возвращается к оригиналу, вариант и
//!   основание удаляются, агенту уходит сообщение в `.hds-agent\messages.jsonl`;
//! * `conflict` — конфликт (оригинал изменился), обязателен `--choice`:
//!   `agent` («оставить вариант агента», backup обязателен), `new` («взять новую
//!   версию»), `delegate` («поручить агенту» — анонимная копия варианта в
//!   `<P>\workspace\_conflicts`, агенту сообщение «перенеси правки»);
//! * `restore`  — «Вернуть»: оригинал заменяется копией из
//!   `.hds-agent\backup\<штамп>`, образ пересобирается; `--stamp` выбирает
//!   версию, без него берётся свежая (В-A6-4);
//! * `rebuild`  — «Пересоздать образ» для записи `orphan`;
//! * `relink`   — восстановить запись по маркеру `hds-agent/<P>/<id>`; только
//!   по команде, в проходе зеркала не выполняется (В-A6-5).
//!
//! Ядро — `hds_agent::results` (те же методы зовёт вкладка «Агент»). Команда
//! только разбирает флаги, поднимает воркеры (как `sync`) и печатает отчёт;
//! коды возврата: 0 — сделано, 1 — отказ (занятый файл, нет копии, не orphan,
//! чужой маркер), 2 — ошибка в аргументах. `--json` — отчёт одной строкой JSON.

use std::path::PathBuf;

use hds_agent::mirror::{LazyAnonymizer, LazyExtractor, Mirror};
use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::results::{BackupCopy, ConflictChoice, Decision, DecisionReport, ResultsError};
use hds_agent::{projects, settings};
use hds_core::config::Config;
use serde_json::{json, Value};

use crate::cmd::agent_sync::{anonymizer_factory, extractor_factory};

/// Что делать: решение по варианту (A4.4) или эксплуатационная операция (A6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Decide(Decision),
    Restore,
    Rebuild,
    Relink,
}

impl Action {
    /// Строка для отчёта и журнала (`accept`, `conflict:agent`, `restore`, …).
    fn label(self) -> String {
        match self {
            Action::Decide(decision) => decision.as_str(),
            Action::Restore => "restore".to_string(),
            Action::Rebuild => "rebuild".to_string(),
            Action::Relink => "relink".to_string(),
        }
    }

    /// Флаги, у которых есть значение (`--flag value` / `--flag=value`).
    fn value_flags(self) -> &'static [&'static str] {
        match self {
            Action::Decide(Decision::Conflict(_)) => &["--project", "--path", "--choice"],
            Action::Restore => &["--project", "--path", "--stamp"],
            _ => &["--project", "--path"],
        }
    }
}

/// Точка входа `hds agent result ...`.
pub fn cmd_result(cfg: &Config, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return 0;
    }
    let Some((action, rest)) = args.split_first() else {
        eprintln!("agent result: нужна подкоманда accept|reject|conflict|restore|rebuild|relink");
        eprintln!("{}", usage());
        return 2;
    };
    let json_out = rest.iter().any(|a| a == "--json");
    let action = match action.as_str() {
        "accept" => Action::Decide(Decision::Accept),
        "reject" => Action::Decide(Decision::Reject),
        "conflict" => match choice(rest) {
            Ok(decision) => Action::Decide(decision),
            Err(code) => return code,
        },
        "restore" => Action::Restore,
        "rebuild" => Action::Rebuild,
        "relink" => Action::Relink,
        other => {
            eprintln!(
                "agent result: неизвестное решение '{other}' \
                 (accept|reject|conflict|restore|rebuild|relink)"
            );
            eprintln!("{}", usage());
            return 2;
        }
    };
    let Some(id) = flag_val(rest, "--project") else {
        eprintln!("agent result: нужен --project <P0001>");
        eprintln!("{}", usage());
        return 2;
    };
    let Some(rel) = flag_val(rest, "--path") else {
        eprintln!("agent result: нужен --path <относительный путь в папке проекта>");
        eprintln!("{}", usage());
        return 2;
    };
    let stamp = flag_val(rest, "--stamp");
    if stamp.is_some() && action != Action::Restore {
        eprintln!("agent result: --stamp есть только у restore (выбор копии)");
        eprintln!("{}", usage());
        return 2;
    }
    if let Some(other) = unknown_flag(rest, action.value_flags(), true) {
        eprintln!("agent result: неизвестный аргумент '{other}'");
        eprintln!("{}", usage());
        return 2;
    }
    let Some(db_path) = settings::registry_db(cfg, &id) else {
        eprintln!("agent result: '{id}' не похож на идентификатор проекта (P + 4 цифры, D21)");
        return 2;
    };
    let project_path = match projects::get(cfg, &id) {
        Some(project) => {
            if !json_out {
                println!("[i] проект {} — папка {}", project.id, project.path);
            }
            PathBuf::from(project.path)
        }
        None => {
            eprintln!("agent result: проект {id} не зарегистрирован (hds agent project add)");
            return 2;
        }
    };
    if !settings::settings(cfg).enabled {
        return fail(
            json_out,
            &action,
            "анонимная зона выключена (agent.enabled: false)",
        );
    }
    if !project_path.is_dir() {
        return fail(
            json_out,
            &action,
            &format!("папка проекта не найдена: {}", project_path.display()),
        );
    }
    let registry = match ArtifactRegistry::open(&db_path) {
        Ok(registry) => registry,
        Err(e) => return fail(json_out, &action, &format!("{e}")),
    };

    let root = hds_core::config::project_root();
    let mut anonymizer = LazyAnonymizer::new(anonymizer_factory(cfg, &root));
    let mut extractor = LazyExtractor::new(extractor_factory(&root));
    let mut mirror = match Mirror::from_config(
        cfg,
        &id,
        &project_path,
        &registry,
        &mut anonymizer,
        &mut extractor,
    ) {
        Ok(mirror) => mirror,
        Err(e) => return fail(json_out, &action, &format!("{e}")),
    };
    if !json_out {
        println!("[i] действие: {}", action.label());
    }
    let outcome = match action {
        Action::Decide(decision) => mirror.decide(&rel, decision),
        Action::Restore => mirror.restore(&rel, stamp.as_deref()),
        Action::Rebuild => mirror.rebuild(&rel),
        Action::Relink => mirror.relink(&rel),
    };
    match outcome {
        Ok(report) => {
            if json_out {
                // Копии нужны панели/скрипту для выбора следующего штампа.
                let copies = match action {
                    Action::Restore => mirror.backups(Some(&rel)).unwrap_or_default(),
                    _ => Vec::new(),
                };
                println!("{}", report_json(&action, &report, &copies));
            } else {
                print_report(&report);
            }
            0
        }
        Err(e) => {
            let text = describe(&e);
            if json_out {
                let mut body = json!({
                    "ok": false,
                    "action": action.label(),
                    "error": text,
                });
                if let ResultsError::NoBackup { available, .. } = &e {
                    body["copies"] = json!(available);
                }
                println!("{body}");
            } else {
                eprintln!("[!] {text}");
            }
            1
        }
    }
}

/// Отказ до открытия зеркала: код 1 и тот же формат, что у ошибок операций.
fn fail(json_out: bool, action: &Action, message: &str) -> i32 {
    if json_out {
        println!(
            "{}",
            json!({"ok": false, "action": action.label(), "error": message.trim_end()})
        );
    } else {
        eprintln!("agent result: {message}");
    }
    1
}

/// Текст ошибки для пользователя: отдельная подсказка для занятого файла.
fn describe(err: &ResultsError) -> String {
    match err {
        ResultsError::Busy(path) => format!(
            "файл занят: закройте {} в Word/Excel и повторите — вариант остался в pending, \
             статус не изменён",
            path.display()
        ),
        other => other.to_string(),
    }
}

/// `--choice` для `conflict`: обязателен и проверяется до открытия БД.
fn choice(args: &[String]) -> Result<Decision, i32> {
    let Some(value) = flag_val(args, "--choice") else {
        eprintln!("agent result conflict: нужен --choice agent|new|delegate");
        eprintln!("{}", usage());
        return Err(2);
    };
    match ConflictChoice::parse(&value) {
        Some(choice) => Ok(Decision::Conflict(choice)),
        None => {
            eprintln!("agent result conflict: неизвестный выбор '{value}' (agent|new|delegate)");
            Err(2)
        }
    }
}

fn print_report(report: &DecisionReport) {
    println!("[i] документ: {}", report.orig_rel);
    println!("[i] образ: {}", report.anon_rel);
    if let Some(stamp) = &report.stamp {
        let path = report
            .backup
            .as_ref()
            .map(|backup| backup.display().to_string())
            .unwrap_or_default();
        println!("[ok] копия: {stamp} · {path}");
    }
    if let Some(sha) = &report.sha256 {
        println!("[ok] sha256: {sha}");
    }
    if let Some(copy) = &report.conflict_copy {
        println!("[ok] правки агента для переноса: {copy}");
    }
    println!("[ok] статус: {}", report.status.as_str());
    if let Some(message) = &report.message {
        println!(
            "[ok] сообщение агенту ({}): {}",
            message.kind.as_str(),
            message.text
        );
    }
    for warning in &report.warnings {
        println!("[!] {warning}");
    }
}

/// Отчёт `--json`: одна строка, только пути/статусы/числа (I8).
fn report_json(action: &Action, report: &DecisionReport, copies: &[BackupCopy]) -> Value {
    let mut body = json!({
        "ok": true,
        "action": action.label(),
        "rel": report.orig_rel,
        "anon": report.anon_rel,
        "status": report.status.as_str(),
        "warnings": report.warnings,
    });
    if let Some(stamp) = &report.stamp {
        body["stamp"] = json!(stamp);
    }
    if let Some(sha) = &report.sha256 {
        body["sha256"] = json!(sha);
    }
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
    if !copies.is_empty() {
        body["backups"] = json!(copies);
    }
    body
}

/// Значение флага: `--flag value` и `--flag=value`.
fn flag_val(args: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == name {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix(&prefix) {
            return Some(rest.to_string());
        }
    }
    None
}

/// Первый аргумент, который не является флагом этой подкоманды.
///
/// `value_flags` — флаги со значением (значение пропускается), `--json` —
/// булев флаг отчёта (A6.4).
fn unknown_flag<'a>(args: &'a [String], value_flags: &[&str], allow_json: bool) -> Option<&'a str> {
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if allow_json && arg == "--json" {
            continue;
        }
        if value_flags.iter().any(|flag| arg == flag) {
            skip_value = true;
            continue;
        }
        let known = value_flags
            .iter()
            .any(|flag| arg.starts_with(&format!("{flag}=")));
        if !known {
            return Some(arg);
        }
    }
    None
}

fn usage() -> String {
    [
        "использование: hds agent result <действие> --project <P> --path <отн. путь>",
        "              [--choice agent|new|delegate] [--stamp <штамп>] [--json]",
        " accept   — «Принять»: backup оригинала, замена вариантом, статус accepted,",
        "            образ пересоздаётся из нового оригинала (TR-7.6, случай 1)",
        " reject   — «Отклонить»: образ возвращается к оригиналу, агенту сообщение",
        "            (.hds-agent\\messages.jsonl; случай 4)",
        " conflict — конфликт, оригинал изменился (случай 2), нужен --choice:",
        "            agent    — оставить вариант агента (правки оригинала уйдут в backup)",
        "            new      — взять новую версию (вариант агента отбрасывается)",
        "            delegate — поручить агенту (копия варианта в workspace\\_conflicts)",
        " restore  — «Вернуть»: заменить оригинал копией из .hds-agent\\backup, образ",
        "            пересобран; --stamp выбирает версию, без него — свежая (A6.4)",
        " rebuild  — «Пересоздать образ» из текущего оригинала (только статус orphan)",
        " relink   — восстановить запись по маркеру образа hds-agent/<P>/<id>;",
        "            только по команде, в проходе зеркала не выполняется (A6.4)",
        " --json — отчёт одной строкой JSON (пути, статусы, числа; I8)",
        r" --path — путь ОРИГИНАЛА в папке проекта, как в реестре: 06 Переписка\Письмо.md",
    ]
    .join("\n")
}
