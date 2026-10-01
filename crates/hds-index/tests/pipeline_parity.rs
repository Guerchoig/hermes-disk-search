//! Паритет конвейера B4 с golden (`tools/parity/golden`) — **строго** по
//! сегментам/чанкам/FTS/хэшам (как `tools/parity/compare.py`).
//!
//! Требует `.venv` (Python-воркер извлечения/лемматизации) и живой фасад
//! эмбеддингов `:8011` (`llm-host`). Если чего-то нет — тест **пропускается**
//! (грабля §9.7.7: тесты с внешними зависимостями не должны ломать `cargo test`).
//!
//! Запуск:
//!   cargo test -p hds-index --test pipeline_parity -- --ignored --nocapture
//!
//! Поиск (`search_*.json`) в B4 не проверяется — порт поиска в W1; compare.py
//! на этом наборе тоже сообщит «ОТСУТСТВУЕТ search_*», это ожидаемо.

use std::io::Read;
use std::path::{Path, PathBuf};

use hds_core::config::Config;
use hds_core::db;
use hds_core::dig;
use hds_index::chunker::make_chunks;
use hds_index::embed::Embedder;
use hds_index::pipeline;
use hds_index::sidecar::{Extractor, Sidecar};
use serde_json::{json, Map, Value};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Чтение golden-файла (обычный `.json` или сжатый `.json.gz`).
fn load_json(path: &Path) -> Option<Value> {
    if path.exists() {
        return serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok();
    }
    let gz = PathBuf::from(format!("{}.gz", path.display()));
    if gz.exists() {
        let mut d = flate2::read::GzDecoder::new(std::fs::File::open(gz).ok()?);
        let mut s = String::new();
        d.read_to_string(&mut s).ok()?;
        return serde_json::from_str(&s).ok();
    }
    None
}

fn seg_json(s: &hds_index::chunker::Segment) -> Value {
    json!({
        "text": s.text,
        "page": s.page,
        "t_start": s.t_start,
        "t_end": s.t_end,
        "head": s.head,
    })
}

/// Копия конфига без `index.exclude_dirs`/`exclude_paths` (для паритета фикстур).
fn without_excludes(mut cfg: Config) -> Config {
    if let Some(serde_yaml::Value::Mapping(idx)) = cfg.get_mut("index") {
        idx.insert(
            serde_yaml::Value::String("exclude_dirs".into()),
            serde_yaml::Value::Sequence(vec![]),
        );
        idx.insert(
            serde_yaml::Value::String("exclude_paths".into()),
            serde_yaml::Value::Sequence(vec![]),
        );
    }
    cfg
}

#[test]
#[ignore = "требует .venv + фасад :8011 + fixtures/golden"]
fn golden_parity_16_fixtures() {
    let root = repo_root();
    let py = root.join(".venv").join("Scripts").join("python.exe");
    if !py.exists() {
        println!("пропуск: нет {}", py.display());
        return;
    }
    let fixtures = root.join("tools").join("parity").join("fixtures");
    let golden = root.join("tools").join("parity").join("golden");
    if !fixtures.is_dir() || !golden.is_dir() {
        println!("пропуск: нет fixtures/golden");
        return;
    }
    let cfg: Config = match hds_core::config::load_from(&root.join("config.yaml")) {
        Ok(c) => c,
        Err(e) => {
            println!("пропуск: config.yaml: {e}");
            return;
        }
    };
    // Фикстуры лежат внутри проекта, а боевой `exclude_dirs` содержит папку
    // проекта (`hermes-disk-search`) — для паритета исключения снимаем
    // (golden.py тоже обходит их: он зовёт extractors.extract напрямую).
    let cfg = without_excludes(cfg);
    let dim = dig(&cfg, "embedding.dim").and_then(|v| v.as_i64()).unwrap_or(1024);

    let emb = Embedder::from_config(&cfg);
    if emb.ping().is_err() {
        println!(
            "пропуск: фасад эмбеддингов недоступен на {} ({})",
            emb.base_url(),
            emb.model()
        );
        return;
    }

    let sidecar = match Sidecar::spawn(&py, &root, true) {
        Ok(s) => s,
        Err(e) => {
            println!("пропуск: воркер не запустился: {}", e.message());
            return;
        }
    };

    let out = root.join("tools").join("parity").join("out").join("rust_parity");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let conn = db::connect(&out.join("index.db"), dim).unwrap();

    let size = pipeline::chunk_size(&cfg);
    let overlap = pipeline::chunk_overlap(&cfg);
    let maxc = pipeline::max_chunks(&cfg);

    let mut names: Vec<String> = std::fs::read_dir(&fixtures)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    let mut fails: Vec<String> = Vec::new();
    let mut ok = 0usize;
    let mut hash_manifest = Map::new();
    let mut total_chunks = 0usize;

    for name in &names {
        let path = fixtures.join(name);
        let stem = Path::new(name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());

        // сегменты — как в golden.py (kind None → "unknown", segments [])
        let (mut kind, mut segments) = match sidecar.extract(&path) {
            Ok(v) => v,
            Err(e) => {
                fails.push(format!("{name}: воркер: {}", e.message()));
                continue;
            }
        };
        if kind.is_empty() {
            kind = "unknown".into();
            segments = Vec::new();
        }
        let segs_value = json!({
            "file": name,
            "kind": kind,
            "error": "",
            "segments": segments.iter().map(seg_json).collect::<Vec<_>>(),
        });

        // конвейер: process_file (чанки + FTS + векторы через фасад)
        let (status, _k) = match pipeline::process_file(
            &conn, &cfg, &emb, &path, false, &sidecar, &sidecar, None,
        ) {
            Ok(v) => v,
            Err(e) => {
                fails.push(format!("{name}: process_file: {}", e.message()));
                continue;
            }
        };
        if !status.starts_with("indexed") {
            fails.push(format!("{name}: статус {status}"));
            continue;
        }

        let fid = db::get_file_by_path(&conn, &pipeline::path_str(&path))
            .unwrap()
            .unwrap()
            .id;
        let chunks = read_chunks(&conn, fid);
        let fts = read_fts(&conn, fid);
        let full_len = if segments.is_empty() {
            0
        } else {
            make_chunks(&segments, size, overlap).len()
        };
        let cut = if maxc > 0 && full_len > maxc {
            full_len - maxc
        } else {
            0
        };
        let chunks_value = json!({"file": name, "kind": kind, "cut": cut, "chunks": chunks});
        let fts_value = json!({"file": name, "fts": fts});
        total_chunks += chunks.len();

        let sz = std::fs::metadata(&path).unwrap().len();
        let chash = hds_index::content_hash(&path, sz).ok();
        hash_manifest.insert(
            name.clone(),
            json!({"kind": kind, "size": sz, "content_hash": chash}),
        );

        compare_strict(&golden, &format!("{stem}.segments.json"), &segs_value, &mut fails);
        compare_strict(&golden, &format!("{stem}.chunks.json"), &chunks_value, &mut fails);
        compare_strict(&golden, &format!("{stem}.fts.json"), &fts_value, &mut fails);
        ok += 1;
    }

    compare_strict(
        &golden,
        "hash_manifest.json",
        &Value::Object(hash_manifest),
        &mut fails,
    );

    println!(
        "паритет B4: файлов={} ок={} чанков={} несовпадений={}",
        names.len(),
        ok,
        total_chunks,
        fails.len()
    );
    for f in &fails {
        println!("[FAIL] {f}");
    }
    sidecar.shutdown();
    assert!(fails.is_empty(), "паритет не сошёлся: {} ошибок", fails.len());
}

