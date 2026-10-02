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
        let items: Vec<serde_json::Value> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id, "status": c.status, "title": c.title, "msg": c.msg, "fix": c.fix
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "ok": ok, "checks": items }));
        return if ok { 0 } else { 1 };
    }

    println!("== hermes-disk-search: проверка окружения ==");
    for c in &checks {
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
