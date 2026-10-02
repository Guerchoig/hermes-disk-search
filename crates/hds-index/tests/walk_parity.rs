//! Паритет обхода и предполётных проверок с Python-версией (B1).
//!
//! Два уровня:
//! 1. самодостаточный `synthetic_tree_rules` — строит дерево во временном
//!    каталоге и проверяет исключения/лимиты/`~$` без Python;
//! 2. `parity_with_python_dump` — читает `tools/parity/out/walk_parity.json`
//!    (создаётся `tools/parity/walk_parity.py`) и сверяет наборы путей и решения
//!    `precheck`; файла нет — тест пропускается.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hds_index::walk::{
    normalize_path, walk_files, Excludes, FileFilter, IndexLimits, PreCheck, WalkEvent, WalkOptions,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Нормализованные ключи путей (сравнение как в Python: регистр/разделители).
fn norm_set<I: IntoIterator<Item = PathBuf>>(paths: I) -> BTreeSet<String> {
    paths
        .into_iter()
        .map(|p| normalize_path(&p.to_string_lossy()))
        .collect()
}

/// Пройти корни и вернуть (файлы, сообщения обхода).
fn walk(opts: &WalkOptions) -> (Vec<PathBuf>, Vec<String>) {
    let mut files = Vec::new();
    let mut events = Vec::new();
    walk_files(
        opts,
        |p| files.push(p.to_path_buf()),
        |e| {
            events.push(match e {
                WalkEvent::Scan(p) => format!("scan {}", p.display()),
                WalkEvent::SkipRootExcluded(p) => format!("skip-excluded {}", p.display()),
                WalkEvent::SkipRootMissing(p) => format!("skip-missing {}", p.display()),
            })
        },
    );
    (files, events)
}

fn precheck_name(p: PreCheck) -> &'static str {
    match p {
        PreCheck::Ok => "Ok",
        PreCheck::SkippedType => "SkippedType",
        PreCheck::SkippedExcluded => "SkippedExcluded",
        PreCheck::SkippedBig => "SkippedBig",
    }
}

/// Временное дерево для самодостаточного теста.
struct TempTree {
    root: PathBuf,
}

impl TempTree {
    fn new(name: &str) -> TempTree {
        let root = std::env::temp_dir().join("hds-index-tests").join(format!(
            "{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp tree");
        TempTree { root }
    }

    /// Создать файл заданного размера (без реальной записи байтов).
    fn file(&self, rel: &str, size: u64) -> PathBuf {
        let p = self.root.join(rel);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).expect("каталог");
        }
        let f = std::fs::File::create(&p).expect("файл");
        f.set_len(size).expect("размер файла");
        p
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Правила обхода/лимитов без Python: имя каталога (в т.ч. другой регистр),
/// префикс с границей компонента, `~$`, лимиты обычных файлов и медиа.
#[test]
fn synthetic_tree_rules() {
    let t = TempTree::new("rules");
    t.file("ok/readme.md", 10);
    t.file("ok/~$таблица.xlsx", 10);
    t.file("ok/unknown.xyz", 10);
    t.file("ok/clip.mp3", 1_500_000);
    t.file("node_modules/lib.js", 10);
    t.file("NODE_MODULES/lib2.js", 10);
    t.file(".git/config", 10);
    t.file("big.txt", 2 * 1024 * 1024);
    t.file("video.mp4", 3 * 1024 * 1024);
    t.file("backup/old.txt", 10);
    t.file("backup2/new.txt", 10);

    let dirs = vec!["node_modules".to_string(), ".git".to_string()];
    let prefixes = vec![t.root.join("backup").display().to_string()];
    let excludes = Excludes::new(&dirs, &prefixes);
    let limits = IndexLimits::new(1, 2);
    let opts = WalkOptions::new(vec![t.root.clone()], excludes.clone());
    let (files, events) = walk(&opts);

    let got = norm_set(files.clone());
    let expect = norm_set(vec![
        t.root.join("ok/readme.md"),
        t.root.join("ok/~$таблица.xlsx"),
        t.root.join("ok/unknown.xyz"),
        t.root.join("ok/clip.mp3"),
        t.root.join("big.txt"),
        t.root.join("video.mp4"),
        t.root.join("backup2/new.txt"),
    ]);
    assert_eq!(got, expect, "набор обойдённых файлов расходится");
    assert_eq!(events.len(), 1, "должно быть ровно одно сообщение [scan]");
    assert!(events[0].starts_with("scan "), "{events:?}");

    // Исключение по префиксу действует и на одиночный путь (watcher, reindex_path)
    assert!(excludes.excludes_path(&t.root.join("backup/old.txt")));
    assert!(!excludes.excludes_path(&t.root.join("backup2/new.txt")));
    assert!(excludes.excludes_path(&t.root.join("sub/node_modules/x.js")));
    assert!(!excludes.excludes_path(&t.root.join("ok/readme.md")));

    let filter = FileFilter::new(excludes, limits);
    let cases = [
        ("ok/readme.md", "Ok"),
        ("ok/~$таблица.xlsx", "SkippedType"),
        ("ok/unknown.xyz", "SkippedType"),
        ("ok/clip.mp3", "Ok"),
        ("big.txt", "SkippedBig"),
        ("video.mp4", "SkippedBig"),
        ("backup/old.txt", "SkippedExcluded"),
    ];
    for (rel, want) in cases {
        let p = t.root.join(rel);
        let size = std::fs::metadata(&p).expect("stat").len();
        assert_eq!(
            precheck_name(filter.precheck(&p, size)),
            want,
            "precheck({rel})"
        );
    }
}

/// Дампы паритета в `out/`: пофайловые (`walk_parity_<scenario>.json`) — основной
/// путь (синтетический коммитится, «боевой» локален); если их нет, берём общий
/// `walk_parity.json`.
fn parity_dumps() -> Vec<PathBuf> {
    let out = repo_root().join("tools/parity/out");
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&out) {
        for e in rd.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.starts_with("walk_parity")
                && name.ends_with(".json")
                && name != "walk_parity.json"
            {
                files.push(p);
            }
        }
    }
    files.sort();
    if files.is_empty() {
        let combined = out.join("walk_parity.json");
        if combined.is_file() {
            files.push(combined);
        }
    }
    files
}

