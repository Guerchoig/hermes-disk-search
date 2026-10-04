//! Автотранскрибация + диаризация «входная папка → выходная» (`PLAN_AUTO_TRANSCRIBE`).
//!
//! Демон `hds transcribe-watch` (вариант A, §4): видит **готовый** медиафайл в
//! `inbox_dir`, зовёт владельца GPU (`/internal/transcribe`, `mode: transcript` +
//! диаризация) через [`TranscribeClient::transcribe_to_file`], пишет `.md` с метками
//! `SPEAKER_NN`/`UNASSIGNED` и sidecar в `out_dir`, затем **утилизирует исходник**
//! (§5.5) — только при успехе.
//!
//! Почему отдельный демон, а не ветка `hds watch`: боевой watcher — порт Python
//! (`hds/watcher.py`), паритет которого проверяется golden-файлами; встраивание
//! второго приоритета очереди его бы усложнило (§4, вариант B отклонён).
//!
//! Что решается **чистыми функциями** (тестируется без ФС и GPU):
//! [`should_enqueue`] (§3), [`AutoTranscribeConfig::validate`] (§2), [`Disposal`] (§5.5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hds_core::config::{dig, project_root, Config};
use hds_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

use crate::kinds;
use crate::transcribe::{TranscribeClient, TranscribeConfig, TranscribeOutcome};
use crate::watch::{self, wait_stable, WatchEvent, WatchLock};

/// Транзиентные суффиксы имён по умолчанию (§2): файл с таким хвостом ещё
/// копируется — ставить задание рано.
pub const DEFAULT_IGNORE_SUFFIXES: &[&str] = &[".part", ".tmp", ".crdownload", ".!qb", ".bak"];

/// Что делать с исходником **после успеха** (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposal {
    /// Удалить исходный медиафайл из `inbox_dir` (по умолчанию).
    Delete,
    /// Оставить на месте (отладка/повторная обработка).
    Keep,
    /// Переместить в `source_disposal_dir`.
    Move,
}

impl Disposal {
    /// Разбор `auto_transcribe.source_disposal` (неизвестное ⇒ `Delete`).
    pub fn parse(s: &str) -> Disposal {
        match s.trim().to_ascii_lowercase().as_str() {
            "keep" | "none" => Disposal::Keep,
            "move" => Disposal::Move,
            _ => Disposal::Delete,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Disposal::Delete => "delete",
            Disposal::Keep => "keep",
            Disposal::Move => "move",
        }
    }
}

/// Метаданные файла для решения о постановке (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    /// `mtime` в миллисекундах от эпохи (0 — неизвестно).
    pub mtime_ms: u64,
}

impl FileMeta {
    /// Снять метаданные с ФС (`None` — файла нет/нет доступа).
    pub fn of(path: &Path) -> Option<FileMeta> {
        let md = std::fs::metadata(path).ok()?;
        Some(FileMeta {
            size: md.len(),
            mtime_ms: mtime_ms(&md),
        })
    }
}

/// `mtime` в миллисекундах (0 — не читается).
pub(crate) fn mtime_ms(md: &std::fs::Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Почему файл **не** ставится в очередь (для логов и тестов, §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// Вне `inbox_dir`.
    OutsideInbox,
    /// Не медиа (по `kinds.rs`/`extensions`).
    NotMedia,
    /// Транзиентное имя (`.part`, `.tmp`…) — файл ещё копируется.
    TransientName,
    /// Уже есть актуальный выход и `overwrite_existing: false`.
    OutputFresh,
    /// Уже обработан успешно (манифест).
    AlreadyDone,
    /// Исчерпаны попытки — «забракован» (манифест), `reconcile` не зацикливаем.
    GaveUp,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::OutsideInbox => "вне inbox_dir",
            SkipReason::NotMedia => "не медиа",
            SkipReason::TransientName => "транзиентное имя (файл ещё копируется)",
            SkipReason::OutputFresh => "выход уже есть и не старше входа",
            SkipReason::AlreadyDone => "уже обработан (манифест)",
            SkipReason::GaveUp => "исчерпаны попытки (манифест)",
        }
    }
}

/// Решение по файлу (§3 пп.1–5) — чистая функция [`should_enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Enqueue,
    Skip(SkipReason),
}

impl Decision {
    pub fn is_enqueue(&self) -> bool {
        matches!(self, Decision::Enqueue)
    }
}
/// Запись манифеста (`state_dir/manifest.jsonl`): что уже сделано/провалено (§3 п.5).
///
/// Ключ дедупликации — абсолютный путь исходника; `(size, mtime_ms)` отличают
/// «тот же файл» от «файл заменили новым содержимым».
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestEntry {
    /// Абсолютный путь исходника (он же ключ).
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
    /// `done` | `failed` | `moved` | `kept`.
    pub status: String,
    /// Чем закончилась утилизация: `delete` | `keep` | `move` | `""` (§5.5).
    #[serde(default)]
    pub disposed: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub added_at: f64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Манифест в памяти (загружается из JSONL при старте демона).
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    by_path: BTreeMap<String, ManifestEntry>,
}

