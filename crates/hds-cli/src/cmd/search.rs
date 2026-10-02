//! `hds search` — порт `hds/cli.py::cmd_search`: гибридный поиск (FTS+vec+CLIP).
//!
//! Зависит от роли эмбеддингов (`embedding.base_url`) и Python-воркера лемматизации
//! (как конвейер). Формат вывода — как Python (`[N] location (score)` + сниппет).

use hds_core::config::project_root;

use crate::support::{build_embedder, build_sidecar, open_conn, parse_kinds};

/// `cmd_search`: 0 — успех (в т.ч. когда ничего не найдено — как Python: 1);
/// возвращает 1 при пустой выдаче, 1 при ошибке конфигурации/воркера.
pub fn cmd_search(query: String, kinds: Option<String>, limit: usize, json: bool) -> i32 {
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
    let kinds_v = kinds.and_then(|k| parse_kinds(&k));
    let kinds_slice = kinds_v.as_deref().filter(|k| !k.is_empty());
    let res = hds_search::search(&conn, Some(&emb), &sidecar, &cfg, &query, kinds_slice, limit);
    sidecar.shutdown();

    if json {
        let arr: Vec<serde_json::Value> = res.iter().map(|r| r.to_json()).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Array(arr)).unwrap_or_default()
        );
        return 0;
    }
    if res.is_empty() {
        println!("Ничего не найдено.");
        return 1;
    }
    for (i, r) in res.iter().enumerate() {
        println!(
            "\n[{}] {}  (score {:.4})",
            i + 1,
            r.location(),
            r.score
        );
        let snip: String = r.snippet.replace('\n', " ").chars().take(600).collect();
        println!("    {snip}");
    }
    0
}