/// Паритет с Python-дампом (`tools/parity/walk_parity.py`): наборы путей и
/// решения `precheck` обязаны совпасть. Нет дампов — тест пропускается.
#[test]
fn parity_with_python_dump() {
    let dumps = parity_dumps();
    if dumps.is_empty() {
        eprintln!(
            "пропуск: нет дампов out/walk_parity*.json (запустите \
             tools/parity/walk_parity.py [--real])"
        );
        return;
    }
    let mut total_files = 0usize;
    for dump in dumps {
        let text = std::fs::read_to_string(&dump)
            .unwrap_or_else(|e| panic!("нет {}: {e}", dump.display()));
        let v: serde_json::Value = serde_json::from_str(&text).expect("walk_parity json");
        let scenarios = v["scenarios"].as_array().expect("scenarios");
        for sc in scenarios {
            let name = sc["name"].as_str().unwrap_or("?");
            // «Боевой» сценарий — только по флагу: обход всего дерева диска
            // занимает минуты и не нужен в каждом прогоне (gate как у `#[ignore]`).
            if name == "real" && std::env::var("HDS_WALK_PARITY_REAL").as_deref() != Ok("1") {
                println!("сценарий 'real' пропущен (задайте HDS_WALK_PARITY_REAL=1)");
                continue;
            }
            let roots: Vec<String> = str_vec(&sc["roots"]);
            // Дамп может ссылаться на локально сгенерированное дерево (в CI его нет) —
            // пропускаем сценарий, если ни один корень не существует.
            if roots.iter().all(|r| !Path::new(r).exists()) {
                println!("сценарий '{name}' пропущен: корни отсутствуют ({roots:?})");
                continue;
            }
            let dirs: Vec<String> = str_vec(&sc["exclude_dirs"]);
            let prefixes: Vec<String> = str_vec(&sc["exclude_paths"]);
            let limits = IndexLimits::new(
                sc["limits"]["max_file_mb"].as_u64().unwrap_or(200),
                sc["limits"]["max_media_mb"].as_u64().unwrap_or(2500),
            );
            let expected: BTreeSet<String> = str_vec(&sc["files"])
                .iter()
                .map(|p| normalize_path(p))
                .collect();

            let opts = WalkOptions::from_parts(&roots, &dirs, &prefixes);
            let (files, _events) = walk(&opts);
            let got = norm_set(files);
            total_files += got.len();

            let missing: Vec<&String> = expected.difference(&got).collect();
            let extra: Vec<&String> = got.difference(&expected).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "сценарий '{name}': не найдено {} (пример: {:?}), лишних {} (пример: {:?})",
                missing.len(),
                missing.first(),
                extra.len(),
                extra.first()
            );
            println!(
                "сценарий '{name}': {}/{} путей совпало",
                got.len(),
                expected.len()
            );

            // Решения предполётных проверок (вид → exclude → ~$ → лимит)
            let filter = FileFilter::new(Excludes::new(&dirs, &prefixes), limits);
            let map = sc["precheck"].as_object().expect("precheck");
            let mut stat_skipped = 0usize;
            for (p, want) in map {
                let want = want.as_str().unwrap_or_default();
                // Python не смог stat-нуть путь > MAX_PATH (260) — Rust умеет
                // (расширенные пути): осознанное расхождение, см. W2_REPORT.md §2.
                if want == "SkippedStat" {
                    stat_skipped += 1;
                    continue;
                }
                let path = Path::new(p);
                let Ok(md) = std::fs::metadata(path) else {
                    continue; // файл исчез после дампа (волатильные логи) — не падаем
                };
                let got = precheck_name(filter.precheck(path, md.len()));
                assert_eq!(got, want, "precheck расходится для {p}");
            }
            if stat_skipped > 0 {
                println!(
                    "сценарий '{name}': {stat_skipped} путей со 'SkippedStat' пропущено \
                 (длинные пути >260: Python не stat-ит, Rust stat-ит)"
                );
            }
        }
    }
    assert!(
        total_files > 0,
        "ни одного файла в дампе — проверять нечего"
    );
    println!("всего файлов сверено: {total_files}");
}

fn str_vec(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}