impl Manifest {
    /// Загрузить `state_dir/manifest.jsonl` (битые строки пропускаются).
    pub fn load(state_dir: &Path) -> Manifest {
        let mut m = Manifest::default();
        let path = state_dir.join("manifest.jsonl");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return m;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(e) = serde_json::from_str::<ManifestEntry>(line) {
                m.by_path.insert(e.path.clone(), e);
            }
        }
        m
    }

    pub fn get(&self, path: &str) -> Option<&ManifestEntry> {
        self.by_path.get(path)
    }

    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// Записать запись в память и дозаписать в JSONL (append-only, §4).
    pub fn record(&mut self, state_dir: &Path, entry: ManifestEntry) -> Result<()> {
        std::fs::create_dir_all(state_dir)?;
        let line = serde_json::to_string(&entry)
            .map_err(|e| CoreError::Other(format!("manifest: {e}")))?;
        append_line(&state_dir.join("manifest.jsonl"), &line)?;
        self.by_path.insert(entry.path.clone(), entry);
        Ok(())
    }

    /// Число попыток для пути (`max_attempts`, §5.5).
    pub fn attempts(&self, path: &str) -> u32 {
        self.by_path.get(path).map(|e| e.attempts).unwrap_or(0)
    }

    /// Запись манифеста, **если** файл не изменился с прошлого раза (`size`+`mtime`);
    /// иначе `None` — содержимое заменили, надо обрабатывать заново (§3 п.5).
    pub fn unchanged(&self, path: &str, meta: FileMeta) -> Option<&ManifestEntry> {
        let e = self.by_path.get(path)?;
        if e.size == meta.size && e.mtime_ms == meta.mtime_ms {
            Some(e)
        } else {
            None
        }
    }
}

