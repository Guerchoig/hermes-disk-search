//! Паритет Rust-поиска на БОЕВОЙ БД по контрольным запросам заказчика
//! (`tools/parity/golden/real_db_queries.json`, W0 §12.3). Требует:
//! .venv (воркер), фасад эмбеддингов `:8011`, боевую БД из `config.yaml`, golden.
//!
//! Запуск: `cargo test -p hds-search --test real_db_parity -- --ignored --nocapture`.

use std::path::PathBuf;

use hds_core::config::Config;
use hds_index::{Embedder, Lemmatizer, Sidecar};
use hds_search::search;
use serde_json::Value as J;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn py_f(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

fn rkey(path: &str, page: Option<i64>, t_start: Option<f64>) -> String {
    let p = page.map(|x| x.to_string()).unwrap_or_else(|| "None".into());
    let t = t_start.map(py_f).unwrap_or_else(|| "None".into());
    format!("{path}|{p}|{t}")
}

fn gkey(r: &J) -> String {
    rkey(
        r.get("path").and_then(|v| v.as_str()).unwrap_or(""),
        r.get("page").and_then(|v| v.as_i64()),
        r.get("t_start").and_then(|v| v.as_f64()),
    )
}

#[test]
#[ignore = "требует .venv + фасад :8011 + боевую БД + golden"]
fn real_db_control_queries() {
    let root = repo();
    let py = root.join(".venv").join("Scripts").join("python.exe");
    let golden = root.join("tools").join("parity").join("golden").join("real_db_queries.json");
    if !py.exists() || !golden.exists() {
        println!("пропуск: нет .venv / golden");
        return;
    }
    let mut cfg: Config = match hds_core::config::load_from(&root.join("config.yaml")) {
        Ok(c) => c,
        Err(e) => {
            println!("пропуск: config.yaml: {e}");
            return;
        }
    };
    if let Some(serde_yaml::Value::Mapping(idx)) = cfg.get_mut("index") {
        idx.insert(serde_yaml::Value::String("clip".into()), serde_yaml::Value::Bool(false));
    }
    let db = hds_core::config::db_abs_path(&cfg);
    if !db.exists() {
        println!("пропуск: нет боевой БД {}", db.display());
        return;
    }
    let emb = Embedder::from_config(&cfg);
    if emb.embed_query("проверка").is_err() {
        println!("пропуск: фасад эмбеддингов {} недоступен", emb.base_url());
        return;
    }
    let sidecar = match Sidecar::spawn(&py, &root, false) {
        Ok(s) => s,
        Err(e) => {
            println!("пропуск: воркер: {}", e.message());
            return;
        }
    };
    let lem: &dyn Lemmatizer = &sidecar;

    hds_core::db::register_vec0();
    let conn = rusqlite::Connection::open_with_flags(
        &db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open real db");

    let g: J = serde_json::from_str(&std::fs::read_to_string(&golden).unwrap()).unwrap();
    let mut fails = Vec::new();
    for q in g["queries"].as_array().cloned().unwrap_or_default() {
        let search_q = q.get("search_query").and_then(|v| v.as_str()).unwrap_or("");
        let label = q.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let res = search(&conn, Some(&emb), lem, &cfg, search_q, None, 20);
        let ka: Vec<String> = res.iter().map(|r| rkey(&r.path, r.page, r.t_start)).collect();
        let gr = q.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let kg: Vec<String> = gr.iter().map(gkey).collect();
        if ka == kg {
            println!("«{label}»: ok ({} результатов, порядок совпал)", res.len());
            continue;
        }
        let mut a = ka.clone();
        a.sort();
        let mut b = kg.clone();
        b.sort();
        let overlap = a.iter().filter(|k| b.contains(k)).count();
        let top1_ok = ka.first() == kg.first();
        println!(
            "«{label}»: got {} want {}, пересечение {}/20, топ-1 {}",
            ka.len(),
            kg.len(),
            overlap,
            if top1_ok { "совпал" } else { "ИНОЙ" }
        );
        let miss: Vec<&String> = kg.iter().filter(|k| !ka.contains(k)).collect();
        for m in miss.iter().take(6) {
            println!("   нет в Rust: {m}");
        }
        // боевая БД дрейфует с момента golden (watcher) → допуск: топ-1 + пересечение ≥16
        if !(top1_ok && overlap >= 16) {
            fails.push(format!("«{label}»: топ-1 {top1_ok}, пересечение {overlap}/20"));
        }
    }
    sidecar.shutdown();
    for f in &fails {
        println!("[FAIL] {f}");
    }
    assert!(fails.is_empty(), "паритет на боевой БД не сошёлся: {} ошибок", fails.len());
}
