//! Обход корней с исключениями и лимитами — порт `hds/indexer.py`:
//! `iter_files` (обход и отсечение поддеревьев), `_norm_path`,
//! `_excluded_prefixes`, `_prefix_excluded`, `path_excluded`, `_limit_mb`,
//! а также предполётные проверки `_extract_file` (вид файла, `~$`, лимит).
//!
//! Что намеренно сохранено 1:1:
//! * приоритет корней: исключённый префиксом корень пропускается с сообщением,
//!   корень-файл отдаётся как есть, отсутствующий корень — сообщение;
//! * отсечение поддеревьев по **имени** каталога (`exclude_dirs`, без регистра)
//!   и по **полному пути** (`exclude_paths`, по границе компонента пути:
//!   `d:/backup2` не совпадает с `d:/backup`);
//! * нормализация путей как в Python (`_norm_path`) — регистр вниз, оба
//!   разделителя к `/`, лексическая нормализация (. / .. / //);
//! * ошибка доступа к подкаталогу не прерывает обход
//!   (`os.walk(onerror=...)` == пропуск поддерева).

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::kinds::{self, Kind};

/// `index.max_file_mb` по умолчанию (обычные файлы).
pub const DEFAULT_MAX_FILE_MB: u64 = 200;
/// `index.max_media_mb` по умолчанию (аудио/видео: длина важнее размера).
pub const DEFAULT_MAX_MEDIA_MB: u64 = 2500;

/// Лимиты размеров файлов (`hds/indexer.py:_limit_mb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexLimits {
    pub max_file_mb: u64,
    pub max_media_mb: u64,
}

impl Default for IndexLimits {
    fn default() -> Self {
        Self {
            max_file_mb: DEFAULT_MAX_FILE_MB,
            max_media_mb: DEFAULT_MAX_MEDIA_MB,
        }
    }
}

impl IndexLimits {
    pub fn new(max_file_mb: u64, max_media_mb: u64) -> Self {
        Self {
            max_file_mb,
            max_media_mb,
        }
    }

    /// Предел в байтах для вида файла: медиа — `max_media_mb`, остальные — `max_file_mb`.
    pub fn limit_bytes(&self, kind: Kind) -> u64 {
        let mb = if kinds::is_media(kind) {
            self.max_media_mb
        } else {
            self.max_file_mb
        };
        mb * 1024 * 1024
    }

    /// True, если файл проходит по лимиту (в Python отсекается `size > limit`).
    pub fn allows(&self, kind: Kind, size: u64) -> bool {
        size <= self.limit_bytes(kind)
    }
}

/// Порт `_norm_path` + `os.path.normpath`: strip → лексическая нормализация →
/// оба разделителя к `/` → нижний регистр. Возвращает ключ для сравнения путей
/// (не путь для файловых операций!).
pub fn normalize_path(p: &str) -> String {
    let swapped = p.trim().replace('\\', "/");
    let (prefix, rest) = split_prefix(&swapped);
    let mut stack: Vec<&str> = Vec::new();
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if matches!(stack.last(), Some(last) if *last != "..") {
                    stack.pop();
                } else {
                    stack.push("..");
                }
            }
            c => stack.push(c),
        }
    }
    let joined = stack.join("/");
    let mut out = String::from(prefix);
    if !joined.is_empty() {
        if !out.ends_with('/') {
            out.push('/');
        }
        out.push_str(&joined);
    }
    if out.is_empty() {
        out.push('.'); // os.path.normpath("") == "." — как в Python
    }
    out.to_lowercase()
}

/// Нормализация пути из `Path` (совпадает с Python-версией для тех же строк).
pub fn normalize_pathbuf(p: &Path) -> String {
    normalize_path(&p.to_string_lossy())
}

fn split_prefix(s: &str) -> (&str, &str) {
    if let Some(rest) = s.strip_prefix("//") {
        return ("//", rest); // UNC: normpath сохраняет ведущие "\\"
    }
    if let Some(rest) = s.strip_prefix('/') {
        return ("/", rest);
    }
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        if b.len() >= 3 && b[2] == b'/' {
            return (&s[..3], &s[3..]);
        }
        return (&s[..2], &s[2..]);
    }
    ("", s)
}

/// `index.exclude_dirs` + `index.exclude_paths` в нормализованном виде.
#[derive(Debug, Clone, Default)]
pub struct Excludes {
    dirs: Vec<String>,
    prefixes: Vec<String>,
}

impl Excludes {
    /// `dir_names` — `index.exclude_dirs` (сравнение по имени каталога, без регистра),
    /// `path_prefixes` — `index.exclude_paths` (по границе компонента пути).
    pub fn new(dir_names: &[String], path_prefixes: &[String]) -> Self {
        let dirs = dir_names.iter().map(|d| d.to_lowercase()).collect();
        let mut prefixes: Vec<String> = Vec::new();
        for p in path_prefixes {
            let n = normalize_path(p);
            if !n.is_empty() && !prefixes.contains(&n) {
                prefixes.push(n);
            }
        }
        Self { dirs, prefixes }
    }

    pub fn from_dirs(dir_names: &[String]) -> Self {
        Self::new(dir_names, &[])
    }

    pub fn dirs(&self) -> &[String] {
        &self.dirs
    }

    pub fn prefixes(&self) -> &[String] {
        &self.prefixes
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.prefixes.is_empty()
    }

    /// Каталог с таким (уже приведённым к нижнему регистру) именем исключён.
    pub fn dir_excluded(&self, name_lower: &str) -> bool {
        self.dirs.iter().any(|d| d == name_lower)
    }

