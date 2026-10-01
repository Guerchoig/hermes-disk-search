//! `hds status` — порт `hds/cli.py::cmd_status` (`db::stats` теми же полями).

use hds_core::db::Stats;
use serde_json::{json, Value};

use crate::support::{fmt_local_datetime, open_conn};

/// JSON `db.stats` — те же поля, что `dict` в Python
/// (`by_kind`/`by_status`/`chunks`/`last_indexed_at`/`errors`; пары — массивами).
pub fn stats_json(st: &Stats) -> Value {
    let by_kind: Vec<Value> = st.by_kind.iter().map(|(k, n)| json!([k, n])).collect();
    let by_status: Vec<Value> = st.by_status.iter().map(|(k, n)| json!([k, n])).collect();
    let errors: Vec<Value> = st.errors.iter().map(|(p, e)| json!([p, e])).collect();
    json!({
        "by_kind": by_kind,
        "by_status": by_status,
        "chunks": st.chunks,
        "last_indexed_at": st.last_indexed_at,
        "errors": errors,
    })
}

/// Человекочитаемый вывод — дословно как `cmd_status` (типы — `?` при пустом виде).
pub fn stats_text(st: &Stats) -> String {
    let kinds = st
        .by_kind
        .iter()
        .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_else(|| "?".into()), n))
        .collect::<Vec<_>>()
        .join(", ");
    let statuses = st
        .by_status
        .iter()
        .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_else(|| "None".into()), n))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = String::new();
    out.push_str(&format!("Файлы по типам: {kinds}\n"));
    out.push_str(&format!("Файлы по статусам: {statuses}\n"));
    out.push_str(&format!("Чанков всего: {}\n", st.chunks));
    if let Some(ts) = st.last_indexed_at {
        out.push_str(&format!("Последняя индексация: {}\n", fmt_local_datetime(ts)));
    }
    if !st.errors.is_empty() {
        out.push_str("Последние ошибки:\n");
        for (p, e) in &st.errors {
            let msg: String = e.clone().unwrap_or_default().chars().take(120).collect();
            out.push_str(&format!("  {p} :: {msg}\n"));
        }
    }
    out
}

/// `cmd_status(json)`: печать состояния индекса; 0 — успех.
pub fn cmd_status(as_json: bool) -> i32 {
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let conn = match open_conn(&cfg) {
        Ok(c) => c,
        Err(e) => {
            println!("База данных недоступна: {}", e.message());
            return 1;
        }
    };
    match hds_core::db::stats(&conn) {
        Ok(st) => {
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&stats_json(&st)).unwrap_or_default()
                );
            } else {
                print!("{}", stats_text(&st));
            }
            0
        }
        Err(e) => {
            println!("База данных: {}", e.message());
            1
        }
    }
}
