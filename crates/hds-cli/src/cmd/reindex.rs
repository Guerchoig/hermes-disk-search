//! `hds reindex <path>` — порт `hds/cli.py::cmd_reindex`
//! (`pipeline::reindex_path`: файл или дерево, `force = !--no-force`).

use std::path::PathBuf;

use hds_core::config::project_root;

use crate::support::{build_embedder, build_sidecar, open_conn};

/// Печатает статус каждого обработанного файла (как Python `print(status)`).
pub fn cmd_reindex(path: &str, no_force: bool) -> i32 {
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
    let p: PathBuf = std::path::absolute(path).unwrap_or_else(|_| PathBuf::from(path));
    let res = hds_index::pipeline::reindex_path(&conn, &cfg, &emb, &sidecar, &sidecar, &p, !no_force);
    sidecar.shutdown();
    match res {
        Ok(items) => {
            for (status, _kind) in items {
                println!("{status}");
            }
            0
        }
        Err(e) => {
            println!("Ошибка: {}", e.message());
            1
        }
    }
}
