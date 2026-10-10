//! `hds cline-sync` — привести настройки Cline в соответствие с `config.yaml`:
//! окна контекста моделей, MCP-сервер disk-search, правило и скилл.
//!
//! Тот же код, что и у кнопки в UI (`POST /api/cline/sync`); инсталляторы
//! (`install_cline.ps1`, `installers/install_cline_macos.sh`) вызывают эту команду.

use hds_core::cline::{sync_with, ClientTarget, Report, RESTART_NOTE};

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

/// Локальный ли адрес (`http://127.0.0.1`, `localhost`, `[::1]`) — чтобы не
/// предупреждать про http:// для dev-стенда.
fn is_localhost(url: &str) -> bool {
    let u = url.trim();
    let u = u.strip_prefix("http://").unwrap_or(u);
    let host = u.split('/').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or(host);
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

/// `cmd_cline_sync`: синхронизация настроек Cline; 0 — без предупреждений.
///
/// `server_url`/`token`/`mcp_path` (Э9/M9) — клиентский режим: MCP-запись на
/// **внешний** сервер с заголовком `Authorization: Bearer`. Без `server_url` —
/// прежнее локальное поведение (`config.yaml` → `mcp_http.*`). В клиентском
/// режиме локального `config.yaml` может не быть (тонкий клиент) — тогда берём
/// `Config::Null`.
pub fn cmd_cline_sync(
    as_json: bool,
    dry_run: bool,
    server_url: Option<String>,
    token: Option<String>,
    mcp_path: Option<String>,
) -> i32 {
    let server_url = server_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // Токен: `--token` или `HDS_CLINE_TOKEN` (env) — чтобы не светить секрет в argv
    // (как `hds login` без пароля в argv); мастер задаёт env для дочернего процесса.
    let token = token
        .or_else(|| std::env::var("HDS_CLINE_TOKEN").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if server_url.is_none() && token.is_some() {
        eprintln!(
            "cline-sync: --token задан без --server-url (клиентский режим требует URL сервера)"
        );
        return 2;
    }
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(_) if server_url.is_some() => hds_core::config::Config::Null,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let client = server_url.map(|url| {
        if !url.starts_with("https://") && !is_localhost(&url) {
            eprintln!(
                "[..] внимание: --server-url по http:// без TLS — токен пойдёт открытым текстом"
            );
        }
        ClientTarget {
            server_url: url,
            token: token.unwrap_or_default(),
            mcp_path: mcp_path.unwrap_or_else(|| "/mcp".to_string()),
        }
    });
    let rep = sync_with(
        &cfg,
        &hds_core::config::project_root(),
        dry_run,
        client.as_ref(),
    );
    print_report(&rep, as_json);
    if rep.ok() {
        0
    } else {
        1
    }
}
