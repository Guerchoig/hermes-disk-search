//! Конвейер индексации — дословный порт `hds/indexer.py` (`PLAN_W2_LLM_HOST.md` §5/B4).
//!
//! Разделение фаз сохранено: [`extract_file`] (проверки + извлечение + чанки,
//! без эмбеддингов) и [`commit_file`] (эмбеддинги + атомарная запись чанков/FTS/vec
//! на файл). [`process_file`] — обе фазы плюс CLIP-задел ([`clip_store`]).
//!
//! Инкрементальность как в Python: `unchanged` (size + |Δmtime| < 2 + status=indexed),
//! `moved` (по `content_hash`, только если старый путь исчез с диска), `force/full`,
//! `prune` удалённых, `rename_path`.
//!
//! Эмбеддинги — **только через фасад** (`:8011`, [`Embedder`]); извлечение и
//! лемматизация — через [`Extractor`]/[`Lemmatizer`] (Python-воркер, прототип B6).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hds_core::config::{dig, project_root, Config};
use hds_core::error::Result;
use hds_core::{db, EMB_CONTEXT};
use serde_json::{json, Value};

use crate::chunker::{make_chunks, Chunk};
use crate::embed::{vector_blob, Embedder};
use crate::hash::content_hash;
use crate::heartbeat::{self, HeartbeatFile};
use crate::kinds::{self, Kind};
use crate::progress::{phase_for, ProgressReporter};
use crate::sidecar::{Extractor, Lemmatizer};
use crate::walk::{walk_files, Excludes, IndexLimits, WalkEvent, WalkOptions};

/// Консервативная оценка «символов на токен» для bge-m3 (`_CHARS_PER_TOKEN`).
pub const CHARS_PER_TOKEN: f64 = 2.4;

/// Колбэк прогресса внутри файла (0–100) — для медиа (`rep.set_progress`).
pub type ProgressCb<'a> = &'a dyn Fn(f64);

/// Порт `clip_for_embedding`: не отдавать модели текст длиннее контекста.
///
/// Режем по последнему `\n` внутри лимита, с предупреждением (формат как в Python).
pub fn clip_for_embedding(text: &str, emb_context: i64) -> String {
    let limit = ((emb_context - 256) as f64 * CHARS_PER_TOKEN) as usize;
    let n_chars = text.chars().count();
    if n_chars <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    let cut = match cut.rfind('\n') {
        Some(nl) if nl > limit / 2 => cut[..nl].to_string(),
        _ => cut,
    };
    println!(
        "[warn] чанк {} симв. длиннее бюджета эмбеддинга (~{} ток., {} симв.), обрезан до {}",
        n_chars,
        emb_context,
        limit,
        cut.chars().count()
    );
    cut
}

/// Настройки отбора файлов из конфига (`index.exclude_dirs/paths`, лимиты размеров).
pub fn index_filter(cfg: &Config) -> (Excludes, IndexLimits) {
    let dirs = string_list(cfg, "index.exclude_dirs");
    let paths = string_list(cfg, "index.exclude_paths");
    let limits = IndexLimits::new(
        dig(cfg, "index.max_file_mb").and_then(|v| v.as_u64()).unwrap_or(200),
        dig(cfg, "index.max_media_mb").and_then(|v| v.as_u64()).unwrap_or(2500),
    );
    (Excludes::new(&dirs, &paths), limits)
}

