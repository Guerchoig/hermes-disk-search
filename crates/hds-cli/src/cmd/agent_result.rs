//! `hds agent result` — решения по вариантам агента (TR-7.6, A4.4).
//!
//! * `accept`   — «Принять»: backup оригинала → атомарная замена вариантом →
//!   статус `accepted` → образ пересоздаётся из нового оригинала → вариант и
//!   основание удаляются;
//! * `reject`   — «Отклонить»: образ возвращается к оригиналу, вариант и
//!   основание удаляются, агенту уходит сообщение в `.hds-agent\messages.jsonl`;
//! * `conflict` — конфликт (оригинал изменился), обязателен `--choice`:
//!   `agent` («оставить вариант агента», backup обязателен), `new` («взять новую
//!   версию»), `delegate` («поручить агенту» — анонимная копия варианта в
//!   `<P>\workspace\_conflicts`, агенту сообщение «перенеси правки»).
//!
//! Ядро решений — `hds_agent::results` (A5 позовёт его из кнопок вкладки
//! «Агент»). Команда только разбирает флаги, поднимает воркеры (как `sync`) и
//! печатает отчёт; коды возврата: 0 — сделано, 1 — отказ (занятый файл,
//! конфликт, нет варианта), 2 — ошибка в аргументах.

use std::path::PathBuf;

use hds_agent::mirror::{LazyAnonymizer, LazyExtractor, Mirror};
use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::results::{ConflictChoice, Decision, DecisionReport, ResultsError};
use hds_agent::{projects, settings};
use hds_core::config::Config;

use crate::cmd::agent_sync::{anonymizer_factory, extractor_factory};

/// Точка входа `hds agent result ...`.
pub fn cmd_result(cfg: &Config, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return 0;
    }
    let Some((action, rest)) = args.split_first() else {
        eprintln!("agent result: нужна подкоманда accept|reject|conflict");
        eprintln!("{}", usage());
        return 2;
    };
    let decision = match action.as_str() {
        "accept" => Decision::Accept,
        "reject" => Decision::Reject,
        "conflict" => match choice(rest) {
            Ok(decision) => decision,
            Err(code) => return code,
        },
        other => {
            eprintln!("agent result: неизвестное решение '{other}' (accept|reject|conflict)");
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
    if let Some(other) = unknown_flag(rest, action == "conflict") {
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
            println!("[i] проект {} — папка {}", project.id, project.path);
            PathBuf::from(project.path)
        }
        None => {
            eprintln!("agent result: проект {id} не зарегистрирован (hds agent project add)");
            return 2;
        }
    };
    if !settings::settings(cfg).enabled {
        eprintln!("agent result: анонимная зона выключена (agent.enabled: false)");
        return 1;
    }
    if !project_path.is_dir() {
        eprintln!(
            "agent result: папка проекта не найдена: {}",
            project_path.display()
        );
        return 1;
    }
    let registry = match ArtifactRegistry::open(&db_path) {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("agent result: {e}");
            return 1;
        }
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
        Err(e) => {
            eprintln!("agent result: {e}");
            return 1;
        }
    };
    println!("[i] решение: {}", decision.as_str());
    match mirror.decide(&rel, decision) {
        Ok(report) => {
            print_report(&report);
            0
        }
        Err(e) => {
            eprintln!("[!] {}", describe(&e));
            1
        }
    }
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
    if let Some(backup) = &report.backup {
        println!("[ok] копия прежней версии: {}", backup.display());
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
fn unknown_flag(args: &[String], with_choice: bool) -> Option<&str> {
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        let is_value_flag =
            arg == "--project" || arg == "--path" || (with_choice && arg == "--choice");
        if is_value_flag {
            skip_value = true;
            continue;
        }
        let known = ["--project=", "--path="]
            .iter()
            .any(|prefix| arg.starts_with(prefix))
            || (with_choice && arg.starts_with("--choice="));
        if !known {
            return Some(arg);
        }
    }
    None
}

fn usage() -> String {
    "использование: hds agent result accept|reject|conflict --project <P> --path <отн. путь>\n\
     \x20              [--choice agent|new|delegate]\n\
     \x20 accept   — «Принять»: backup оригинала, замена вариантом, статус accepted,\n\
     \x20            образ пересоздаётся из нового оригинала (TR-7.6, случай 1)\n\
     \x20 reject   — «Отклонить»: образ возвращается к оригиналу, агенту сообщение\n\
     \x20            (.hds-agent\\messages.jsonl; случай 4)\n\
     \x20 conflict — конфликт, оригинал изменился (случай 2), нужен --choice:\n\
     \x20            agent    — оставить вариант агента (правки оригинала уйдут в backup)\n\
     \x20            new      — взять новую версию (вариант агента отбрасывается)\n\
     \x20            delegate — поручить агенту (копия варианта в workspace\\_conflicts)\n\
     \x20 --path — путь ОРИГИНАЛА в папке проекта, как в реестре: 06 Переписка\\Письмо.md"
        .to_string()
}
