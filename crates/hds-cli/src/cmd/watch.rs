//! `hds watch` — порт `hds/cli.py::cmd_watch`: наблюдатель ФС (B5, `run_watch`).
//!
//! Нужен в том числе `db-move` (перезапускает watcher после переноса БД).

use hds_core::config::project_root;
use hds_index::run_watch;

use crate::support::{build_embedder, build_sidecar, open_conn, parse_roots};

/// `cmd_watch(roots)`: `--roots a;b` или `index.roots`; возвращает код `run_watch`.
pub fn cmd_watch(roots: Option<String>) -> i32 {
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
            println!("Воркер извлечения: {}", e.message());
            return 1;
        }
    };
    let r = roots.map(|s| parse_roots(&s)).filter(|v| !v.is_empty());
    let res = run_watch(&conn, &cfg, &emb, &sidecar, &sidecar, r);
    sidecar.shutdown();
    match res {
        Ok(code) => code,
        Err(e) => {
            println!("Ошибка: {}", e.message());
            1
        }
    }
}