/// Список строк из конфига (`index.roots` и т.п.).
pub fn string_list(cfg: &Config, dotted: &str) -> Vec<String> {
    dig(cfg, dotted)
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Размер чанка (`chunk.size`, по умолчанию 800).
pub fn chunk_size(cfg: &Config) -> usize {
    dig(cfg, "chunk.size")
        .and_then(|v| v.as_u64())
        .unwrap_or(800) as usize
}

/// Перекрытие (`chunk.overlap`, по умолчанию 120).
pub fn chunk_overlap(cfg: &Config) -> i64 {
    dig(cfg, "chunk.overlap").and_then(|v| v.as_i64()).unwrap_or(120)
}

/// Лимит чанков на файл (`index.max_chunks`, 0 = без лимита).
pub fn max_chunks(cfg: &Config) -> usize {
    dig(cfg, "index.max_chunks").and_then(|v| v.as_u64()).unwrap_or(3000) as usize
}

/// Результат фазы извлечения.
pub enum ExtractOutcome {
    /// Ранний выход: статус-строка и вид (если известен) — `(status, kind)`.
    Early { status: String, kind: Option<String> },
    /// Файл готов к коммиту: id, чанки и вид.
    Ready {
        fid: i64,
        chunks: Vec<Chunk>,
        kind: String,
    },
}

/// Вид файла по пути (как `_kind_of`).
pub fn kind_of_path(path: &Path) -> Option<Kind> {
    kinds::kind_of_path(path)
}

/// Путь в виде строки для БД (как Python `os.path` — строка).
pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `time.time()`.
pub fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `st_mtime` в секундах эпохи.
pub fn mtime_secs(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Обрезка строки по символам (как Python `str[:n]`).
pub fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `embedding.batch_size` (в `_commit_file` Python дефолт 32, не 64!).
pub fn embed_batch(cfg: &Config) -> usize {
    dig(cfg, "embedding.batch_size")
        .and_then(|v| v.as_i64())
        .unwrap_or(32)
        .max(1) as usize
}

/// Порт `_extract_file`: проверки + извлечение + чанки (без эмбеддингов).
pub fn extract_file(
    conn: &rusqlite::Connection,
    cfg: &Config,
    path: &Path,
    force: bool,
    extractor: &dyn Extractor,
) -> Result<ExtractOutcome> {
    let ext = kinds::ext_of(path);
    let kind = match kinds::kind_of(&ext) {
        Some(k) => k,
        None => {
            return Ok(ExtractOutcome::Early {
                status: "skipped_type".into(),
                kind: None,
            })
        }
    };
    let p = path_str(path);
    let (excludes, limits) = index_filter(cfg);
    if excludes.excludes_path(path) {
        return Ok(ExtractOutcome::Early {
            status: "skipped_excluded".into(),
            kind: Some(kind.into()),
        });
    }
    if kinds::is_office_lock_file(path) {
        return Ok(ExtractOutcome::Early {
            status: "skipped_type".into(),
            kind: Some(kind.into()),
        });
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return Ok(ExtractOutcome::Early {
                status: format!("stat_error: {e}"),
                kind: Some(kind.into()),
            })
        }
    };
    let size = meta.len();
    let mtime = mtime_secs(&meta);
    if !limits.allows(kind, size) {
        return Ok(ExtractOutcome::Early {
            status: "skipped_big".into(),
            kind: Some(kind.into()),
        });
    }

    let row = db::get_file_by_path(conn, &p)?;
    if !force {
        if let Some(r) = &row {
            if r.is_indexed()
                && r.size == Some(size as i64)
                && (r.mtime_or_zero() - mtime).abs() < 2.0
            {
                return Ok(ExtractOutcome::Early {
                    status: "unchanged".into(),
                    kind: Some(kind.into()),
                });
            }
        }
    }

    // переименование/переезд: контент уже в индексе под другим путём, а старый исчез
    // (в Python `content_hash` отдаёт None при OSError — здесь то же через `.ok()`)
    let chash: Option<String> = content_hash(path, size).ok();
    let need_hash_check = !force && row.as_ref().map(|r| !r.is_indexed()).unwrap_or(true);
    if need_hash_check {
        if let Some(same) = db::get_file_by_hash(conn, chash.as_deref())? {
            if same.path != p
                && same.chunk_count.unwrap_or(0) > 0
                && !Path::new(&same.path).exists()
            {
                db::rename_path(conn, &same.path, &p)?;
                return Ok(ExtractOutcome::Early {
                    status: "moved".into(),
                    kind: Some(kind.into()),
                });
            }
        }
    }

    let fid = db::upsert_file(
        conn,
        &p,
        &ext,
        kind,
        size as i64,
        mtime,
        chash.as_deref(),
    )?;
    let (kind2, segments) = match extractor.extract(path) {
        Ok(v) => v,
        Err(e) => {
            let msg = e.message();
            db::finish_file(conn, fid, "error", Some(&trunc(&msg, 500)), None, 0.0)?;
            return Ok(ExtractOutcome::Early {
                status: format!("error: {}", trunc(&msg, 200)),
                kind: Some(kind.into()),
            });
        }
    };
    let chunks: Vec<Chunk> = if segments.is_empty() {
        Vec::new()
    } else {
        make_chunks(&segments, chunk_size(cfg), chunk_overlap(cfg))
    };
    let mc = max_chunks(cfg);
    let chunks = if mc > 0 && chunks.len() > mc {
        println!(
            "[warn] {}: текст обрезан до {} чанков (index.max_chunks); \
             увеличьте лимит в config.yaml при необходимости",
            p, mc
        );
        chunks[..mc].to_vec()
    } else {
        chunks
    };
    let kind_out = if kind2.is_empty() { kind.to_string() } else { kind2 };
    Ok(ExtractOutcome::Ready {
        fid,
        chunks,
        kind: kind_out,
    })
}

/// Порт `_commit_file`: эмбеддинги чанков + запись в БД. Возвращает статус-строку.
pub fn commit_file(
    conn: &rusqlite::Connection,
    cfg: &Config,
    emb: &Embedder,
    fid: i64,
    chunks: &[Chunk],
    lemmatizer: &dyn Lemmatizer,
    progress_cb: Option<ProgressCb>,
) -> Result<String> {
    let bs = embed_batch(cfg);
    let total = chunks.len();
    if let Some(cb) = progress_cb {
        if total > 0 {
            cb(0.0);
        }
    }
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(total);
    for (i, batch) in chunks.chunks(bs).enumerate() {
        let texts: Vec<String> = batch
            .iter()
            .map(|c| clip_for_embedding(&c.text, EMB_CONTEXT))
            .collect();
        match emb.embed(&texts) {
            Ok(v) => vectors.extend(v),
            Err(e) => {
                let msg = e.message();
                db::finish_file(conn, fid, "error", Some(&trunc(&msg, 500)), None, 0.0)?;
                return Ok(format!("error: {}", trunc(&msg, 200)));
            }
        }
        if let Some(cb) = progress_cb {
            if total > 0 {
                let done = ((i + 1) * bs).min(total);
                cb(100.0 * done as f64 / total as f64);
            }
        }
    }

    // FTS-текст (лемматизированный) — батчем, результат тот же, что в Python по одному
    let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let fts = match lemmatizer.normalize_many(&texts) {
        Ok(f) => f,
        Err(e) => {
            let msg = e.message();
            db::finish_file(conn, fid, "error", Some(&trunc(&msg, 500)), None, 0.0)?;
            return Ok(format!("error: {}", trunc(&msg, 200)));
        }
    };

    db::delete_file_data(conn, fid)?;
    for (i, c) in chunks.iter().enumerate() {
        let fts_i = fts.get(i).cloned().unwrap_or_default();
        let cid = db::add_chunk(conn, fid, i as i64, c.page, c.t_start, c.t_end, &c.text, &fts_i)?;
        if let Some(v) = vectors.get(i) {
            db::add_vector(conn, cid, &vector_blob(v))?;
        }
    }
    db::finish_file(
        conn,
        fid,
        "indexed",
        None,
        Some(total as i64),
        now_epoch(),
    )?;
    Ok(format!("indexed({} чанков)", total))
}

/// Порт `process_file`: обе фазы + CLIP-задел. Возвращает `(status, kind)`.
pub fn process_file(
    conn: &rusqlite::Connection,
    cfg: &Config,
    emb: &Embedder,
    path: &Path,
    force: bool,
    extractor: &dyn Extractor,
    lemmatizer: &dyn Lemmatizer,
    progress_cb: Option<ProgressCb>,
) -> Result<(String, Option<String>)> {
    let (fid, chunks, kind) = match extract_file(conn, cfg, path, force, extractor)? {
        ExtractOutcome::Early { status, kind } => return Ok((status, kind)),
        ExtractOutcome::Ready { fid, chunks, kind } => (fid, chunks, kind),
    };
    let status = commit_file(conn, cfg, emb, fid, &chunks, lemmatizer, progress_cb)?;
    clip_store(conn, cfg, fid, path, &kind, &status);
    Ok((status, Some(kind)))
}

/// Порт `_clip_store` — **задел** B4: CLIP переезжает в Rust на ONNX в W3 (§7).
///
/// Пока ничего не делает (как если бы `index.clip: false`): CLIP-векторы в B4
/// не создаются, паритет по ним не проверяется. Сохранена сигнатура и условие,
/// чтобы в W3 подключить ONNX-энкодер без переделки конвейера.
pub fn clip_store(
    _conn: &rusqlite::Connection,
    _cfg: &Config,
    _fid: i64,
    _path: &Path,
    _kind: &str,
    _status: &str,
) {
    // W3: kind == "image" && status.startswith("indexed") && index.clip
    //     → clip_index.store_for_file(...) на ONNX Runtime (ort).
}

/// `chunk_count` из статус-строки (`indexed(N чанков)` → `N`), иначе 0.
pub fn chunks_in_status(status: &str) -> u64 {
    if let Some(rest) = status.split('(').nth(1) {
        return rest
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
    }
    0
}

/// Ключ счётчика: часть статуса до `(` (как `status.split("(")[0]`).
pub fn status_key(status: &str) -> String {
    status.split('(').next().unwrap_or(status).to_string()
}

/// Параметры [`run_index`] (порт аргументов `cmd_index`).
#[derive(Debug, Clone)]
pub struct RunIndexArgs {
    pub roots: Option<Vec<PathBuf>>,
    pub full: bool,
    pub limit: Option<usize>,
    pub single_paths: Option<Vec<PathBuf>>,
    pub prune: bool,
    pub confirm_delete: bool,
    pub progress_sec: u64,
    pub quiet: bool,
}

impl Default for RunIndexArgs {
    fn default() -> Self {
        RunIndexArgs {
            roots: None,
            full: false,
            limit: None,
            single_paths: None,
            prune: true,
            confirm_delete: false,
            progress_sec: 3,
            quiet: false,
        }
    }
}

/// Корни обхода: явные `roots` или `index.roots` из конфига.
fn effective_roots(cfg: &Config, roots: Option<&[PathBuf]>) -> Vec<PathBuf> {
    match roots {
        Some(r) => r.to_vec(),
        None => string_list(cfg, "index.roots").iter().map(PathBuf::from).collect(),
    }
}

/// Обход корней в список путей (с сообщениями `[scan]`/`[skip]` как в Python).
pub fn collect_files(cfg: &Config, roots: Option<&[PathBuf]>) -> Vec<PathBuf> {
    let opts = WalkOptions::from_parts(
        &effective_roots(cfg, roots)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        &string_list(cfg, "index.exclude_dirs"),
        &string_list(cfg, "index.exclude_paths"),
    );
    let mut files = Vec::new();
    walk_files(
        &opts,
        |f| files.push(f.to_path_buf()),
        |ev| match ev {
            WalkEvent::Scan(p) => println!("[scan] {}", p.display()),
            WalkEvent::SkipRootExcluded(p) => {
                println!("[skip] корень исключён настройкой exclude_paths: {}", p.display())
            }
            WalkEvent::SkipRootMissing(p) => println!("[skip] корень не найден: {}", p.display()),
        },
    );
    files
}

/// Пре-подсчёт числа файлов (для ETA) — как `_precount` в Python.
pub fn count_files(cfg: &Config, roots: Option<&[PathBuf]>) -> usize {
    let opts = WalkOptions::from_parts(
        &effective_roots(cfg, roots)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        &string_list(cfg, "index.exclude_dirs"),
        &string_list(cfg, "index.exclude_paths"),
    );
    let mut n = 0usize;
    walk_files(&opts, |_| n += 1, |_| {});
    n
}

/// Порт `_prune_deleted`: удалить записи исчезнувших файлов (защита >20 %).
pub fn prune_deleted(
    conn: &rusqlite::Connection,
    _cfg: &Config,
    confirm_delete: bool,
) -> Result<usize> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    if total == 0 {
        return Ok(0);
    }
    let mut missing: Vec<(i64, String)> = Vec::new();
    for (fid, path) in db::all_files(conn)? {
        if !Path::new(&path).exists() {
            missing.push((fid, path));
        }
    }
    if missing.is_empty() {
        return Ok(0);
    }
    if missing.len() as f64 > 0.2 * total as f64 && !confirm_delete {
        println!(
            "[prune] Пропало {} из {} файлов (>{}%). Похоже, диск был отключён. \
             Удаление заблокировано; перезапустите с --confirm-delete, если это ожидаемо.",
            missing.len(),
            total,
            20
        );
        return Ok(0);
    }
    for (fid, _path) in &missing {
        db::delete_file_data(conn, *fid)?;
        conn.execute("DELETE FROM files WHERE id=?1", [*fid])?;
    }
    println!("[prune] Удалено из индекса: {}", missing.len());
    Ok(missing.len())
}