    /// Порт `_prefix_excluded`: путь равен исключённому префиксу или лежит под ним.
    pub fn matches_prefix(&self, path: &Path) -> bool {
        if self.prefixes.is_empty() {
            return false;
        }
        let np = normalize_pathbuf(path);
        self.prefixes
            .iter()
            .any(|pr| np == *pr || np.starts_with(&format!("{pr}/")))
    }

    /// Порт `hds/indexer.py:path_excluded`: исключение по имени любого компонента
    /// пути (`exclude_dirs`) или по префиксу пути (`exclude_paths`). Применяется к
    /// одиночным путям — события watcher'а, `MCP reindex_path` (обход дерева идёт
    /// через [`walk_files`], где отсечение делается на уровне каталогов).
    pub fn excludes_path(&self, path: &Path) -> bool {
        let p = path.to_string_lossy().replace('\\', "/");
        if p.split('/').any(|part| self.dir_excluded(&part.to_lowercase())) {
            return true;
        }
        self.matches_prefix(path)
    }
}

/// Сообщения обхода (соответствуют строкам Python в stdout; печатает вызывающий).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkEvent<'a> {
    /// `[scan] {root}`
    Scan(&'a Path),
    /// `[skip] корень исключён настройкой exclude_paths: {root}`
    SkipRootExcluded(&'a Path),
    /// `[skip] корень не найден: {root}`
    SkipRootMissing(&'a Path),
}

/// Параметры обхода: корни + исключения (лимиты применяет [`FileFilter`]).
#[derive(Debug, Clone, Default)]
pub struct WalkOptions {
    pub roots: Vec<PathBuf>,
    pub excludes: Excludes,
}

impl WalkOptions {
    pub fn new(roots: Vec<PathBuf>, excludes: Excludes) -> Self {
        Self { roots, excludes }
    }

    /// Из «сырого» конфига: `index.roots`, `index.exclude_dirs`, `index.exclude_paths`.
    pub fn from_parts(roots: &[String], exclude_dirs: &[String], exclude_paths: &[String]) -> Self {
        Self {
            roots: roots.iter().map(PathBuf::from).collect(),
            excludes: Excludes::new(exclude_dirs, exclude_paths),
        }
    }
}

/// Обход корней — порт `hds/indexer.py:iter_files`.
///
/// `on_file` вызывается для каждого найденного файла (в Python генератор отдаёт
/// **все** файлы; фильтрация по виду/размеру — в [`FileFilter`]). Возвращает
/// число отданных файлов.
pub fn walk_files<F, L>(opts: &WalkOptions, mut on_file: F, mut on_event: L) -> usize
where
    F: FnMut(&Path),
    L: FnMut(WalkEvent<'_>),
{
    let mut n = 0usize;
    for root in &opts.roots {
        // os.path.abspath(root) — без разрешения симлинков
        let root = std::path::absolute(root).unwrap_or_else(|_| root.clone());
        if opts.excludes.matches_prefix(&root) {
            on_event(WalkEvent::SkipRootExcluded(&root));
            continue;
        }
        if root.is_file() {
            on_file(&root);
            n += 1;
            continue;
        }
        if !root.is_dir() {
            on_event(WalkEvent::SkipRootMissing(&root));
            continue;
        }
        on_event(WalkEvent::Scan(&root));

        let excludes = &opts.excludes;
        let walker = WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(move |e| {
                if e.depth() == 0 {
                    return true; // сам корень не отсекаем (проверен выше)
                }
                let name = e.file_name().to_string_lossy().to_lowercase();
                !excludes.dir_excluded(&name) && !excludes.matches_prefix(e.path())
            });

        for entry in walker {
            // os.walk(onerror=lambda e: None) — недоступное поддерево пропускаем
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let ft = entry.file_type();
            if ft.is_dir() {
                continue;
            }
            // Симлинк на каталог Python кладёт в dirnames (followlinks=False ⇒ не
            // обходится и не отдаётся). Симлинк на файл (и битый) — в filenames.
            if ft.is_symlink() && entry.path().is_dir() {
                continue;
            }
            let p = entry.path();
            if opts.excludes.matches_prefix(p) {
                continue; // исключённые одиночные файлы
            }
            on_file(p);
            n += 1;
        }
    }
    n
}

/// Решение предполётных проверок файла (порт первой половины `_extract_file`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreCheck {
    /// Файл дошёл до извлечения (дальше — `unchanged`/`moved`/индексация).
    Ok,
    /// Вид не поддержан либо это служебный `~$`-файл Office (`skipped_type`).
    SkippedType,
    /// Путь исключён настройками (`skipped_excluded`).
    SkippedExcluded,
    /// Размер больше лимита для вида (`skipped_big`).
    SkippedBig,
}

/// Фильтр файлов: исключения + лимиты размеров по виду.
#[derive(Debug, Clone, Default)]
pub struct FileFilter {
    pub excludes: Excludes,
    pub limits: IndexLimits,
}

impl FileFilter {
    pub fn new(excludes: Excludes, limits: IndexLimits) -> Self {
        Self { excludes, limits }
    }

    /// Вид файла по расширению (`_kind_of`).
    pub fn kind(&self, path: &Path) -> Option<Kind> {
        kinds::kind_of_path(path)
    }

    /// Порт предполётных проверок `_extract_file` — **порядок важен**:
    /// вид → исключённый путь → `~$` → лимит размера.
    pub fn precheck(&self, path: &Path, size: u64) -> PreCheck {
        let kind = match kinds::kind_of_path(path) {
            Some(k) => k,
            None => return PreCheck::SkippedType,
        };
        if self.excludes.excludes_path(path) {
            return PreCheck::SkippedExcluded;
        }
        if kinds::is_office_lock_file(path) {
            return PreCheck::SkippedType;
        }
        if !self.limits.allows(kind, size) {
            return PreCheck::SkippedBig;
        }
        PreCheck::Ok
    }
}



