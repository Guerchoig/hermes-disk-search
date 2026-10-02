//! Паритет порта поиска с golden `search_*.json` (W1). Требует:
//! * `.venv` (Python-воркер лемматизации);
//! * фасад эмбеддингов `:8011` (векторная ветка);
//! * БД фикстур `tools/parity/out/index.db` (от `golden.py`) и `golden/`.
//!
//! Запуск: `cargo test -p hds-search --test search_parity -- --ignored --nocapture`.
//! Сравнение — как `compare.py`: ключ `path|page|t_start`; порядок допускает
//! перестановку только между равными скорами.

use std::path::PathBuf;

use hds_core::config::Config;
use hds_index::{Embedder, Lemmatizer, Sidecar};
use hds_search::search;
use serde_json::Value as J;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// `str(float)` в Python: целые — с `.0` (0.0), остальные — кратчайшая запись.
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

fn golden_key(r: &J) -> String {
    rkey(
        r.get("path").and_then(|v| v.as_str()).unwrap_or(""),
        r.get("page").and_then(|v| v.as_i64()),
        r.get("t_start").and_then(|v| v.as_f64()),
    )
}

#[test]
#[ignore = "требует .venv + фасад :8011 + out/index.db + golden"]
fn golden_search_parity() {
    let root = repo();
    let py = root.join(".venv").join("Scripts").join("python.exe");
    let db = root.join("tools").join("parity").join("out").join("index.db");
    let golden = root.join("tools").join("parity").join("golden");
    if !py.exists() || !db.exists() || !golden.join("search_01.json").exists() {
        println!("пропуск: нет .venv / out/index.db / golden");
        return;
    }
    let mut cfg: Config = match hds_core::config::load_from(&root.join("config.yaml")) {
        Ok(c) => c,
        Err(e) => {
            println!("пропуск: config.yaml: {e}");
            return;
        }
    };
    // CLIP выключен (как в golden W0): не грузим модели зря
    if let Some(serde_yaml::Value::Mapping(idx)) = cfg.get_mut("index") {
        idx.insert(
            serde_yaml::Value::String("clip".into()),
            serde_yaml::Value::Bool(false),
        );
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
    .expect("open index.db");

    let mut fails = Vec::new();
    for i in 1..=10 {
        let gp = golden.join(format!("search_{i:02}.json"));
        let g: J = match std::fs::read_to_string(&gp) {
            Ok(s) => serde_json::from_str(&s).unwrap(),
            Err(_) => continue,
        };
        let query = g.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let res = search(&conn, Some(&emb), lem, &cfg, query, None, 20);
        let ka: Vec<String> = res
            .iter()
            .map(|r| rkey(&r.path, r.page, r.t_start))
            .collect();
        let gr = g.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let kg: Vec<String> = gr.iter().map(golden_key).collect();

        if ka == kg {
            println!("Q{i:02} ok ({} результатов)", res.len());
        } else if { let mut a = ka.clone(); a.sort(); let mut b = kg.clone(); b.sort(); a == b } {
            // состав совпал — проверим, что перестановка только среди равных скоров
            let mut bad = false;
            for (x, y) in gr.iter().zip(res.iter()) {
                if golden_key(x) != rkey(&y.path, y.page, y.t_start)
                    && (x.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0) - y.score).abs() > 1e-9
                {
                    bad = true;
                }
            }
            if bad {
                fails.push(format!("Q{i:02}: порядок изменён при разных скорах"));
            } else {
                println!("Q{i:02} ok (состав совпал, порядок среди равных)");
            }
        } else {
            fails.push(format!("Q{i:02}: разный состав топ-20 (got {}, want {})", ka.len(), kg.len()));
        }
    }
    sidecar.shutdown();
    for f in &fails {
        println!("[FAIL] {f}");
    }
    assert!(fails.is_empty(), "паритет поиска не сошёлся: {} ошибок", fails.len());
}