/// Порт `reindex_path`: файл или дерево (только `exclude_dirs`, как в Python).
pub fn reindex_path(
    conn: &rusqlite::Connection,
    cfg: &Config,
    emb: &Embedder,
    extractor: &dyn Extractor,
    lemmatizer: &dyn Lemmatizer,
    path: &Path,
    force: bool,
) -> Result<Vec<(String, Option<String>)>> {
    let p = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if p.is_file() {
        return Ok(vec![process_file(
            conn, cfg, emb, &p, force, extractor, lemmatizer, None,
        )?]);
    }
    let opts = WalkOptions::new(
        vec![p],
        Excludes::from_dirs(&string_list(cfg, "index.exclude_dirs")),
    );
    let mut files: Vec<PathBuf> = Vec::new();
    walk_files(&opts, |f| files.push(f.to_path_buf()), |_| {});
    let mut res = Vec::new();
    for f in files {
        res.push(process_file(
            conn, cfg, emb, &f, force, extractor, lemmatizer, None,
        )?);
    }
    Ok(res)
}

/// Будет ли медиафайл реально обрабатываться (для «долго»-сообщения).
fn media_will_process(conn: &rusqlite::Connection, path: &Path, full: bool) -> Result<bool> {
    let row0 = db::get_file_by_path(conn, &path_str(path))?;
    let skip = match std::fs::metadata(path) {
        Ok(m) => {
            !full
                && row0
                    .as_ref()
                    .map(|r| {
                        r.is_indexed()
                            && r.size == Some(m.len() as i64)
                            && (r.mtime_or_zero() - mtime_secs(&m)).abs() < 2.0
                    })
                    .unwrap_or(false)
        }
        Err(_) => true,
    };
    Ok(!skip)
}

