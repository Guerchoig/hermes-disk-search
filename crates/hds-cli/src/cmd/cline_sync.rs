//! `hds cline-sync` — привести настройки Cline в соответствие с `config.yaml`:
//! окна контекста моделей, MCP-сервер disk-search, правило и скилл.
//!
//! Тот же код, что и у кнопки в UI (`POST /api/cline/sync`); инсталляторы
//! (`install_cline.ps1`, `installers/install_cline_macos.sh`) вызывают эту команду.

use hds_core::cline::{sync, Report, RESTART_NOTE};

/// Печать отчёта (человекочитаемо или `--json` — формат `Report::to_json`).
fn print_report(rep: &Report, as_json: bool) {
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&rep.to_json()).unwrap_or_default()
        );
        return;
    }
    for s in &rep.steps {
        let mark = match s.status {
            "ok" => "[ok]",
            "skip" => "[--]",
            _ => "[!!]",
        };
        println!("{mark} {}: {}", s.title, s.msg);
    }
    if rep.restart_required {
        println!("{RESTART_NOTE}");
    }
}

/// `cmd_cline_sync`: синхронизация настроек Cline; 0 — без предупреждений.
pub fn cmd_cline_sync(as_json: bool, dry_run: bool) -> i32 {
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let rep = sync(&cfg, &hds_core::config::project_root(), dry_run);
    print_report(&rep, as_json);
    if rep.ok() {
        0
    } else {
        1
    }
}
