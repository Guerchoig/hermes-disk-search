//! `hds ask` — порт `hds/cli.py::cmd_ask`: RAG-ответ по локальным файлам (поиск + чат-роль).

use hds_core::config::project_root;

use crate::support::{build_embedder, build_sidecar, open_conn};

/// `cmd_ask`: печатает `answer` (или JSON `{answer, sources}`); 0 — как Python.
pub fn cmd_ask(question: String, limit: usize, json: bool) -> i32 {
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
    let emb = build_embedder(&cfg);
    let root = project_root();
    let sidecar = match build_sidecar(&root) {
        Ok(s) => s,
        Err(e) => {
            println!("Воркер извлечения/лемматизации: {}", e.message());
            return 1;
        }
    };
    let out = hds_search::ask(&conn, Some(&emb), &sidecar, &cfg, &question, limit);
    sidecar.shutdown();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&out.to_json()).unwrap_or_default()
        );
    } else {
        println!("{}", out.answer);
    }
    0
}