/// Дозапись строки в файл (append, как JSONL-журнал в Python).
fn append_line(path: &Path, line: &str) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Задание очереди (`queue_file`) — append-only JSONL (§4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QJob {
    pub id: String,
    pub path: String,
    pub size: u64,
    pub mtime_ms: u64,
    /// `pending` | `running` | `done` | `retry` | `failed`.
    pub status: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub added_at: f64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Разбор секции `auto_transcribe.*` (§2) с дефолтами и валидацией.
#[derive(Debug, Clone)]
pub struct AutoTranscribeConfig {
    pub enabled: bool,
    /// ВХОД: исходные медиа (абсолютный путь).
    pub inbox_dir: Option<PathBuf>,
    /// ВЫХОД: транскрибированные/диаризованные (индексируется).
    pub out_dir: Option<PathBuf>,
    pub recursive: bool,
    pub debounce_seconds: u64,
    pub max_stable_wait: u64,
    pub poll_seconds: u64,
    pub require_exclusive_read: bool,
    /// `[]` — все медиа (`kinds.rs`); иначе только перечисленные расширения.
    pub extensions: Vec<String>,
    pub ignore_suffixes: Vec<String>,
    pub queue_file: PathBuf,
    pub state_dir: PathBuf,
    pub overwrite_existing: bool,
    pub max_attempts: u32,
    pub source_disposal: Disposal,
    pub source_disposal_dir: Option<PathBuf>,
    /// Индексировать `out_dir` (§7).
    pub index_outputs: bool,
    /// Ставить `index.pause` на время задания (выше индексации по ТЗ).
    pub pause_indexing: bool,
    /// Как звать владельца GPU (`auto_transcribe.*` → `TranscribeConfig`).
    pub transcribe: TranscribeConfig,
}

impl Default for AutoTranscribeConfig {
    fn default() -> Self {
        AutoTranscribeConfig {
            enabled: false,
            inbox_dir: None,
            out_dir: None,
            recursive: false,
            debounce_seconds: 8,
            max_stable_wait: 120,
            poll_seconds: 3,
            require_exclusive_read: true,
            extensions: Vec::new(),
            ignore_suffixes: DEFAULT_IGNORE_SUFFIXES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            queue_file: PathBuf::from("data/auto-transcribe-queue.jsonl"),
            state_dir: PathBuf::from("data/auto-transcribe"),
            overwrite_existing: false,
            max_attempts: 3,
            source_disposal: Disposal::Delete,
            source_disposal_dir: None,
            index_outputs: true,
            pause_indexing: true,
            transcribe: TranscribeConfig::default(),
        }
    }
}

impl AutoTranscribeConfig {
    /// Собрать из `config.yaml` (относительные пути — от корня проекта).
    pub fn from_config(cfg: &Config) -> Self {
        let root = project_root();
        let abs = |p: PathBuf| if p.is_absolute() { p } else { root.join(p) };
        let d = AutoTranscribeConfig::default();
        AutoTranscribeConfig {
            enabled: bool_or(cfg, "auto_transcribe.enabled", d.enabled),
            inbox_dir: dig_str(cfg, "auto_transcribe.inbox_dir").map(|s| abs(PathBuf::from(s))),
            out_dir: dig_str(cfg, "auto_transcribe.out_dir").map(|s| abs(PathBuf::from(s))),
            recursive: bool_or(cfg, "auto_transcribe.recursive", d.recursive),
            debounce_seconds: u64_or(cfg, "auto_transcribe.debounce_seconds", d.debounce_seconds),
            max_stable_wait: u64_or(cfg, "auto_transcribe.max_stable_wait", d.max_stable_wait),
            poll_seconds: u64_or(cfg, "auto_transcribe.poll_seconds", d.poll_seconds).max(1),
            require_exclusive_read: bool_or(
                cfg,
                "auto_transcribe.require_exclusive_read",
                d.require_exclusive_read,
            ),
            extensions: dig_list(cfg, "auto_transcribe.extensions"),
            ignore_suffixes: {
                let v = dig_list(cfg, "auto_transcribe.ignore_suffixes");
                if v.is_empty() {
                    d.ignore_suffixes
                } else {
                    v
                }
            },
            queue_file: dig_str(cfg, "auto_transcribe.queue_file")
                .map(|s| abs(PathBuf::from(s)))
                .unwrap_or(d.queue_file),
            state_dir: dig_str(cfg, "auto_transcribe.state_dir")
                .map(|s| abs(PathBuf::from(s)))
                .unwrap_or(d.state_dir),
            overwrite_existing: bool_or(
                cfg,
                "auto_transcribe.overwrite_existing",
                d.overwrite_existing,
            ),
            max_attempts: u64_or(cfg, "auto_transcribe.max_attempts", d.max_attempts as u64).max(1)
                as u32,
            source_disposal: dig_str(cfg, "auto_transcribe.source_disposal")
                .map(|s| Disposal::parse(&s))
                .unwrap_or(d.source_disposal),
            source_disposal_dir: dig_str(cfg, "auto_transcribe.source_disposal_dir")
                .map(|s| abs(PathBuf::from(s))),
            index_outputs: bool_or(cfg, "auto_transcribe.index_outputs", d.index_outputs),
            pause_indexing: bool_or(cfg, "auto_transcribe.pause_indexing", d.pause_indexing),
            transcribe: TranscribeConfig::from_auto_transcribe(cfg),
        }
    }

    /// Правила §2: каталоги заданы, различны и **не вложены** друг в друга;
    /// `move` требует каталога, и он не должен лежать внутри `inbox_dir`.
    ///
    /// Возвращает `Err` с понятным текстом — при таком конфиге демон не стартует.
    pub fn validate(&self) -> Result<()> {
        let inbox = self
            .inbox_dir
            .as_ref()
            .ok_or_else(|| CoreError::Other("auto_transcribe.inbox_dir не задан".to_string()))?;
        let out = self
            .out_dir
            .as_ref()
            .ok_or_else(|| CoreError::Other("auto_transcribe.out_dir не задан".to_string()))?;
        if inbox == out {
            return Err(CoreError::Other(
                "auto_transcribe: inbox_dir и out_dir совпадают".to_string(),
            ));
        }
        if is_within(inbox, out) {
            return Err(CoreError::Other(format!(
                "auto_transcribe: out_dir вложен в inbox_dir ({}) — рекурсивная петля",
                out.display()
            )));
        }
        if is_within(out, inbox) {
            return Err(CoreError::Other(format!(
                "auto_transcribe: inbox_dir вложен в out_dir ({}) — рекурсивная петля",
                inbox.display()
            )));
        }
        if self.source_disposal == Disposal::Move {
            let dir = self.source_disposal_dir.as_ref().ok_or_else(|| {
                CoreError::Other(
                    "auto_transcribe.source_disposal: move требует source_disposal_dir".to_string(),
                )
            })?;
            if is_within(inbox, dir) {
                return Err(CoreError::Other(format!(
                    "auto_transcribe: source_disposal_dir внутри inbox_dir ({}) — файл вернётся \
                     в очередь",
                    dir.display()
                )));
            }
        }
        Ok(())
    }

    /// Некритичные замечания — пишутся в лог при старте демона.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if self.transcribe.diarization.enabled && self.transcribe.diarization.model.is_none() {
            w.push(
                "diarization_model не задан: владелец ищет sortformer сам; если модели нет — \
                 задание упадёт (жёсткий отказ, §0 п.6)"
                    .to_string(),
            );
        }
        if self.transcribe.mode != crate::transcribe::AUTO_MODE {
            w.push(format!(
                "mode = '{}' (ожидается '{}' — транскрибация + диаризация)",
                self.transcribe.mode,
                crate::transcribe::AUTO_MODE
            ));
        }
        w
    }

    /// Внутри ли путь `inbox_dir` (§3 п.1): без `recursive` — только прямые дети.
    pub fn in_inbox(&self, path: &Path) -> bool {
        let Some(dir) = &self.inbox_dir else {
            return false;
        };
        if !path.starts_with(dir) {
            return false;
        }
        if self.recursive {
            return true;
        }
        path.parent().map(|p| p == dir).unwrap_or(false)
    }

    /// Медиа ли файл: `kinds.rs` + необязательный белый список `extensions` (§3 п.1).
    pub fn is_media(&self, path: &Path) -> bool {
        if kinds::kind_of_path(path) != Some("media") {
            return false;
        }
        if self.extensions.is_empty() {
            return true;
        }
        let ext = kinds::ext_of(path);
        self.extensions
            .iter()
            .any(|e| e.to_ascii_lowercase().trim_start_matches('.') == ext.trim_start_matches('.'))
    }

    /// Транзиентное имя (`.part`, `.tmp`, `.crdownload`…) — файл ещё копируется (§3 п.2).
    pub fn is_transient(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name.is_empty() {
            return true;
        }
        if kinds::ext_of(path) == ".tmp" {
            return true;
        }
        self.ignore_suffixes
            .iter()
            .any(|s| name.ends_with(&s.to_ascii_lowercase()))
    }

    /// Ожидаемый выход для исходника (`out_dir/<stem>.<out_format>`, §5.3).
    pub fn output_path_for(&self, path: &Path) -> Option<PathBuf> {
        let out_dir = self.out_dir.as_ref()?;
        let stem = path.file_stem()?.to_str()?;
        Some(out_dir.join(format!(
            "{stem}.{}",
            self.transcribe.out_format.trim_start_matches('.')
        )))
    }
}

