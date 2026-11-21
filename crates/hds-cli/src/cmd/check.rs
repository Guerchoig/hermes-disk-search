//! `hds check` — печать проверок окружения и итог (порт `hds/diag.py::run_checks`).
//!
//! Сама логика проверок живёт в [`hds_index::diag::run_checks`] — общий код с
//! веб-интерфейсом (`/api/diagnostics`); здесь только форматирование вывода
//! (`[ok]/[--]/[!!]` + `-> fix`, как `cmd_check` в Python) и `--json`.

/// `cmd_check`: печать проверок и итог; 1 — есть `fail`. `json` — машинный вывод
/// (`{"ok":…, "checks":[…]}`), его использует веб-интерфейс.
pub fn cmd_check(json: bool) -> i32 {
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "ok": false, "error": e.message() })
                );
            } else {
                println!("== hermes-disk-search: проверка окружения ==");
                println!("[!!] config.yaml недоступен: {}", e.message());
                println!("Итог: есть критические проблемы");
            }
            return 1;
        }
    };
    let checks = hds_index::diag::run_checks(&cfg);
    let ok = !checks.iter().any(|c| c.status == "fail");

    if json {
        println!(
            "{}",
            serde_json::json!({ "ok": ok, "checks": checks_json(&checks) })
        );
        return if ok { 0 } else { 1 };
    }

    println!("== hermes-disk-search: проверка окружения ==");
    print_checks(&checks);
    println!(
        "Итог: {}",
        if ok {
            "основные компоненты готовы"
        } else {
            "есть критические проблемы"
        }
    );
    if ok {
        0
    } else {
        1
    }
}

/// Пункты в человекочитаемом виде (`[ok]`/`[--]`/`[!!]`, `msg`, `-> fix`).
///
/// A6.1: тот же формат печатает `hds agent check` — вывод `hds check` от
/// выделения не меняется.
pub(crate) fn print_checks(checks: &[hds_index::diag::Check]) {
    for c in checks {
        if c.status == "ok" {
            println!("[ok] {}", c.title);
            continue;
        }
        println!(
            "[{}] {}",
            if c.status == "fail" { "!!" } else { "--" },
            c.title
        );
        if !c.msg.is_empty() {
            println!("     {}", c.msg);
        }
        if !c.fix.is_empty() {
            println!("     -> {}", c.fix);
        }
    }
}

/// Пункты для `--json` (`{"id","status","title","msg","fix"}`).
pub(crate) fn checks_json(checks: &[hds_index::diag::Check]) -> Vec<serde_json::Value> {
    checks
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id, "status": c.status, "title": c.title, "msg": c.msg, "fix": c.fix
            })
        })
        .collect()
}