/// Чанки файла из БД в формате golden (`text/page/t_start/t_end`).
fn read_chunks(conn: &rusqlite::Connection, fid: i64) -> Vec<Value> {
    let mut st = conn
        .prepare("SELECT page, t_start, t_end, text FROM chunks WHERE file_id=?1 ORDER BY ord")
        .unwrap();
    let rows = st
        .query_map([fid], |r| {
            Ok(json!({
                "text": r.get::<_, String>(3)?,
                "page": r.get::<_, Option<i64>>(0)?,
                "t_start": r.get::<_, Option<f64>>(1)?,
                "t_end": r.get::<_, Option<f64>>(2)?,
            }))
        })
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

/// Лемматизированный FTS-текст файла из `chunks_fts` (по порядку чанков).
fn read_fts(conn: &rusqlite::Connection, fid: i64) -> Vec<String> {
    let mut st = conn
        .prepare(
            "SELECT cf.text FROM chunks c JOIN chunks_fts cf ON cf.rowid=c.id \
             WHERE c.file_id=?1 ORDER BY c.ord",
        )
        .unwrap();
    let rows = st
        .query_map([fid], |r| r.get::<_, String>(0))
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

/// Строгое сравнение с golden-файлом (`.json` или `.json.gz`); расхождение — в `fails`.
///
/// Значения прогоняются через [`scrub`]: сообщение о сбое транскрипции содержит
/// **имя временного wav** (`tmpXXXX.wav`), которое меняется от прогона к прогону
/// (та же грабля волатильности, что в §9.7 README).
fn compare_strict(golden: &Path, name: &str, actual: &Value, fails: &mut Vec<String>) {
    match load_json(&golden.join(name)) {
        None => fails.push(format!("ОТСУТСТВУЕТ golden: {name}")),
        Some(g) if scrub(&g) == scrub(actual) => {}
        Some(_) => fails.push(format!("НЕ СОВПАЛО (строго): {name}")),
    }
}

/// Рекурсивная замена волатильных фрагментов (`tmpXXXX.wav` → `tmpTMP.wav`).
fn scrub(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(scrub_str(s)),
        Value::Array(a) => Value::Array(a.iter().map(scrub).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), scrub(x))).collect()),
        other => other.clone(),
    }
}

fn scrub_str(s: &str) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    while i < s.len() {
        if s[i..].starts_with("tmp") {
            let mut j = i + 3;
            let bytes = s.as_bytes();
            while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
                j += 1;
            }
            // `tmpXXXX` (имя временного файла; в FTS `.wav` уже отделён пробелом)
            if j > i + 3 {
                out.push_str("tmpTMP");
                i = j;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}