/// Порт `run_index`: проход по корням, прогресс, heartbeat, пауза/стоп, prune.
///
/// Возвращает `counters` (как Python); при живом прогоне в другом процессе —
/// `{"skipped_other_process": true}` (R30: паузная/зависшая сессия не блокирует).
pub fn run_index(
    conn: &rusqlite::Connection,
    cfg: &Config,
    emb: &Embedder,
    extractor: &dyn Extractor,
    lemmatizer: &dyn Lemmatizer,
    args: &RunIndexArgs,
) -> Result<Value> {
    let root = project_root();
    let stop_file = root.join("index.stop");
    let pause_file = root.join("index.pause");
    let hb = HeartbeatFile::new(&root);

    if heartbeat::index_running(&hb, 30.0) {
        println!("[index] в другом процессе уже идёт индексация (свежий index.heartbeat.json) — выход.");
        return Ok(json!({"skipped_other_process": true}));
    }

    let mut counters: BTreeMap<String, Value> = BTreeMap::new();
    let seen_roots = args.single_paths.is_none();

    let rep = ProgressReporter::new(if args.quiet { 0 } else { args.progress_sec });
    rep.start();

    // рефреш heartbeat каждые 5 с (долгий файл не выглядит как «встал»)
    let hb_stop = Arc::new(AtomicBool::new(false));
    let hb_thread = {
        let (hb2, rep2, stop2) = (hb.clone(), rep.clone(), Arc::clone(&hb_stop));
        std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(5));
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                hb2.write(&rep2.heartbeat_data());
            }
        })
    };
    let write_hb = |extra: Option<Value>| {
        let mut d = rep.heartbeat_data();
        if let (Some(Value::Object(e)), Some(obj)) = (extra, d.as_object_mut()) {
            for (k, v) in e {
                obj.insert(k, v);
            }
        }
        hb.write(&d);
    };
    write_hb(None);

    if !args.quiet {
        let mode = if rep.is_tty() {
            "терминал (живая строка)"
        } else {
            "не-терминал (полные строки)"
        };
        println!(
            "[прогресс] активен: обновление каждые {} с, режим: {}; отключить: --progress-sec 0",
            args.progress_sec, mode
        );
    }

    if seen_roots {
        let (cfg2, roots, rep2) = (cfg.clone(), args.roots.clone(), rep.clone());
        std::thread::spawn(move || rep2.set_total(count_files(&cfg2, roots.as_deref()) as u64));
    }

    let paths: Vec<PathBuf> = match &args.single_paths {
        Some(v) => v.clone(),
        None => collect_files(cfg, args.roots.as_deref()),
    };

    let mut n = 0usize;
    for path in &paths {
        rep.set_last_path(&path_str(path));
        if pause_file.exists() {
            let mut was_paused = false;
            while pause_file.exists() {
                if stop_file.exists() {
                    break;
                }
                if !was_paused {
                    rep.set_paused(true);
                    println!(
                        "[пауза] индексация приостановлена (файл index.pause); \
                         снимите паузу через UI или удалите файл"
                    );
                }
                was_paused = true;
                write_hb(Some(json!({"paused": true})));
                std::thread::sleep(Duration::from_secs(1));
            }
            rep.set_paused(false);
            if stop_file.exists() {
                break;
            }
        }
        if stop_file.exists() {
            rep.note();
            println!(
                "[stop] найден index.stop — аккуратная остановка \
                 (все обработанные файлы уже сохранены)"
            );
            counters.insert("stopped".to_string(), json!(true));
            break;
        }

        n += 1;
        rep.seen();
        let kind = kinds::kind_of(&kinds::ext_of(path));
        rep.set_current(&path_str(path), phase_for(kind));
        write_hb(Some(json!({"path": path_str(path), "phase": phase_for(kind)})));

        if kind == Some("media") && media_will_process(conn, path, args.full)? {
            rep.note();
            let sz = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            println!(
                "[..] {} — извлечение аудио + Whisper-транскрипция ({:.0} МБ), \
                 может занять несколько минут...",
                path_str(path),
                sz as f64 / 1048576.0
            );
        }

        let cb = |p: f64| rep.set_progress(p);
        let t1 = std::time::Instant::now();
        let (status, kind2) =
            match process_file(conn, cfg, emb, path, args.full, extractor, lemmatizer, Some(&cb)) {
                Ok(v) => v,
                Err(e) => (
                    format!("error: {}", trunc(&e.message(), 200)),
                    kind.map(|k| k.to_string()),
                ),
            };
        let dur = t1.elapsed().as_secs_f64();
        write_hb(None);

        rep.processed(&status, kind2.as_deref(), dur, chunks_in_status(&status));
        rep.set_last_done(&path_str(path), &status_key(&status), dur);
        let key = status_key(&status);
        let cur = counters.get(&key).and_then(|v| v.as_u64()).unwrap_or(0);
        counters.insert(key, json!(cur + 1));

        rep.note();
        if !status.starts_with("unchanged")
            && (n % 20 == 1 || status.starts_with("indexed") || status.starts_with("error"))
        {
            println!("[{}] {} -> {} ({:.1} с)", n, path_str(path), status, dur);
        }
        if let Some(lim) = args.limit {
            if n >= lim {
                break;
            }
        }
    }

    if args.prune && seen_roots && !counters.contains_key("stopped") {
        rep.note();
        prune_deleted(conn, cfg, args.confirm_delete)?;
    }

    let _ = std::fs::remove_file(&stop_file);
    hb_stop.store(true, Ordering::Relaxed);
    let _ = hb_thread.join();
    hb.remove();

    let mut map = serde_json::Map::new();
    for (k, v) in counters {
        map.insert(k, v);
    }
    let cv = Value::Object(map);
    rep.finish(Some(&cv));
    Ok(cv)
}