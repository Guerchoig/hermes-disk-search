//! `hds index` — порт `hds/cli.py::cmd_index` (обёртка над `pipeline::run_index`).
//!
//! `--rechunk` (`run_rechunk`) в Rust ещё не портирован — ключ принимается, но
//! возвращает понятное «не поддерживается» (вне B7). `--kinds` — предупреждение
//! (у `run_index` из B4 фильтра по видам пока нет).

use hds_core::config::project_root;
use hds_index::pipeline::{run_index, RunIndexArgs};

use crate::support::{build_embedder, build_media_extractor, build_sidecar, open_conn, parse_kinds, parse_roots};

/// Аргументы `hds index` (порт набора флагов `cmd_index`).
#[derive(Debug, Clone)]
pub struct IndexOpts {
    pub roots: Option<String>,
    pub kinds: Option<String>,
    pub full: bool,
    pub rechunk: bool,
    pub limit: Option<usize>,
    pub no_prune: bool,
    pub confirm_delete: bool,
    pub progress_sec: u64,
    pub quiet: bool,
}

/// `cmd_index`: индексация корней/папок; 0 — успех.
pub fn cmd_index(o: IndexOpts) -> i32 {
    if o.rechunk {
        println!(
            "--rechunk (run_rechunk) ещё не перенесён в Rust (отдельная задача вне B7); \
             ключ принят, но не применён."
        );
        return 1;
    }
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    if let Some(k) = &o.kinds {
        if parse_kinds(k).is_some() {
            println!("[warn] --kinds пока не применяется run_index (Rust B4): фильтр проигнорирован.");
        }
    }
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
    let args = RunIndexArgs {
        roots: o.roots.as_deref().map(parse_roots),
        full: o.full,
        limit: o.limit,
        single_paths: None,
        prune: !o.no_prune,
        confirm_delete: o.confirm_delete,
        progress_sec: o.progress_sec,
        quiet: o.quiet,
    };
    let res = run_index(&conn, &cfg, &emb, &build_media_extractor(&cfg, &sidecar), &sidecar, &args);
    sidecar.shutdown();
    match res {
        Ok(_) => 0,
        Err(e) => {
            println!("Ошибка: {}", e.message());
            1
        }
    }
}