/// `child` лежит внутри `parent` (лексически, без обращений к ФС).
fn is_within(parent: &Path, child: &Path) -> bool {
    child != parent && child.starts_with(parent)
}

/// Путь из события ФС, если оно значит «файл появился/готов» (§3 п.3).
pub fn event_path(ev: &WatchEvent) -> Option<PathBuf> {
    match ev {
        WatchEvent::Modified(p) => Some(p.clone()),
        WatchEvent::Moved(_, dst) => Some(dst.clone()),
        WatchEvent::Deleted(_) => None,
    }
}

/// Поставить файл в очередь, если [`should_enqueue`] разрешает (§3).
///
/// `Some(job)` — новое задание; `None` — пропуск (по причине) либо уже в очереди.
pub fn try_enqueue(
    cfg: &AutoTranscribeConfig,
    manifest: &Manifest,
    queue: &mut Vec<QJob>,
    path: &Path,
    log_skips: bool,
) -> Option<QJob> {
    let key = path.to_string_lossy().to_string();
    let meta = FileMeta::of(path)?;
    match should_enqueue(cfg, path, meta, manifest) {
        Decision::Skip(r) => {
            if log_skips {
                println!("[transcribe] пропуск: {} — {}", path.display(), r.as_str());
            }
            None
        }
        Decision::Enqueue => {
            if queue.iter().any(|j| j.path == key) {
                return None; // уже в очереди/в работе
            }
            let job = job_for(path, meta, "pending");
            if let Err(e) = append_queue(&cfg.queue_file, &job) {
                eprintln!("[transcribe] очередь не записана: {}", e.message());
            }
            println!(
                "[transcribe] в очередь: {} ({} байт)",
                path.display(),
                meta.size
            );
            queue.push(job.clone());
            Some(job)
        }
    }
}

/// Периодическая/стартовая сверка `inbox_dir` (§3 п.7).
pub fn enqueue_reconcile(cfg: &AutoTranscribeConfig, manifest: &Manifest, queue: &mut Vec<QJob>) {
    for p in reconcile_candidates(cfg, manifest) {
        try_enqueue(cfg, manifest, queue, &p, true);
    }
}

/// Отметить задание завершённым в журнале очереди (§4; `retry` — при ошибке).
fn close_queue(cfg: &AutoTranscribeConfig, job: &QJob, ok: bool) {
    let mut done = job.clone();
    done.status = if ok { "done" } else { "retry" }.to_string();
    done.attempts += 1;
    if let Err(e) = append_queue(&cfg.queue_file, &done) {
        eprintln!("[transcribe] очередь не обновлена: {}", e.message());
    }
}

/// Sortformer-модель диаризации из общего каталога движка: первый `*.gguf` в
/// каталоге `*sortformer*` (там же, где движок держит скачанные модели).
///
/// Нужна и демону (жёсткий отказ без модели, §0 п.6), и диагностике (`hds check`,
/// `hds whisper-check`) — чтобы было видно, готова ли автотранскрибация.
pub fn default_diarization_model() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    let root = base.config_dir().join("OpenResearchTools").join("models");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase().contains("sortformer"))
                    .unwrap_or(false)
        })
        .collect();
    dirs.sort();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|f| f.path())
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("gguf"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        if let Some(p) = files.into_iter().next() {
            return Some(p);
        }
    }
    None
}

/// Модель диаризации для задачи: явный путь из конфига, иначе — автопоиск.
pub fn diarization_model_for(cfg: &Config) -> Option<PathBuf> {
    dig(cfg, "auto_transcribe.diarization_model")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(default_diarization_model)
}

/// Подхватить задания, добавленные в журнал очереди **извне** (§8.3, кнопка
/// «Перезапустить задание» в UI).
///
/// Демон читает `queue_file` один раз на старте, поэтому внешние записи доносим
/// до него периодическим merge'ом (в такт [`AutoTranscribeConfig::poll_seconds`]).
/// Возвращает число добавленных заданий.
pub fn merge_external_jobs(queue_file: &Path, queue: &mut Vec<QJob>) -> usize {
    let mut added = 0usize;
    for job in load_queue(queue_file) {
        if queue.iter().any(|j| j.path == job.path) {
            continue;
        }
        queue.push(job);
        added += 1;
    }
    added
}

/// Один файл «здесь и сейчас» (`hds transcribe-once <файл>`): без очереди и демона.
pub fn transcribe_once(cfg: &AutoTranscribeConfig, path: &Path) -> Result<TranscribeOutcome> {
    cfg.validate()?;
    let _pause = PauseGate::acquire(cfg.pause_indexing);
    TranscribeClient::new(cfg.transcribe.clone()).transcribe_to_file(path)
}

