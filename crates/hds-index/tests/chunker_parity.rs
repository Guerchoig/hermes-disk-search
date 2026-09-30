//! Паритет чанкера с Python-версией (B3, `PLAN_W2_LLM_HOST.md` §5).
//!
//! Для каждой фикстуры golden-набора: `*.segments.json` (+ `.gz`) →
//! `hds_index::chunker::make_chunks(size, overlap)` из `manifest.json` →
//! сравнение с `*.chunks.json` (+ `.gz`) **пополе** (текст, page, t_start, t_end),
//! плюс сверка `n_segments`/`n_chunks`/`cut` из манифеста (обрезка `max_chunks`).
//!
//! Золотые файлы закоммичены (`tools/parity/golden`, 1,87 МБ) — тест работает
//! без окружения Python. Крупные лежат как `.json.gz` (`slim_golden.py`).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use hds_index::chunker::{make_chunks, Chunk, Segment};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn golden_dir() -> PathBuf {
    repo_root().join("tools/parity/golden")
}

/// Прочитать JSON-файл (при необходимости — распаковав gzip).
fn read_json_text(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("нет {}: {e}", path.display()));
    if path.extension().map(|e| e == "gz").unwrap_or(false) {
        let mut out = String::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_string(&mut out)
            .unwrap_or_else(|e| panic!("gunzip {}: {e}", path.display()));
        out
    } else {
        String::from_utf8(bytes).expect("UTF-8")
    }
}

/// Параметры golden-прогона (манифест создаёт `tools/parity/golden.py`).
struct Manifest {
    size: usize,
    overlap: i64,
    max_chunks: usize,
    files: BTreeMap<String, serde_json::Value>,
}

fn load_manifest() -> Manifest {
    let path = golden_dir().join("manifest.json");
    let v: serde_json::Value = serde_json::from_str(&read_json_text(&path)).expect("manifest.json");
    let files = v["files"]
        .as_object()
        .expect("files")
        .iter()
        .map(|(k, val)| (k.clone(), val.clone()))
        .collect();
    Manifest {
        size: v["chunk"]["size"].as_u64().unwrap_or(800) as usize,
        overlap: v["chunk"]["overlap"].as_i64().unwrap_or(120),
        max_chunks: v["max_chunks"].as_u64().unwrap_or(3000) as usize,
        files,
    }
}

/// Найти golden-файл по имени с любым из расширений (.json / .json.gz).
fn find_golden(dir: &Path, name: &str) -> Option<PathBuf> {
    let plain = dir.join(name);
    if plain.is_file() {
        return Some(plain);
    }
    let gz = dir.join(format!("{name}.gz"));
    if gz.is_file() {
        return Some(gz);
    }
    None
}

/// Короткая цитата текста для сообщения о расхождении (без разрыва UTF-8).
fn head_of(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

/// Первое различие двух списков чанков (индекс + причина).
fn first_diff(actual: &[Chunk], expected: &[Chunk]) -> Option<String> {
    if actual.len() != expected.len() {
        return Some(format!(
            "число чанков: Rust {} против Python {}",
            actual.len(),
            expected.len()
        ));
    }
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        if a == e {
            continue;
        }
        let mut why = Vec::new();
        if a.text != e.text {
            why.push(format!(
                "text: Rust «{}» против Python «{}»",
                head_of(&a.text, 120),
                head_of(&e.text, 120)
            ));
        }
        if a.page != e.page {
            why.push(format!("page: {:?} против {:?}", a.page, e.page));
        }
        if a.t_start != e.t_start {
            why.push(format!("t_start: {:?} против {:?}", a.t_start, e.t_start));
        }
        if a.t_end != e.t_end {
            why.push(format!("t_end: {:?} против {:?}", a.t_end, e.t_end));
        }
        return Some(format!("чанк #{i}: {}", why.join("; ")));
    }
    None
}

/// Главная проверка: чанкер повторяет Python на всём golden-наборе.
#[test]
fn chunker_matches_golden() {
    let man = load_manifest();
    let dir = golden_dir();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("каталог golden")
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            n.ends_with(".segments.json") || n.ends_with(".segments.json.gz")
        })
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "в golden нет ни одного *.segments.json");

    let mut checked_chunks = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for seg_path in &entries {
        let name = seg_path.file_name().unwrap().to_string_lossy().to_string();
        let stem = name
            .trim_end_matches(".gz")
            .trim_end_matches(".segments.json")
            .to_string();

        let seg_json: serde_json::Value =
            serde_json::from_str(&read_json_text(seg_path)).expect("segments json");
        let segments: Vec<Segment> =
            serde_json::from_value(seg_json["segments"].clone()).expect("segments");

        let Some(chunks_path) = find_golden(&dir, &format!("{stem}.chunks.json")) else {
            continue;
        };
        let ch_json: serde_json::Value =
            serde_json::from_str(&read_json_text(&chunks_path)).expect("chunks json");
        let expected: Vec<Chunk> =
            serde_json::from_value(ch_json["chunks"].clone()).expect("chunks");

        // Полный результат чанкера + обрезка max_chunks (её делает конвейер B4)
        let full = make_chunks(&segments, man.size, man.overlap);
        let actual: Vec<Chunk> = if man.max_chunks > 0 && full.len() > man.max_chunks {
            full[..man.max_chunks].to_vec()
        } else {
            full.clone()
        };
        let cut = full.len().saturating_sub(actual.len());

        let mut problems: Vec<String> = Vec::new();
        if let Some(diff) = first_diff(&actual, &expected) {
            problems.push(diff);
        }
        // Числа из манифеста (n_segments / n_chunks / cut) — если фикстура там есть
        if let Some((key, meta)) = man
            .files
            .iter()
            .find(|(k, _)| k.starts_with(&format!("{stem}.")))
        {
            let m_segs = meta["n_segments"].as_u64().unwrap_or(0) as usize;
            let m_chunks = meta["n_chunks"].as_u64().unwrap_or(0) as usize;
            let m_cut = meta["cut"].as_u64().unwrap_or(0) as usize;
            if m_segs != segments.len() {
                problems.push(format!(
                    "{key}: сегментов {} против {}",
                    segments.len(),
                    m_segs
                ));
            }
            if m_chunks != actual.len() || m_cut != cut {
                problems.push(format!(
                    "{key}: чанков {} (cut {}) против {} (cut {})",
                    actual.len(),
                    cut,
                    m_chunks,
                    m_cut
                ));
            }
        }

        if problems.is_empty() {
            println!(
                "{stem}: ok ({} сегментов → {} чанков, cut {})",
                segments.len(),
                actual.len(),
                cut
            );
            checked_chunks += actual.len();
        } else {
            failures.push(format!("{stem}: {}", problems.join(" | ")));
        }
    }

    assert!(
        failures.is_empty(),
        "расхождения чанкера:\n{}",
        failures.join("\n")
    );
    assert!(checked_chunks > 0, "ни одного чанка не сверено");
    println!("всего чанков сверено: {checked_chunks}");
}

