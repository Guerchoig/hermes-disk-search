//! `hds agent sync` — демон двустороннего зеркала папки проекта и
//! `<P>\workspace` (фаза A4, TR-7.1…7.9).
//!
//! **A4.1 (этот под-этап):** команда открывает (при необходимости создаёт)
//! реестр артефактов `data\agent\registry\<P>.db` и печатает сводку. Обход
//! дерева, прямой и обратный путь, варианты TR-7.6 и валидатор добавляются
//! под-этапами A4.2…A4.5; циклический режим (`--interval`) — A4.6.
//!
//! Порядок именно такой: реестр — то, что переживает перезапуск и потерю
//! папки, и без него остальные шаги нечем проверять.
//!
//! Ошибка реестра **не** валит будущий демон: в цикле она станет статусом
//! `error` и записью в журнал (TR-7.2). CLI же честно возвращает код 1.

use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::{projects, settings};
use hds_core::config::Config;

/// Точка входа `hds agent sync ...`.
pub fn cmd_sync(cfg: &Config, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return 0;
    }
    let Some(id) = crate::cmd::agent::flag_val(args, "--project") else {
        eprintln!("agent sync: нужен --project <P0001>");
        eprintln!("{}", usage());
        return 2;
    };
    if let Some(other) = unknown_flag(args) {
        eprintln!("agent sync: неизвестный аргумент '{other}'");
        eprintln!("{}", usage());
        return 2;
    }
    let Some(db_path) = settings::registry_db(cfg, &id) else {
        eprintln!("agent sync: '{id}' не похож на идентификатор проекта (P + 4 цифры, D21)");
        return 2;
    };
    match projects::get(cfg, &id) {
        Some(project) => {
            println!("[i] проект {} — папка {}", project.id, project.path);
        }
        None => {
            eprintln!("agent sync: проект {id} не зарегистрирован (hds agent project add)");
            return 2;
        }
    }
    if !settings::settings(cfg).enabled {
        eprintln!("agent sync: анонимная зона выключена (agent.enabled: false)");
        return 1;
    }

    let registry = match ArtifactRegistry::open(&db_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("agent sync: {e}");
            return 1;
        }
    };
    let (count, pending) = match (registry.count(), registry.list_pending()) {
        (Ok(count), Ok(pending)) => (count, pending.len()),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("agent sync: {e}");
            return 1;
        }
    };
    println!("[ok] реестр: {}", registry.path().display());
    println!("[ok] записей: {count}, ждут решения: {pending}");
    println!("[i] обход дерева и зеркалирование — под-этапы A4.2…A4.5, демон — A4.6");
    0
}

/// Первый аргумент, который не является флагом, известным подкоманде.
fn unknown_flag(args: &[String]) -> Option<&str> {
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        match arg.as_str() {
            "--project" => skip_value = true,
            "--once" => {}
            other if other.starts_with("--project=") => {}
            other => return Some(other),
        }
    }
    None
}

fn usage() -> String {
    "использование: hds agent sync --project <P> [--once]\n\
     \x20 (A4.1: открывает/создаёт реестр data\\agent\\registry\\<P>.db и печатает\n\
     \x20  сводку. Обход и зеркалирование — A4.2…A4.5, --interval — A4.6;\n\
     \x20  путь реестра: <state_dir>\\data\\agent\\registry\\<P>.db, TR-7.2)"
        .to_string()
}