/// Демон `hds transcribe-watch` (вариант A, §4): единственный исполнитель заданий.
///
/// Поток работы: событие ФС → `wait_stable` → [`should_enqueue`] → очередь → движок →
/// выход + sidecar → утилизация → манифест. Периодический reconcile догоняет
/// пропущенное. Остановка — `Ctrl+C` или файл `transcribe.stop`.
pub fn run_transcribe_watch(cfg: &AutoTranscribeConfig) -> Result<i32> {
    if !cfg.enabled {
        println!("[transcribe] auto_transcribe.enabled: false — конвейер выключен");
        return Ok(0);
    }
    cfg.validate()?;
    for w in cfg.warnings() {
        println!("[transcribe] внимание: {w}");
    }
    let inbox = cfg.inbox_dir.clone().expect("validate проверил");
    let out = cfg.out_dir.clone().expect("validate проверил");
    std::fs::create_dir_all(&inbox)?;
    std::fs::create_dir_all(&out)?;
    let root = project_root();
    let Some(_lock) = WatchLock::acquire_named(&root, "transcribe.lock", 3)? else {
        println!("[transcribe] уже запущен (transcribe.lock) — выходим");
        return Ok(0);
    };
    let stop_file = root.join("transcribe.stop");
    let _ = std::fs::remove_file(&stop_file);

    let mut manifest = Manifest::load(&cfg.state_dir);
    let mut queue = load_queue(&cfg.queue_file);
    println!(
        "[transcribe] наблюдаю: {} → {}",
        inbox.display(),
        out.display()
    );
    println!(
        "[transcribe] режим '{}', диаризация: {}",
        cfg.transcribe.mode,
        if cfg.transcribe.diarization.enabled {
            "вкл (sortformer)"
        } else {
            "выкл"
        }
    );
    println!(
        "[transcribe] очередь: {} задание(й), манифест: {} записей",
        queue.len(),
        manifest.len()
    );

    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    watch::spawn_root_watcher(&inbox, tx, Arc::clone(&stop));
    enqueue_reconcile(cfg, &manifest, &mut queue);

    let poll = Duration::from_secs(cfg.poll_seconds.max(1));
    let mut last_poll = std::time::Instant::now();
    println!("[transcribe] готово. Остановка: Ctrl+C или файл transcribe.stop");

    while !stop_file.exists() {
        if let Some(job) = queue.first().cloned() {
            queue.remove(0);
            let path = PathBuf::from(&job.path);
            if FileMeta::of(&path).is_none() {
                continue; // файл исчез, пока стоял в очереди
            }
            if !wait_stable(&path, cfg.debounce_seconds, cfg.max_stable_wait) {
                continue; // файл удалён/так и не стабилизировался
            }
            if cfg.require_exclusive_read && !is_readable(&path) {
                queue.push(job); // писатель ещё держит — вернём задание и подождём
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            let attempts = manifest.attempts(&job.path);
            let ok = process_job(cfg, &mut manifest, &path, attempts)?;
            close_queue(cfg, &job, ok);
            continue;
        }
        if last_poll.elapsed() >= poll {
            last_poll = std::time::Instant::now();
            enqueue_reconcile(cfg, &manifest, &mut queue);
            // T5: задания, добавленные извне (UI «Перезапустить задание»), приходят
            // через журнал очереди — демон читает его только на старте.
            let added = merge_external_jobs(&cfg.queue_file, &mut queue);
            if added > 0 {
                println!("[transcribe] из очереди подхвачено заданий: {added}");
            }
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => {
                if let Some(p) = event_path(&ev) {
                    try_enqueue(cfg, &manifest, &mut queue, &p, false);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    stop.store(true, Ordering::Relaxed);
    println!(
        "[transcribe] остановлен (манифест: {} записей)",
        manifest.len()
    );
    Ok(0)
}

/// Файлы `inbox_dir`, готовые к обработке, в **стабильном порядке** — reconcile (§3).
///
/// Сверка догоняет пропущенное при старте/перезапуске и подчищает «залипшие»
/// задания; порядок сортировки нужен для детерминизма тестов и логов.
pub fn reconcile_candidates(cfg: &AutoTranscribeConfig, manifest: &Manifest) -> Vec<PathBuf> {
    let Some(inbox) = cfg.inbox_dir.as_ref() else {
        return Vec::new();
    };
    let depth = if cfg.recursive { usize::MAX } else { 1 };
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(inbox)
        .max_depth(depth)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let p = entry.path().to_path_buf();
        let Some(meta) = FileMeta::of(&p) else {
            continue;
        };
        if should_enqueue(cfg, &p, meta, manifest).is_enqueue() {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Обработать одно задание: движок → выход + sidecar → утилизация → манифест (§5.3–5.5).
///
/// `Ok(true)` — успех. Ошибка транскрибации демон **не роняет**: попытка растёт,
/// исходник сохраняется и будет повторён до `max_attempts`.
pub fn process_job(
    cfg: &AutoTranscribeConfig,
    manifest: &mut Manifest,
    path: &Path,
    attempts: u32,
) -> Result<bool> {
    let key = path.to_string_lossy().to_string();
    let meta = FileMeta::of(path).unwrap_or(FileMeta {
        size: 0,
        mtime_ms: 0,
    });
    let _pause = PauseGate::acquire(cfg.pause_indexing);
    let client = TranscribeClient::new(cfg.transcribe.clone());
    match client.transcribe_to_file(path) {
        Ok(out) => {
            let (disposed, warn) = dispose_source(cfg, path);
            if let Some(w) = &warn {
                eprintln!("[transcribe] {w}: {}", path.display());
            }
            manifest.record(
                &cfg.state_dir,
                ManifestEntry {
                    path: key,
                    size: meta.size,
                    mtime_ms: meta.mtime_ms,
                    status: "done".to_string(),
                    disposed,
                    attempts: attempts + 1,
                    added_at: now_secs(),
                    error: warn,
                },
            )?;
            println!(
                "[transcribe] {} → {} ({} симв., спикеры: {})",
                path.display(),
                out.out_path.display(),
                out.chars,
                if out.speakers.is_empty() {
                    "—".to_string()
                } else {
                    out.speakers.join(", ")
                }
            );
            Ok(true)
        }
        Err(e) => {
            let attempts = attempts + 1;
            manifest.record(
                &cfg.state_dir,
                ManifestEntry {
                    path: key,
                    size: meta.size,
                    mtime_ms: meta.mtime_ms,
                    status: "failed".to_string(),
                    disposed: String::new(),
                    attempts,
                    added_at: now_secs(),
                    error: Some(e.message()),
                },
            )?;
            eprintln!(
                "[transcribe] ошибка (попытка {attempts}): {} — {}",
                path.display(),
                e.message()
            );
            Ok(false)
        }
    }
}

/// Прочитать очередь (`queue_file`, JSONL): завершённые отбрасываем (§4, рестарт).
pub fn load_queue(path: &Path) -> Vec<QJob> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut jobs: Vec<QJob> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(job) = serde_json::from_str::<QJob>(line) else {
            continue;
        };
        // журнал append-only: последняя запись по пути — истина
        jobs.retain(|j| j.path != job.path);
        if job.status != "done" {
            jobs.push(job);
        }
    }
    jobs
}

/// Дозаписать задание/статус в очередь (append-only JSONL, §4).
pub fn append_queue(path: &Path, job: &QJob) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let line = serde_json::to_string(job).map_err(|e| CoreError::Other(format!("queue: {e}")))?;
    append_line(path, &line)
}

/// Новая запись очереди из решения по файлу.
pub fn job_for(path: &Path, meta: FileMeta, status: &str) -> QJob {
    QJob {
        id: format!(
            "{}-{}",
            meta.mtime_ms,
            path.file_name().unwrap_or_default().to_string_lossy()
        ),
        path: path.to_string_lossy().to_string(),
        size: meta.size,
        mtime_ms: meta.mtime_ms,
        status: status.to_string(),
        attempts: 0,
        added_at: now_secs(),
        error: None,
    }
}

/// Файл открывается на чтение — значит, писатель его отпустил (§3 п.4).
pub fn is_readable(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}

/// Результат утилизации исходника (§5.5): `(disposed, warning)`.
///
/// `disposed` — что реально произошло (`delete`/`keep`/`move`/`""`), `warning` —
/// не-фатальная жалоба (например, файл занят: задание всё равно `done`).
pub fn dispose_source(cfg: &AutoTranscribeConfig, path: &Path) -> (String, Option<String>) {
    match cfg.source_disposal {
        Disposal::Keep => (Disposal::Keep.as_str().to_string(), None),
        Disposal::Delete => match std::fs::remove_file(path) {
            Ok(()) => (Disposal::Delete.as_str().to_string(), None),
            Err(e) => (
                String::new(),
                Some(format!("удалить исходник не удалось ({e}) — файл оставлен")),
            ),
        },
        Disposal::Move => {
            let Some(dir) = cfg.source_disposal_dir.as_ref() else {
                return (
                    String::new(),
                    Some("source_disposal: move, но source_disposal_dir не задан".to_string()),
                );
            };
            if let Err(e) = std::fs::create_dir_all(dir) {
                return (String::new(), Some(format!("каталог архива: {e}")));
            }
            let name = match path.file_name() {
                Some(n) => n,
                None => return (String::new(), Some("имя файла не читается".to_string())),
            };
            let dst = dir.join(name);
            match std::fs::rename(path, &dst) {
                Ok(()) => (Disposal::Move.as_str().to_string(), None),
                Err(e) => (
                    String::new(),
                    Some(format!(
                        "переместить исходник не удалось ({e}) — файл оставлен"
                    )),
                ),
            }
        }
    }
}

/// Шлюз `index.pause` на время задания (приоритет «выше индексации», §7).
///
/// Ставим паузу **только если её не было**, и снимаем **только свою** — то же
/// правило «ставит и снимает один», что у арбитра W2 (`index.pause` пользователя
/// не трогаем). Полная интеграция с диспетчером VRAM — T3.2.
struct PauseGate {
    path: PathBuf,
    ours: bool,
}

impl PauseGate {
    fn acquire(enabled: bool) -> PauseGate {
        let path = project_root().join("index.pause");
        if !enabled || path.exists() {
            return PauseGate { path, ours: false };
        }
        let ours = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .is_ok();
        PauseGate { path, ours }
    }
}

impl Drop for PauseGate {
    fn drop(&mut self) {
        if self.ours {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Решение о постановке файла в очередь (§3) — чистая функция.
///
/// Порядок проверок намеренно такой: сначала дешёвые отсечения (вне инбокса →
/// транзиентное имя → не медиа), затем манифест (идемпотентность) и только потом
/// обращение к ФС за `mtime` выхода.
///
/// Транзиентность проверяется **раньше** вида: недокачанный `клип.mp4.part` не
/// является медиа по расширению, но правильная причина пропуска — «файл ещё
/// копируется», а не «не медиа».
///
/// `meta` — снимок файла на момент проверки (берётся **после** `wait_stable`,
/// иначе получим битый/недописанный файл).
pub fn should_enqueue(
    cfg: &AutoTranscribeConfig,
    path: &Path,
    meta: FileMeta,
    manifest: &Manifest,
) -> Decision {
    if !cfg.in_inbox(path) {
        return Decision::Skip(SkipReason::OutsideInbox);
    }
    if cfg.is_transient(path) {
        return Decision::Skip(SkipReason::TransientName);
    }
    if !cfg.is_media(path) {
        return Decision::Skip(SkipReason::NotMedia);
    }
    let key = path.to_string_lossy().to_string();
    if let Some(e) = manifest.unchanged(&key, meta) {
        match e.status.as_str() {
            "done" | "moved" | "kept" => return Decision::Skip(SkipReason::AlreadyDone),
            "failed" if e.attempts >= cfg.max_attempts => {
                return Decision::Skip(SkipReason::GaveUp);
            }
            _ => {}
        }
    }
    if !cfg.overwrite_existing {
        if let Some(out) = cfg.output_path_for(path) {
            if let Some(om) = FileMeta::of(&out) {
                if om.mtime_ms >= meta.mtime_ms {
                    return Decision::Skip(SkipReason::OutputFresh);
                }
            }
        }
    }
    Decision::Enqueue
}

/// Строка из конфига (`None` — нет ключа/не строка/пусто).
fn dig_str(cfg: &Config, dotted: &str) -> Option<String> {
    dig(cfg, dotted)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Список строк из конфига (не-строки пропускаются).
fn dig_list(cfg: &Config, dotted: &str) -> Vec<String> {
    dig(cfg, dotted)
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn bool_or(cfg: &Config, dotted: &str, fallback: bool) -> bool {
    dig(cfg, dotted)
        .and_then(|v| v.as_bool())
        .unwrap_or(fallback)
}

fn u64_or(cfg: &Config, dotted: &str, fallback: u64) -> u64 {
    dig(cfg, dotted)
        .and_then(|v| v.as_u64())
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::DEFAULT_OUT_FORMAT;

    /// Временный каталог под тест (чистится на входе).
    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hds-auto-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Конфиг с заданными каталогами и дефолтами остального.
    fn cfg_with(inbox: &Path, out: &Path) -> AutoTranscribeConfig {
        AutoTranscribeConfig {
            enabled: true,
            inbox_dir: Some(inbox.to_path_buf()),
            out_dir: Some(out.to_path_buf()),
            state_dir: out.join("_state"),
            queue_file: out.join("_state").join("queue.jsonl"),
            ..Default::default()
        }
    }

    /// §2: каталоги обязательны, различны и не вложены; `move` требует каталога.
    #[test]
    fn validate_rejects_bad_dirs() {
        let root = tmpdir("validate");
        let inbox = root.join("in");
        let out = root.join("out");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(&out).unwrap();

        let ok = cfg_with(&inbox, &out);
        assert!(ok.validate().is_ok(), "нормальная пара каталогов");

        let mut missing = ok.clone();
        missing.inbox_dir = None;
        assert!(missing.validate().is_err(), "нет inbox_dir");

        let mut same = ok.clone();
        same.out_dir = Some(inbox.clone());
        assert!(same.validate().is_err(), "каталоги совпадают");

        let mut nested = ok.clone();
        nested.out_dir = Some(inbox.join("done"));
        assert!(nested.validate().is_err(), "out_dir вложен в inbox_dir");

        let mut nested2 = ok.clone();
        nested2.inbox_dir = Some(out.join("in"));
        assert!(nested2.validate().is_err(), "inbox_dir вложен в out_dir");

        let mut mv = ok.clone();
        mv.source_disposal = Disposal::Move;
        assert!(mv.validate().is_err(), "move без каталога архива");

        mv.source_disposal_dir = Some(root.join("archive"));
        assert!(mv.validate().is_ok(), "move с каталогом архива");

        let mut mv_inside = mv.clone();
        mv_inside.source_disposal_dir = Some(inbox.join("archive"));
        assert!(mv_inside.validate().is_err(), "архив внутри inbox");
        std::fs::remove_dir_all(&root).ok();
    }

    /// §3: отсечения по инбоксу, виду, транзиентному имени и белому списку.
    #[test]
    fn should_enqueue_filters_by_kind_and_name() {
        let root = tmpdir("enq");
        let inbox = root.join("in");
        let out = root.join("out");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let cfg = cfg_with(&inbox, &out);
        let m = Manifest::default();

        let clip = inbox.join("клип.wav");
        std::fs::write(&clip, b"123").unwrap();
        let meta = FileMeta::of(&clip).unwrap();
        assert_eq!(should_enqueue(&cfg, &clip, meta, &m), Decision::Enqueue);

        let outside = root.join("чужой.wav");
        std::fs::write(&outside, b"123").unwrap();
        assert_eq!(
            should_enqueue(&cfg, &outside, FileMeta::of(&outside).unwrap(), &m),
            Decision::Skip(SkipReason::OutsideInbox)
        );

        let txt = inbox.join("заметка.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert_eq!(
            should_enqueue(&cfg, &txt, FileMeta::of(&txt).unwrap(), &m),
            Decision::Skip(SkipReason::NotMedia)
        );

        let part = inbox.join("клип.mp4.part");
        std::fs::write(&part, b"x").unwrap();
        assert_eq!(
            should_enqueue(&cfg, &part, FileMeta::of(&part).unwrap(), &m),
            Decision::Skip(SkipReason::TransientName)
        );

        let sub = inbox.join("под");
        std::fs::create_dir_all(&sub).unwrap();
        let deep = sub.join("клип.mp3");
        std::fs::write(&deep, b"x").unwrap();
        assert_eq!(
            should_enqueue(&cfg, &deep, FileMeta::of(&deep).unwrap(), &m),
            Decision::Skip(SkipReason::OutsideInbox),
            "без recursive вложенные не берём"
        );
        let mut rec = cfg.clone();
        rec.recursive = true;
        assert_eq!(
            should_enqueue(&rec, &deep, FileMeta::of(&deep).unwrap(), &m),
            Decision::Enqueue
        );

        let mut only_mp4 = cfg.clone();
        only_mp4.extensions = vec![".mp4".to_string()];
        assert_eq!(
            should_enqueue(&only_mp4, &clip, meta, &m),
            Decision::Skip(SkipReason::NotMedia),
            "wav не в белом списке"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// §3 п.5: манифест делает обработку идемпотентной и «сдаётся» после лимита попыток.
    #[test]
    fn manifest_dedup_and_give_up() {
        let root = tmpdir("manifest");
        let inbox = root.join("in");
        let out = root.join("out");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let cfg = cfg_with(&inbox, &out);
        let clip = inbox.join("клип.wav");
        std::fs::write(&clip, b"123").unwrap();
        let meta = FileMeta::of(&clip).unwrap();
        let key = clip.to_string_lossy().to_string();

        let mut m = Manifest::default();
        assert_eq!(
            should_enqueue(&cfg, &clip, meta, &m),
            Decision::Enqueue,
            "до манифеста — в очередь"
        );

        let entry = |status: &str, attempts: u32, size: u64, mtime_ms: u64| ManifestEntry {
            path: key.clone(),
            size,
            mtime_ms,
            status: status.to_string(),
            disposed: "delete".to_string(),
            attempts,
            added_at: 0.0,
            error: None,
        };
        m.record(&cfg.state_dir, entry("done", 1, meta.size, meta.mtime_ms))
            .unwrap();
        assert_eq!(
            should_enqueue(&cfg, &clip, meta, &m),
            Decision::Skip(SkipReason::AlreadyDone)
        );

        // файл заменили новым содержимым — обрабатываем снова
        std::fs::write(&clip, b"1234567890").unwrap();
        let meta2 = FileMeta::of(&clip).unwrap();
        assert_ne!(meta2.size, meta.size, "содержимое изменилось");
        assert_eq!(should_enqueue(&cfg, &clip, meta2, &m), Decision::Enqueue);

        // исчерпание попыток
        let mut m2 = Manifest::default();
        m2.record(
            &cfg.state_dir,
            entry("failed", cfg.max_attempts, meta2.size, meta2.mtime_ms),
        )
        .unwrap();
        assert_eq!(
            should_enqueue(&cfg, &clip, meta2, &m2),
            Decision::Skip(SkipReason::GaveUp)
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// §3: актуальный выход ⇒ повторно не транскрибируем (кроме `overwrite_existing`).
    #[test]
    fn fresh_output_skips_input() {
        let root = tmpdir("fresh");
        let inbox = root.join("in");
        let out = root.join("out");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let cfg = cfg_with(&inbox, &out);
        let clip = inbox.join("клип.wav");
        std::fs::write(&clip, b"123").unwrap();
        let md = out.join(format!("клип.{DEFAULT_OUT_FORMAT}"));
        std::fs::write(&md, "### SPEAKER_00 [00:00:01 - 00:00:02]\nпривет\n").unwrap();
        let meta = FileMeta::of(&clip).unwrap();
        assert_eq!(
            should_enqueue(&cfg, &clip, meta, &Manifest::default()),
            Decision::Skip(SkipReason::OutputFresh)
        );

        let mut force = cfg.clone();
        force.overwrite_existing = true;
        assert_eq!(
            should_enqueue(&force, &clip, meta, &Manifest::default()),
            Decision::Enqueue
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// §3 п.3: событие ФС → путь для проверки (удаление заданий не порождает).
    #[test]
    fn event_path_mapping() {
        let p = PathBuf::from("D:\\in\\клип.mp4");
        assert_eq!(
            event_path(&WatchEvent::Modified(p.clone())),
            Some(p.clone())
        );
        assert_eq!(
            event_path(&WatchEvent::Moved(
                PathBuf::from("D:\\in\\a.mp4"),
                p.clone()
            )),
            Some(p)
        );
        assert_eq!(event_path(&WatchEvent::Deleted(PathBuf::from("x"))), None);
    }

    /// T5: задание, добавленное в журнал извне (UI «Перезапустить»), подхватывается
    /// периодическим merge'ом; завершённое — не воскрешаем.
    #[test]
    fn external_queue_jobs_are_merged_once() {
        let root = tmpdir("merge");
        let q = root.join("queue.jsonl");
        let p1 = root.join("a.wav");
        let p2 = root.join("b.wav");
        std::fs::write(&p1, b"x").unwrap();
        std::fs::write(&p2, b"y").unwrap();
        let mut queue = vec![job_for(&p1, FileMeta::of(&p1).unwrap(), "pending")];
        append_queue(&q, &job_for(&p2, FileMeta::of(&p2).unwrap(), "pending")).unwrap();
        assert_eq!(merge_external_jobs(&q, &mut queue), 1, "добавилось одно");
        assert_eq!(queue.len(), 2);
        assert_eq!(
            merge_external_jobs(&q, &mut queue),
            0,
            "повторно не дублируем"
        );

        // задание закрылось (`done`) — из журнала его больше не поднимаем
        let mut done = job_for(&p2, FileMeta::of(&p2).unwrap(), "done");
        done.attempts = 1;
        append_queue(&q, &done).unwrap();
        let mut queue2 = vec![job_for(&p1, FileMeta::of(&p1).unwrap(), "pending")];
        assert_eq!(
            merge_external_jobs(&q, &mut queue2),
            0,
            "done не воскрешаем"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
