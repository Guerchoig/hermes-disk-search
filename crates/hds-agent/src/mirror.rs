//! Прямой путь зеркала: папка проекта → `<P>\workspace` (TR-7.1/TR-7.3, A4.2).
//!
//! Обход дерева проекта идемпотентен: решение «пересоздавать образ или нет»
//! принимается **только по sha256** оригинала и образа, а реестр обновляется
//! через `upsert`/`set_issued`/`mark_seen`. Повторный прогон по неизменённой
//! папке даёт `SyncReport::changes() == 0` и не пишет в БД.
//!
//! Что делает проход:
//! * обходит проект рекурсивно, пропуская `.hds-agent`, `_attachments`,
//!   `_conflicts`, `~$*`, `*.tmp`, `Thumbs.db`; junction/симлинк не обходится
//!   и проверяется канонизацией (AG-11);
//! * `wait_stable` перед обработкой: размер и mtime не меняются
//!   `wait_stable_sec` (опрос `wait_poll_ms`, максимум `wait_max_sec`); файл
//!   старше `settle_age_sec` не ждём. Не дождались — отложили до следующего
//!   прохода, **без** статуса «ошибка»;
//! * `docx`/`xlsx`/`txt`/`md`/`xml` → образ того же формата
//!   (`anonymize mode:"agent"`); `pdf`/`pptx`/`mpp` → текстовый образ
//!   `<анонимное имя>.<ext>.md` (текст — `hds_extract`, затем `text_image`);
//! * прочие расширения → `unsupported`, файлы больше `index.max_file_mb` →
//!   `too_big`; в workspace они не попадают;
//! * удалённый оригинал → образ удаляется, если агент его не менял
//!   (`seen == issued`), иначе запись остаётся `orphan`;
//! * переименованный оригинал (тот же sha256, новый путь) → образ
//!   переименовывается, повторной анонимизации нет;
//! * изменённый оригинал при `pending_accept`/`conflict` → `conflict`, образ
//!   агента **не перезаписывается** (TR-7.6, случай 2);
//! * изменение образа агентом (`seen != issued`) фиксируется статусом
//!   `agent_changed`, но прямой путь его не затирает — это A4.3.
//!
//! Имена: `anonymize_path` даёт метки без падежного суффикса (Python),
//! длина компонента ограничена [`MAX_NAME_LEN`], коллизии — ` (2)`, ` (3)`.
//! Связь «оригинал ↔ образ» живёт в реестре, а не вычисляется из имени.
//!
//! Воркеры (анонимизация, извлечение) поднимаются **лениво**: пустая папка
//! или папка без поддерживаемых форматов не требует Python вообще.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use hds_core::config::{dig, Config};
use sha2::{Digest, Sha256};

use crate::registry_db::{Artifact, ArtifactRegistry, Kind, RegistryError, Status};
use crate::settings::{sync_settings, SyncSettings};

/// Предел длины одного компонента имени (TR-7.1, В-5).
///
/// Windows без `LongPathsEnabled` ограничивает путь ~260 символами, а путь
/// образа = `<agent.root>\P0001\workspace\` + вложенность + имя.
pub const MAX_NAME_LEN: usize = 120;

/// Служебные каталоги: не обходятся и не выпускаются (TR-7.1).
pub fn is_service_dir(name: &str) -> bool {
    [".hds-agent", "_attachments", "_conflicts"]
        .iter()
        .any(|service| name.eq_ignore_ascii_case(service))
}

/// Служебные файлы: не обходятся и не выпускаются (TR-7.1).
pub fn is_service_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.starts_with("~$") || lower.ends_with(".tmp") || lower == "thumbs.db"
}

/// Вид артефакта по расширению (TR-7.3): образ или текстовый образ.
pub fn kind_for(rel: &str) -> Option<Kind> {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "docx" | "xlsx" | "txt" | "md" | "xml" => Some(Kind::Image),
        "pdf" | "pptx" | "mpp" => Some(Kind::TextImage),
        _ => None,
    }
}

/// Компоненты относительного пути (оба разделителя, без `.` и пустых).
pub fn split_path(rel: &str) -> Vec<String> {
    rel.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.to_string())
        .collect()
}

/// Обрезать длинное имя до [`MAX_NAME_LEN`], сохранив расширение (TR-7.1).
///
/// К обрезанному имени добавляется 8 hex от sha256 исходного имени:
/// два разных длинных имени не сливаются в одно.
pub fn fit_component(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= MAX_NAME_LEN {
        return name.to_string();
    }
    let hash = sha256_hex(name.as_bytes());
    // Расширение до 10 символов сохраняем целиком — иначе образ потеряет тип.
    let ext_len = match name.rfind('.') {
        Some(idx) => {
            let tail = name[idx..].chars().count();
            if tail <= 10 {
                tail
            } else {
                0
            }
        }
        None => 0,
    };
    let keep = MAX_NAME_LEN - 9 - ext_len;
    let head: String = chars[..keep].iter().collect();
    let ext: String = chars[chars.len() - ext_len..].iter().collect();
    format!("{head}-{}{ext}", &hash[..8])
}

pub(crate) fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(idx) if idx > 0 => (&name[..idx], &name[idx..]),
        _ => (name, ""),
    }
}

/// Свободное имя: `имя (2).ext`, `имя (3).ext` (TR-7.1, В-5).
///
/// `taken` — занятые относительные пути образов; существование файла на
/// диске проверяет вызывающий (образ мог появиться не из реестра).
pub fn unique_name(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    let (stem, ext) = split_name(base);
    for n in 2..10_000u32 {
        let candidate = format!("{stem} ({n}){ext}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    format!("{stem} ({}){ext}", &sha256_hex(base.as_bytes())[..8])
}

/// sha256 байтов.
pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// sha256 файла целиком (потоково, блоками 64 КБ).
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Канонический путь без префикса `\\?\` (грабля §6 п.1: Windows добавляет его).
pub fn canonical_clean(path: &Path) -> PathBuf {
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = canonical.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        canonical
    }
}

/// Файл папки проекта, попавший в обход.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Относительный путь (разделитель платформы), как в реестре.
    pub rel: String,
    /// Абсолютный путь.
    pub abs: PathBuf,
    pub size: u64,
    pub mtime: SystemTime,
}

/// Обойти папку проекта (TR-7.1): рекурсивно, детерминированно, без служебного.
///
/// Junction/симлинк не обходится: каталог канонизируется и обязан остаться
/// внутри корня (AG-11) — иначе ссылка утащила бы в зеркало чужие файлы.
pub fn walk_files(root: &Path) -> Result<Vec<ScannedFile>, MirrorError> {
    let root_canon = canonical_clean(root);
    let mut out = Vec::new();
    walk_dir(root, &root_canon, Path::new(""), &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn walk_dir(
    dir: &Path,
    root_canon: &Path,
    rel: &Path,
    out: &mut Vec<ScannedFile>,
) -> Result<(), MirrorError> {
    let entries =
        fs::read_dir(dir).map_err(|e| MirrorError::Walk(dir.to_path_buf(), e.to_string()))?;
    let mut items: Vec<fs::DirEntry> = entries.filter_map(|entry| entry.ok()).collect();
    items.sort_by_key(|entry| entry.file_name());
    for entry in items {
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        let rel_path = if rel.as_os_str().is_empty() {
            PathBuf::from(&name)
        } else {
            rel.join(&name)
        };
        if file_type.is_dir() {
            if is_service_dir(&name) {
                continue;
            }
            let canon = canonical_clean(&entry.path());
            if !canon.starts_with(root_canon) {
                continue;
            }
            walk_dir(&entry.path(), root_canon, &rel_path, out)?;
        } else if file_type.is_file() {
            if is_service_file(&name) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            out.push(ScannedFile {
                rel: rel_path.to_string_lossy().into_owned(),
                abs: entry.path(),
                size: meta.len(),
                mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(())
}

/// Итог ожидания «файл успокоился» (TR-7.3 п.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stable {
    /// Можно обрабатывать.
    Ready,
    /// Не дождались: отложить до следующего прохода (не ошибка).
    Deferred,
    /// Файл исчез, пока ждали.
    Gone,
}

/// `wait_stable` с явным «сейчас» — чтобы тест не спал реальные секунды.
///
/// Логика: файл старше `settle_age_sec` не ждём вовсе (первый проход по
/// большой папке не тормозит), иначе опрашиваем размер+mtime каждые
/// `wait_poll_ms`, пока они не продержатся `wait_stable_sec`; дольше
/// `wait_max_sec` не ждём.
pub fn wait_stable_at(path: &Path, opts: &SyncSettings, now: SystemTime) -> Stable {
    let Ok(meta) = fs::metadata(path) else {
        return Stable::Gone;
    };
    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if now.duration_since(mtime).unwrap_or_default().as_secs() >= opts.settle_age_sec {
        return Stable::Ready;
    }
    if opts.wait_stable_sec == 0 {
        return Stable::Ready;
    }
    let poll = Duration::from_millis(opts.wait_poll_ms.max(1));
    let deadline = now + Duration::from_secs(opts.wait_max_sec);
    let mut last = (meta.len(), mtime);
    let mut stable = Duration::ZERO;
    loop {
        if SystemTime::now() >= deadline {
            return Stable::Deferred;
        }
        std::thread::sleep(poll);
        let Ok(meta) = fs::metadata(path) else {
            return Stable::Gone;
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if (meta.len(), mtime) == last {
            stable += poll;
            if stable.as_secs() >= opts.wait_stable_sec {
                return Stable::Ready;
            }
        } else {
            last = (meta.len(), mtime);
            stable = Duration::ZERO;
        }
    }
}

/// Анонимизация имён и образов — то, что зеркало ждёт от воркера.
///
/// Трейт, а не конкретный `Client`: тест зеркала не поднимает Python и NER.
pub trait Anonymizer {
    /// `anonymize_path`: относительный путь → путь с метками (без `|`).
    fn anonymize_path(&mut self, project_id: &str, rel: &str) -> Result<String, MirrorError>;
    /// `deanonymize_path`: путь с метками → реальные значения проекта (TR-7.5).
    fn deanonymize_path(
        &mut self,
        project_id: &str,
        rel: &str,
    ) -> Result<hds_anonymize::DeanonymizedPath, MirrorError>;
    /// Образ того же формата: `anonymize mode:"agent"` c явным `out_path`.
    fn anonymize_agent(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: i64,
    ) -> Result<(), MirrorError>;
    /// Текстовый образ `<имя>.<ext>.md` (TR-7.3): `text` — от [`TextExtractor`].
    fn text_image(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: i64,
        text: Option<&str>,
    ) -> Result<(), MirrorError>;
    /// `read_marker`: маркер `hds-agent/<P>/<id>` образа (TR-7.4/7.7).
    fn read_marker(&mut self, path: &Path) -> Result<hds_anonymize::MarkerInfo, MirrorError>;
    /// `image_map` (В-4): карта `[IMAGE_n]` → часть пакета.
    fn image_map(&mut self, path: &Path) -> Result<serde_json::Value, MirrorError>;
    /// Обратный акт (TR-7.5): деанонимизировать образ в файл пользователя.
    ///
    /// `restore_from` — снимок основания: из него возвращаются `docProps` и
    /// картинки тела на места меток `[IMAGE_n]`; `None` — новый файл агента.
    fn deanonymize_agent(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        restore_from: Option<&Path>,
    ) -> Result<hds_anonymize::DeanonymizeResult, MirrorError>;
}

/// Извлечение текста для `pdf`/`pptx`/`mpp` (воркер `hds-extract`).
pub trait TextExtractor {
    fn extract(&mut self, path: &Path) -> Result<String, MirrorError>;
}

impl Anonymizer for hds_anonymize::Client {
    fn anonymize_path(&mut self, project_id: &str, rel: &str) -> Result<String, MirrorError> {
        hds_anonymize::Client::anonymize_path(self, project_id, rel)
            .map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }

    fn anonymize_agent(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: i64,
    ) -> Result<(), MirrorError> {
        let id = artifact_id.to_string();
        hds_anonymize::Client::anonymize_agent_to(self, in_path, out_path, project_id, Some(&id))
            .map(|_| ())
            .map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }

    fn text_image(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: i64,
        text: Option<&str>,
    ) -> Result<(), MirrorError> {
        let id = artifact_id.to_string();
        hds_anonymize::Client::text_image_to(self, in_path, out_path, project_id, Some(&id), text)
            .map(|_| ())
            .map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }

    fn deanonymize_path(
        &mut self,
        project_id: &str,
        rel: &str,
    ) -> Result<hds_anonymize::DeanonymizedPath, MirrorError> {
        hds_anonymize::Client::deanonymize_path(self, project_id, rel)
            .map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }

    fn read_marker(&mut self, path: &Path) -> Result<hds_anonymize::MarkerInfo, MirrorError> {
        hds_anonymize::Client::read_marker(self, path)
            .map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }

    fn image_map(&mut self, path: &Path) -> Result<serde_json::Value, MirrorError> {
        hds_anonymize::Client::image_map(self, path)
            .map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }

    fn deanonymize_agent(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        restore_from: Option<&Path>,
    ) -> Result<hds_anonymize::DeanonymizeResult, MirrorError> {
        hds_anonymize::Client::deanonymize_agent_to(
            self,
            in_path,
            out_path,
            project_id,
            restore_from,
            restore_from.is_some(),
        )
        .map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }
}

impl TextExtractor for hds_extract::Worker {
    fn extract(&mut self, path: &Path) -> Result<String, MirrorError> {
        let result = hds_extract::Worker::extract(self, path)
            .map_err(|e| MirrorError::Extractor(e.to_string()))?;
        Ok(join_segments(&result))
    }
}

/// Сегменты `hds_extract` → текст для `text_image` (страницы — как в D18).
pub fn join_segments(result: &hds_extract::ExtractResult) -> String {
    let mut out = String::new();
    let mut page: Option<i64> = None;
    for segment in &result.segments {
        if segment.page != page {
            page = segment.page;
            if let Some(n) = page {
                out.push_str(&format!("\n\n<!-- страница {n} -->\n\n"));
            }
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&segment.text);
    }
    out
}

/// Ленивый воркер анонимизации: Python поднимается при первом же файле.
pub struct LazyAnonymizer {
    spawn: Box<dyn FnMut() -> Result<hds_anonymize::Client, MirrorError>>,
    client: Option<hds_anonymize::Client>,
}

impl LazyAnonymizer {
    pub fn new<F>(spawn: F) -> LazyAnonymizer
    where
        F: FnMut() -> Result<hds_anonymize::Client, MirrorError> + 'static,
    {
        LazyAnonymizer {
            spawn: Box::new(spawn),
            client: None,
        }
    }

    fn get(&mut self) -> Result<&mut hds_anonymize::Client, MirrorError> {
        if self.client.is_none() {
            self.client = Some((self.spawn)()?);
        }
        Ok(self.client.as_mut().expect("воркер только что поднят"))
    }
}

impl Drop for LazyAnonymizer {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            client.shutdown();
        }
    }
}

impl Anonymizer for LazyAnonymizer {
    fn anonymize_path(&mut self, project_id: &str, rel: &str) -> Result<String, MirrorError> {
        Anonymizer::anonymize_path(self.get()?, project_id, rel)
    }

    fn anonymize_agent(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: i64,
    ) -> Result<(), MirrorError> {
        Anonymizer::anonymize_agent(self.get()?, in_path, out_path, project_id, artifact_id)
    }

    fn text_image(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        artifact_id: i64,
        text: Option<&str>,
    ) -> Result<(), MirrorError> {
        Anonymizer::text_image(
            self.get()?,
            in_path,
            out_path,
            project_id,
            artifact_id,
            text,
        )
    }

    fn deanonymize_path(
        &mut self,
        project_id: &str,
        rel: &str,
    ) -> Result<hds_anonymize::DeanonymizedPath, MirrorError> {
        Anonymizer::deanonymize_path(self.get()?, project_id, rel)
    }

    fn read_marker(&mut self, path: &Path) -> Result<hds_anonymize::MarkerInfo, MirrorError> {
        Anonymizer::read_marker(self.get()?, path)
    }

    fn image_map(&mut self, path: &Path) -> Result<serde_json::Value, MirrorError> {
        Anonymizer::image_map(self.get()?, path)
    }

    fn deanonymize_agent(
        &mut self,
        in_path: &Path,
        out_path: &Path,
        project_id: &str,
        restore_from: Option<&Path>,
    ) -> Result<hds_anonymize::DeanonymizeResult, MirrorError> {
        Anonymizer::deanonymize_agent(self.get()?, in_path, out_path, project_id, restore_from)
    }
}

/// Ленивый воркер извлечения: нужен только `pdf`/`pptx`/`mpp`.
pub struct LazyExtractor {
    spawn: Box<dyn FnMut() -> Result<hds_extract::Worker, MirrorError>>,
    worker: Option<hds_extract::Worker>,
}

impl LazyExtractor {
    pub fn new<F>(spawn: F) -> LazyExtractor
    where
        F: FnMut() -> Result<hds_extract::Worker, MirrorError> + 'static,
    {
        LazyExtractor {
            spawn: Box::new(spawn),
            worker: None,
        }
    }

    fn get(&mut self) -> Result<&mut hds_extract::Worker, MirrorError> {
        if self.worker.is_none() {
            self.worker = Some((self.spawn)()?);
        }
        Ok(self.worker.as_mut().expect("воркер только что поднят"))
    }
}

impl Drop for LazyExtractor {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.as_mut() {
            worker.shutdown();
        }
    }
}

impl TextExtractor for LazyExtractor {
    fn extract(&mut self, path: &Path) -> Result<String, MirrorError> {
        TextExtractor::extract(self.get()?, path)
    }
}

/// Ошибка зеркала. Ошибка одного файла не валит проход: она становится
/// статусом `error` и строкой в [`SyncReport::problems`].
#[derive(Debug)]
pub enum MirrorError {
    Walk(PathBuf, String),
    Registry(RegistryError),
    Anonymizer(String),
    Extractor(String),
    Io(PathBuf, String),
    Config(String),
}

impl std::fmt::Display for MirrorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MirrorError::Walk(path, why) => write!(f, "обход {}: {why}", path.display()),
            MirrorError::Registry(err) => write!(f, "{err}"),
            MirrorError::Anonymizer(why) => write!(f, "анонимизация: {why}"),
            MirrorError::Extractor(why) => write!(f, "извлечение текста: {why}"),
            MirrorError::Io(path, why) => write!(f, "{}: {why}", path.display()),
            MirrorError::Config(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for MirrorError {}

impl From<RegistryError> for MirrorError {
    fn from(err: RegistryError) -> MirrorError {
        MirrorError::Registry(err)
    }
}

/// Счётчики прохода: их печатает CLI и по ним же проверяется идемпотентность.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub scanned: usize,
    pub created: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub unsupported: usize,
    pub too_big: usize,
    pub deferred: usize,
    pub removed: usize,
    pub renamed: usize,
    pub orphans: usize,
    pub agent_changed: usize,
    pub conflicts: usize,
    pub errors: usize,
    /// A4.3: файлов агента выпущено в папку проекта (`agent_new`).
    pub emitted: usize,
    /// A4.3: вариантов изменённых документов собрано в `.hds-agent\pending`.
    pub variants: usize,
    /// «Файл: причина» — без значений PII (I8).
    pub problems: Vec<String>,
}

impl SyncReport {
    /// Сколько всего изменилось. Повторный прогон по неизменной папке обязан
    /// дать 0 (A4.2: «повторный прогон — 0 изменений»).
    pub fn changes(&self) -> usize {
        self.created
            + self.updated
            + self.removed
            + self.renamed
            + self.orphans
            + self.agent_changed
            + self.conflicts
            + self.errors
            + self.emitted
            + self.variants
    }
}

/// Один проход зеркала.
///
/// Поля `pub(crate)`: решения по вариантам (`.hds-agent\pending`, TR-7.6) живут
/// в [`crate::results`] и работают с теми же путями и воркерами, что и обход —
/// второй экземпляр зеркала с другим набором настроек дал бы расхождение.
pub struct Mirror<'a> {
    pub(crate) project_id: String,
    pub(crate) project_root: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) registry: &'a ArtifactRegistry,
    pub(crate) anonymizer: &'a mut dyn Anonymizer,
    pub(crate) extractor: &'a mut dyn TextExtractor,
    pub(crate) sync: SyncSettings,
    pub(crate) max_file_mb: u64,
}

impl<'a> Mirror<'a> {
    /// Собрать зеркало из явных параметров (тесты — без конфига).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_id: &str,
        project_root: &Path,
        workspace: &Path,
        registry: &'a ArtifactRegistry,
        anonymizer: &'a mut dyn Anonymizer,
        extractor: &'a mut dyn TextExtractor,
        sync: SyncSettings,
        max_file_mb: u64,
    ) -> Mirror<'a> {
        Mirror {
            project_id: project_id.to_string(),
            project_root: project_root.to_path_buf(),
            workspace: workspace.to_path_buf(),
            registry,
            anonymizer,
            extractor,
            sync,
            max_file_mb,
        }
    }

    /// Собрать зеркало по конфигу: `agent.sync.*` и `index.max_file_mb` (В-3).
    pub fn from_config(
        cfg: &Config,
        project_id: &str,
        project_root: &Path,
        registry: &'a ArtifactRegistry,
        anonymizer: &'a mut dyn Anonymizer,
        extractor: &'a mut dyn TextExtractor,
    ) -> Result<Mirror<'a>, MirrorError> {
        let workspace = crate::settings::workspace_dir(cfg, project_id).ok_or_else(|| {
            MirrorError::Config("agent.root не задан — некуда зеркалить (TR-8.1)".to_string())
        })?;
        let max_file_mb = dig(cfg, "index.max_file_mb")
            .and_then(|v| v.as_u64())
            .unwrap_or(200);
        Ok(Mirror::new(
            project_id,
            project_root,
            &workspace,
            registry,
            anonymizer,
            extractor,
            sync_settings(cfg),
            max_file_mb,
        ))
    }

    fn max_bytes(&self) -> u64 {
        self.max_file_mb.saturating_mul(1024 * 1024)
    }

    /// Один проход (A4.2 + A4.3): прямой путь, затем обратный.
    pub fn sync_once(&mut self) -> Result<SyncReport, MirrorError> {
        let mut report = self.sync_forward()?;
        self.sync_reverse(&mut report)?;
        Ok(report)
    }

    /// Прямой путь (A4.2): папка проекта → workspace.
    fn sync_forward(&mut self) -> Result<SyncReport, MirrorError> {
        let mut report = SyncReport::default();
        let files = walk_files(&self.project_root)?;
        report.scanned = files.len();

        let mut by_orig: BTreeMap<String, Artifact> = BTreeMap::new();
        for row in self.registry.list()? {
            by_orig.insert(row.orig_rel.clone(), row);
        }

        let mut present: BTreeSet<String> = BTreeSet::new();
        let mut shas: BTreeMap<String, String> = BTreeMap::new();
        for file in &files {
            present.insert(file.rel.clone());
            let Some(kind) = kind_for(&file.rel) else {
                self.mark_status(&by_orig, file, Kind::Unknown, Status::Unsupported)?;
                report.unsupported += 1;
                continue;
            };
            if file.size > self.max_bytes() {
                self.mark_status(&by_orig, file, kind, Status::TooBig)?;
                report.too_big += 1;
                continue;
            }
            match wait_stable_at(&file.abs, &self.sync, SystemTime::now()) {
                Stable::Ready => {}
                Stable::Deferred => {
                    report.deferred += 1;
                    continue;
                }
                Stable::Gone => continue,
            }
            // Файл мог подрасти, пока ждали: лимит проверяем ещё раз.
            if fs::metadata(&file.abs)
                .map(|m| m.len())
                .unwrap_or(file.size)
                > self.max_bytes()
            {
                self.mark_status(&by_orig, file, kind, Status::TooBig)?;
                report.too_big += 1;
                continue;
            }
            match sha256_file(&file.abs) {
                Ok(sha) => {
                    shas.insert(file.rel.clone(), sha);
                }
                Err(e) => {
                    report.errors += 1;
                    report
                        .problems
                        .push(format!("{}: не прочитан ({e})", file.rel));
                }
            }
        }

        // Исчезнувшие оригиналы: из них же берутся кандидаты на переименование.
        let mut missing: BTreeMap<String, Artifact> = by_orig
            .iter()
            .filter(|(rel, row)| !present.contains(rel.as_str()) && has_image(row))
            .map(|(rel, row)| (rel.clone(), row.clone()))
            .collect();
        let mut missing_by_sha: BTreeMap<String, String> = BTreeMap::new();
        for (rel, row) in &missing {
            if let Some(sha) = &row.orig_sha256 {
                missing_by_sha
                    .entry(sha.clone())
                    .or_insert_with(|| rel.clone());
            }
        }

        let mut taken: BTreeSet<String> = by_orig
            .values()
            .filter(|row| !row.anon_rel.is_empty())
            .map(|row| row.anon_rel.clone())
            .collect();

        for file in &files {
            let Some(sha) = shas.get(&file.rel).cloned() else {
                continue;
            };
            let kind = kind_for(&file.rel).expect("отфильтровано выше");
            if !by_orig.contains_key(&file.rel) {
                if let Some(old_rel) = missing_by_sha.remove(&sha) {
                    if let Some(row) = missing.remove(&old_rel) {
                        match self.rename(&row, file, &mut taken, &mut report) {
                            Ok(true) => continue,
                            Ok(false) => {}
                            Err(e) => {
                                report.errors += 1;
                                report
                                    .problems
                                    .push(format!("{}: переименование — {e}", file.rel));
                                continue;
                            }
                        }
                    }
                }
            }
            let existing = by_orig.get(&file.rel).cloned();
            self.publish(file, &sha, kind, existing.as_ref(), &mut taken, &mut report);
        }

        for row in missing.values() {
            match self.remove_missing(row) {
                Ok(MissingAction::Deleted) => report.removed += 1,
                Ok(MissingAction::Orphan) => report.orphans += 1,
                Ok(MissingAction::Kept) => {}
                Err(e) => {
                    report.errors += 1;
                    report
                        .problems
                        .push(format!("{}: удаление — {e}", row.orig_rel));
                }
            }
        }
        Ok(report)
    }

    /// Статус `unsupported`/`too_big`: запись есть, образа нет (TR-7.3 п.4).
    ///
    /// Идемпотентно: повторная запись с тем же статусом БД не трогает.
    fn mark_status(
        &self,
        rows: &BTreeMap<String, Artifact>,
        file: &ScannedFile,
        kind: Kind,
        status: Status,
    ) -> Result<(), MirrorError> {
        match rows.get(&file.rel) {
            Some(row) if row.status == status => Ok(()),
            Some(row) => {
                if !row.anon_rel.is_empty() {
                    let image = self.workspace.join(&row.anon_rel);
                    let _ = fs::remove_file(&image);
                    self.prune_empty_dirs(image.parent());
                }
                self.registry.set_status_by_orig(&file.rel, status)?;
                Ok(())
            }
            None => {
                self.registry
                    .upsert(&Artifact::new(&file.rel, "", kind, status))?;
                Ok(())
            }
        }
    }

    /// Записать образ (или обновить запись), не трогая чужую правку агента.
    fn publish(
        &mut self,
        file: &ScannedFile,
        sha: &str,
        kind: Kind,
        existing: Option<&Artifact>,
        taken: &mut BTreeSet<String>,
        report: &mut SyncReport,
    ) {
        if let Some(row) = existing {
            if !is_forward_managed(row.status) {
                // `pending_accept`/`conflict`/`accepted` — A4.3: образ не
                // перезаписываем, а расхождение основания фиксируем.
                if matches!(row.status, Status::PendingAccept | Status::Conflict)
                    && row.orig_sha256.as_deref() != Some(sha)
                    && row.status != Status::Conflict
                    && self
                        .registry
                        .set_status_by_orig(&file.rel, Status::Conflict)
                        .is_ok()
                {
                    report.conflicts += 1;
                }
                return;
            }
            if row.orig_sha256.as_deref() != Some(sha) && row.anon_sha256_issued.is_some() {
                // Оригинал изменился. Если параллельно правился и образ —
                // это конфликт TR-7.6 (случай 2): образ агента не затираем.
                if let Ok(current) = sha256_file(&self.workspace.join(&row.anon_rel)) {
                    if row.anon_sha256_issued.as_deref() != Some(current.as_str()) {
                        if row.status != Status::Conflict
                            && self
                                .registry
                                .set_status_by_orig(&file.rel, Status::Conflict)
                                .is_ok()
                        {
                            report.conflicts += 1;
                        }
                        return;
                    }
                }
            }
            if row.orig_sha256.as_deref() == Some(sha) {
                let image = self.workspace.join(&row.anon_rel);
                match sha256_file(&image) {
                    Ok(seen) if row.anon_sha256_issued.as_deref() == Some(seen.as_str()) => {
                        let mark = row.anon_sha256_seen.as_deref() != Some(seen.as_str());
                        let restore = row.status == Status::Orphan;
                        if (mark && self.registry.mark_seen(row.id, Some(&seen)).is_err())
                            || (restore
                                && self
                                    .registry
                                    .set_status_by_orig(&file.rel, Status::Synced)
                                    .is_err())
                        {
                            report.errors += 1;
                            report
                                .problems
                                .push(format!("{}: не записан mark_seen", file.rel));
                        } else {
                            report.unchanged += 1;
                        }
                        return;
                    }
                    Ok(_) => {
                        // Оригинал прежний, образ другой — правка агента.
                        if row.status != Status::AgentChanged
                            && self
                                .registry
                                .set_status_by_orig(&file.rel, Status::AgentChanged)
                                .is_ok()
                        {
                            report.agent_changed += 1;
                        }
                        return;
                    }
                    Err(_) if row.anon_sha256_issued.is_some() => {
                        // Образ исчез: считаем это удалением агентом — оригинал
                        // не трогаем и образ заново не создаём (TR-7.5; решение
                        // «удалить оригинал?» — A5). Пересоздание вернуло бы
                        // файл, который агент осознанно удалил.
                        if row.status != Status::Orphan
                            && self
                                .registry
                                .set_status_by_orig(&file.rel, Status::Orphan)
                                .is_ok()
                        {
                            report.orphans += 1;
                        }
                        return;
                    }
                    Err(_) => {
                        // Образа ещё не было (прошлый проход упал) — пересоздаём.
                    }
                }
            }
        }

        let out_rel = match existing {
            Some(row) if !row.anon_rel.is_empty() => row.anon_rel.clone(),
            _ => match self.plan_anon_rel(&file.rel, kind, taken) {
                Ok(rel) => rel,
                Err(e) => {
                    report.errors += 1;
                    report
                        .problems
                        .push(format!("{}: имя образа — {e}", file.rel));
                    return;
                }
            },
        };
        taken.insert(out_rel.clone());

        let id = match existing {
            Some(row) => row.id,
            None => {
                let mut provisional = Artifact::new(&file.rel, &out_rel, kind, Status::Synced);
                provisional.orig_sha256 = Some(sha.to_string());
                match self.registry.insert(&provisional) {
                    Ok(id) => id,
                    Err(e) => {
                        report.errors += 1;
                        report.problems.push(format!("{}: реестр — {e}", file.rel));
                        return;
                    }
                }
            }
        };

        let out_path = self.workspace.join(&out_rel);
        match self.write_image(file, kind, &out_path, id) {
            Ok(()) => match sha256_file(&out_path) {
                Ok(anon_sha) => {
                    let artifact = Artifact {
                        id,
                        orig_rel: file.rel.clone(),
                        anon_rel: out_rel,
                        kind,
                        orig_sha256: Some(sha.to_string()),
                        anon_sha256_issued: Some(anon_sha.clone()),
                        anon_sha256_seen: Some(anon_sha),
                        status: Status::Synced,
                        ts: 0.0,
                    };
                    if let Err(e) = self.registry.upsert(&artifact) {
                        report.errors += 1;
                        report.problems.push(format!("{}: реестр — {e}", file.rel));
                    } else if existing.is_some() {
                        report.updated += 1;
                    } else {
                        report.created += 1;
                    }
                }
                Err(e) => {
                    let _ = self.registry.set_status_by_orig(&file.rel, Status::Error);
                    report.errors += 1;
                    report
                        .problems
                        .push(format!("{}: образ не прочитан ({e})", file.rel));
                }
            },
            Err(e) => {
                let _ = self.registry.set_status_by_orig(&file.rel, Status::Error);
                report.errors += 1;
                report.problems.push(format!("{}: {e}", file.rel));
            }
        }
    }

    pub(crate) fn write_image(
        &mut self,
        file: &ScannedFile,
        kind: Kind,
        out_path: &Path,
        id: i64,
    ) -> Result<(), MirrorError> {
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| MirrorError::Io(parent.to_path_buf(), e.to_string()))?;
        }
        match kind {
            Kind::Image => {
                self.anonymizer
                    .anonymize_agent(&file.abs, out_path, &self.project_id, id)
            }
            Kind::TextImage => {
                let text = self.extractor.extract(&file.abs)?;
                self.anonymizer
                    .text_image(&file.abs, out_path, &self.project_id, id, Some(&text))
            }
            _ => Err(MirrorError::Config(format!(
                "вид {} не имеет прямого образа",
                kind.as_str()
            ))),
        }
    }

    /// Анонимное имя образа: `anonymize_path` + длина + коллизии (TR-7.1).
    pub(crate) fn plan_anon_rel(
        &mut self,
        rel: &str,
        kind: Kind,
        taken: &BTreeSet<String>,
    ) -> Result<String, MirrorError> {
        let anon = self.anonymizer.anonymize_path(&self.project_id, rel)?;
        let mut components = split_path(&anon);
        if components.is_empty() {
            // Воркер вернул пустое имя — не теряем файл, берём исходное.
            components = split_path(rel);
        }
        let mut components: Vec<String> = components
            .iter()
            .map(|part| {
                if part == ".." {
                    part.clone()
                } else {
                    fit_component(part)
                }
            })
            .collect();
        let last = components.len() - 1;
        if kind == Kind::TextImage {
            components[last].push_str(".md");
        }
        let separator = std::path::MAIN_SEPARATOR.to_string();
        let (stem, ext) = split_name(&components[last]);
        let (stem, ext) = (stem.to_string(), ext.to_string());
        let mut n = 2u32;
        let mut candidate = components.join(&separator);
        while taken.contains(&candidate) || self.workspace.join(&candidate).exists() {
            components[last] = format!("{stem} ({n}){ext}");
            n += 1;
            candidate = components.join(&separator);
        }
        Ok(candidate)
    }

    /// Переименование оригинала → переименование образа (TR-7.3).
    ///
    /// `Ok(false)` — переименовать нельзя (образ пропал/занят): вызывающий
    /// сделает обычный образ, старая запись уйдёт как удалённая.
    fn rename(
        &mut self,
        row: &Artifact,
        file: &ScannedFile,
        taken: &mut BTreeSet<String>,
        report: &mut SyncReport,
    ) -> Result<bool, MirrorError> {
        let old_path = self.workspace.join(&row.anon_rel);
        if !old_path.is_file() {
            return Ok(false);
        }
        let mut without = taken.clone();
        without.remove(&row.anon_rel);
        let new_rel = match self.plan_anon_rel(&file.rel, row.kind, &without) {
            Ok(rel) => rel,
            Err(_) => return Ok(false),
        };
        let new_path = self.workspace.join(&new_rel);
        if new_path.exists() {
            return Ok(false);
        }
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| MirrorError::Io(parent.to_path_buf(), e.to_string()))?;
        }
        fs::rename(&old_path, &new_path)
            .map_err(|e| MirrorError::Io(new_path.clone(), e.to_string()))?;
        self.prune_empty_dirs(old_path.parent());
        self.registry.rename_orig(row.id, &file.rel, &new_rel)?;
        taken.remove(&row.anon_rel);
        taken.insert(new_rel);
        report.renamed += 1;
        Ok(true)
    }

    /// Исчезнувший оригинал: удалить образ или оставить `orphan` (TR-7.3 п.5).
    fn remove_missing(&self, row: &Artifact) -> Result<MissingAction, MirrorError> {
        if !matches!(
            row.status,
            Status::Synced
                | Status::Error
                | Status::Unsupported
                | Status::TooBig
                | Status::Accepted
        ) {
            return Ok(MissingAction::Kept);
        }
        if row.anon_rel.is_empty() {
            self.registry.delete(row.id)?;
            return Ok(MissingAction::Deleted);
        }
        let image = self.workspace.join(&row.anon_rel);
        if !image.is_file() {
            self.registry.delete(row.id)?;
            return Ok(MissingAction::Deleted);
        }
        let seen = sha256_file(&image).ok();
        let untouched = match (&seen, &row.anon_sha256_issued) {
            (Some(seen), Some(issued)) => seen == issued,
            (_, None) => true,
            _ => false,
        };
        if untouched {
            let _ = fs::remove_file(&image);
            self.prune_empty_dirs(image.parent());
            self.registry.delete(row.id)?;
            Ok(MissingAction::Deleted)
        } else {
            self.registry
                .set_status_by_orig(&row.orig_rel, Status::Orphan)?;
            Ok(MissingAction::Orphan)
        }
    }

    // ── обратный путь (A4.3, TR-7.5/7.7) ───────────────────────────────

    /// Обратный путь: workspace → папка проекта.
    ///
    /// * новый файл без нашего маркера (`agent_new`) — деанонимизируется и
    ///   кладётся на своё место (суффикс ` (агент)` при занятом имени);
    /// * образ изменён агентом (`seen != issued`) — снимок основания и
    ///   вариант в `.hds-agent\pending` (TR-7.5), оригинал не трогается;
    /// * образ исчез — оригинал остаётся, запись `orphan` (A5 предложит
    ///   удалить оригинал);
    /// * наш маркер `hds-agent/<P>/<id>` повторно не выпускается (TR-7.7).
    fn sync_reverse(&mut self, report: &mut SyncReport) -> Result<(), MirrorError> {
        if !self.workspace.is_dir() {
            // До первого образа анонимной зоны может не быть вовсе.
            return Ok(());
        }
        let files = walk_files(&self.workspace)?;
        let rows = self.registry.list()?;
        let mut by_anon: BTreeMap<String, Artifact> = BTreeMap::new();
        let mut by_id: BTreeMap<i64, Artifact> = BTreeMap::new();
        for row in &rows {
            if !row.anon_rel.is_empty() {
                by_anon.insert(row.anon_rel.clone(), row.clone());
            }
            by_id.insert(row.id, row.clone());
        }
        let mut present: BTreeSet<String> = BTreeSet::new();
        for file in &files {
            present.insert(file.rel.clone());
            let Some(row) = by_anon.get(&file.rel).cloned() else {
                self.consider_new_file(file, &by_id, report);
                continue;
            };
            let Ok(sha) = sha256_file(&file.abs) else {
                continue;
            };
            if row.anon_sha256_issued.as_deref() == Some(sha.as_str()) {
                continue;
            }
            if matches!(
                row.status,
                Status::Synced
                    | Status::Error
                    | Status::AgentChanged
                    | Status::PendingAccept
                    | Status::Accepted
            ) {
                self.make_variant(&row, file, &sha, report);
            }
        }
        for row in by_anon.values() {
            if present.contains(&row.anon_rel) {
                continue;
            }
            if !matches!(
                row.status,
                Status::Synced | Status::Error | Status::AgentChanged | Status::Accepted
            ) {
                continue;
            }
            // Образа не было (упавший первый проход) — запись не «осиротела».
            if row.anon_sha256_issued.is_none() {
                continue;
            }
            if self
                .registry
                .set_status_by_orig(&row.orig_rel, Status::Orphan)
                .is_ok()
            {
                report.orphans += 1;
            }
        }
        Ok(())
    }

    /// Файл workspace без записи по `anon_rel`: новый файл агента или образ.
    fn consider_new_file(
        &mut self,
        file: &ScannedFile,
        by_id: &BTreeMap<i64, Artifact>,
        report: &mut SyncReport,
    ) {
        let marker = match self.anonymizer.read_marker(&file.abs) {
            Ok(marker) => marker,
            Err(e) => {
                report.errors += 1;
                report.problems.push(format!("{}: маркер — {e}", file.rel));
                return;
            }
        };
        if marker.project_id.as_deref() == Some(self.project_id.as_str()) {
            // Наш маркер: файл выпущен демоном. Повторно его не выпускаем
            // (TR-7.7), а неизвестный id значит «реестр потерян» — это не
            // повод продублировать файл в папке проекта.
            if let Some(id) = marker.artifact_id {
                if !by_id.contains_key(&id) {
                    report.problems.push(format!(
                        "{}: маркер hds-agent/{}/{id} без записи реестра — пропущен",
                        file.rel, self.project_id
                    ));
                }
            }
            return;
        }
        self.emit_agent_new(file, report);
    }

    /// Изменённый агентом образ → снимок основания + вариант в `pending`.
    fn make_variant(
        &mut self,
        row: &Artifact,
        file: &ScannedFile,
        sha: &str,
        report: &mut SyncReport,
    ) {
        // Основание — версия оригинала, из которой сделан образ. Если
        // пользователь успел изменить оригинал, вариант собрать нельзя:
        // это конфликт TR-7.6, случай 2 (молча ничего не теряем).
        let original = self.project_root.join(rel_to_path(&row.orig_rel));
        let original_sha = sha256_file(&original).ok();
        let base_ok = original_sha
            .as_deref()
            .zip(row.orig_sha256.as_deref())
            .map(|(seen, base)| seen == base)
            .unwrap_or(false);
        if !base_ok {
            if row.status != Status::Conflict
                && self
                    .registry
                    .set_status_by_orig(&row.orig_rel, Status::Conflict)
                    .is_ok()
            {
                report.conflicts += 1;
            }
            return;
        }

        let base_path = self.base_path(&row.orig_rel);
        if !base_path.is_file() {
            if let Err(e) = copy_file(&original, &base_path) {
                report.errors += 1;
                report
                    .problems
                    .push(format!("{}: снимок основания — {e}", row.orig_rel));
                return;
            }
            let map_path = self.image_map_path(&base_path);
            match self.anonymizer.image_map(&base_path) {
                Ok(map) => {
                    let text =
                        serde_json::to_string_pretty(&map).unwrap_or_else(|_| "[]".to_string());
                    if let Err(e) = fs::write(&map_path, text) {
                        report.problems.push(format!(
                            "{}: карта картинок не записана ({e})",
                            row.orig_rel
                        ));
                    }
                }
                Err(e) => report
                    .problems
                    .push(format!("{}: карта картинок — {e}", row.orig_rel)),
            }
        }

        let pending = self.pending_path(&row.orig_rel);
        if let Some(parent) = pending.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                report.errors += 1;
                report
                    .problems
                    .push(format!("{}: каталог варианта — {e}", row.orig_rel));
                return;
            }
        }
        match self.anonymizer.deanonymize_agent(
            &file.abs,
            &pending,
            &self.project_id,
            Some(&base_path),
        ) {
            Ok(result) => {
                for warning in &result.warnings {
                    report.problems.push(format!("{}: {warning}", row.orig_rel));
                }
                match self.registry.set_issued(row.id, sha, Status::PendingAccept) {
                    Ok(()) => report.variants += 1,
                    Err(e) => {
                        report.errors += 1;
                        report
                            .problems
                            .push(format!("{}: реестр — {e}", row.orig_rel));
                    }
                }
            }
            Err(e) => {
                // Статус не портим: следующий проход повторит сборку.
                report.errors += 1;
                report
                    .problems
                    .push(format!("{}: вариант не собран — {e}", row.orig_rel));
            }
        }
    }

    /// Новый файл агента → папка проекта (D5, TR-7.5/7.9).
    fn emit_agent_new(&mut self, file: &ScannedFile, report: &mut SyncReport) {
        let deanon = match self
            .anonymizer
            .deanonymize_path(&self.project_id, &file.rel)
        {
            Ok(path) => path,
            Err(e) => {
                report.errors += 1;
                report.problems.push(format!("{}: имя — {e}", file.rel));
                return;
            }
        };
        for token in &deanon.unresolved {
            report.problems.push(format!(
                "{}: метка {token} не найдена в базе проекта (оставлена в имени)",
                file.rel
            ));
        }
        let Some(mut components) = safe_components(&deanon.components) else {
            report.errors += 1;
            report.problems.push(format!(
                "{}: небезопасный путь после деанонимизации — пропущен",
                file.rel
            ));
            return;
        };
        if components.is_empty() {
            components = split_path(&file.rel);
        }
        let separator = std::path::MAIN_SEPARATOR.to_string();
        let rel = components.join(&separator);
        let (target_rel, target) = self.free_project_name(&rel);

        let tmp_root = self.tmp_root();
        if let Err(e) = fs::create_dir_all(&tmp_root) {
            report.errors += 1;
            report
                .problems
                .push(format!("{}: каталог обмена — {e}", file.rel));
            return;
        }
        let tmp = self.tmp_path(&file.rel);
        match self
            .anonymizer
            .deanonymize_agent(&file.abs, &tmp, &self.project_id, None)
        {
            Ok(result) => {
                for warning in &result.warnings {
                    report.problems.push(format!("{}: {warning}", file.rel));
                }
                if let Err(e) = move_file(&tmp, &target) {
                    let _ = fs::remove_file(&tmp);
                    report.errors += 1;
                    report
                        .problems
                        .push(format!("{}: выпуск в проект — {e}", file.rel));
                    return;
                }
                let orig_sha = sha256_file(&target);
                let anon_sha = sha256_file(&file.abs);
                let (Ok(orig_sha), Ok(anon_sha)) = (orig_sha, anon_sha) else {
                    report.errors += 1;
                    report
                        .problems
                        .push(format!("{}: выпуск не прочитан", file.rel));
                    return;
                };
                let artifact = Artifact {
                    id: 0,
                    orig_rel: target_rel,
                    anon_rel: file.rel.clone(),
                    kind: Kind::AgentNew,
                    orig_sha256: Some(orig_sha),
                    anon_sha256_issued: Some(anon_sha.clone()),
                    anon_sha256_seen: Some(anon_sha),
                    status: Status::Synced,
                    ts: 0.0,
                };
                if let Err(e) = self.registry.upsert(&artifact) {
                    report.errors += 1;
                    report.problems.push(format!("{}: реестр — {e}", file.rel));
                } else {
                    report.emitted += 1;
                }
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                report.errors += 1;
                report
                    .problems
                    .push(format!("{}: деанонимизация — {e}", file.rel));
            }
        }
    }

    /// Свободное имя в папке проекта: `имя (агент).ext`, `имя (агент 2).ext`.
    fn free_project_name(&self, rel: &str) -> (String, PathBuf) {
        let separator = std::path::MAIN_SEPARATOR.to_string();
        let mut components = split_path(rel);
        let Some(last) = components.last().cloned() else {
            return (rel.to_string(), self.project_root.join(rel_to_path(rel)));
        };
        let (stem, ext) = split_name(&last);
        let (stem, ext) = (stem.to_string(), ext.to_string());
        let mut candidate_rel = components.join(&separator);
        let mut candidate = self.project_root.join(rel_to_path(&candidate_rel));
        if !candidate.exists() {
            return (candidate_rel, candidate);
        }
        let index = components.len() - 1;
        for n in 1..10_000u32 {
            components[index] = if n == 1 {
                format!("{stem} (агент){ext}")
            } else {
                format!("{stem} (агент {n}){ext}")
            };
            candidate_rel = components.join(&separator);
            candidate = self.project_root.join(rel_to_path(&candidate_rel));
            if !candidate.exists() {
                return (candidate_rel, candidate);
            }
        }
        (candidate_rel, candidate)
    }

    pub(crate) fn base_path(&self, orig_rel: &str) -> PathBuf {
        self.project_root
            .join(".hds-agent")
            .join("base")
            .join(rel_to_path(orig_rel))
    }

    pub(crate) fn pending_path(&self, orig_rel: &str) -> PathBuf {
        self.project_root
            .join(".hds-agent")
            .join("pending")
            .join(rel_to_path(orig_rel))
    }

    pub(crate) fn tmp_root(&self) -> PathBuf {
        self.project_root.join(".hds-agent").join("tmp")
    }

    fn tmp_path(&self, anon_rel: &str) -> PathBuf {
        let hash = &sha256_hex(anon_rel.as_bytes())[..12];
        let mut candidate = self.tmp_root().join(format!("{hash}.tmp"));
        let mut n = 2u32;
        while candidate.exists() {
            candidate = self.tmp_root().join(format!("{hash}-{n}.tmp"));
            n += 1;
        }
        candidate
    }

    pub(crate) fn image_map_path(&self, base_path: &Path) -> PathBuf {
        PathBuf::from(format!("{}.images.json", base_path.to_string_lossy()))
    }

    /// Убрать опустевшие каталоги образа (но не сам workspace).
    fn prune_empty_dirs(&self, mut dir: Option<&Path>) {
        while let Some(current) = dir {
            if current == self.workspace || !current.starts_with(&self.workspace) {
                break;
            }
            if fs::remove_dir(current).is_err() {
                break;
            }
            dir = current.parent();
        }
    }
}

/// Что стало с записью исчезнувшего оригинала.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingAction {
    Deleted,
    Orphan,
    Kept,
}

/// Статусы, которыми управляет прямой путь (A4.2).
///
/// `accepted` (A4.4) — тоже обычный артефакт: после «Принять» оригинал равен
/// варианту, образ пересоздан, и пользователь продолжает править документ.
/// Без этого статуса принятая запись замирала бы: прямой путь её больше не
/// обслуживал бы, а образ агента после приёма не обновлялся бы никогда.
fn is_forward_managed(status: Status) -> bool {
    matches!(
        status,
        Status::Synced
            | Status::Error
            | Status::Unsupported
            | Status::TooBig
            | Status::Orphan
            | Status::Accepted
    )
}

/// Относительный путь (`a\b\c`) → `PathBuf` без обращения к ФС.
pub(crate) fn rel_to_path(rel: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for part in split_path(rel) {
        path.push(part);
    }
    path
}

/// Компоненты после деанонимизации: без `..`, абсолютных путей и `:`.
///
/// `None` — путь небезопасен (значение метки содержит `..`/`:`/разделитель):
/// такой файл лучше пропустить с ошибкой, чем выпустить вне папки проекта.
fn safe_components(components: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::with_capacity(components.len());
    for part in components {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') {
            return None;
        }
        if part.contains('\\') || part.contains('/') {
            return None;
        }
        if Path::new(part).is_absolute() {
            return None;
        }
        out.push(part.clone());
    }
    Some(out)
}

/// Скопировать файл, создав каталоги назначения (снимок основания, backup TR-7.6).
pub(crate) fn copy_file(src: &Path, dst: &Path) -> Result<(), MirrorError> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| MirrorError::Io(parent.to_path_buf(), e.to_string()))?;
    }
    fs::copy(src, dst)
        .map(|_| ())
        .map_err(|e| MirrorError::Io(dst.to_path_buf(), e.to_string()))
}

/// Перенести файл (выпуск в папку проекта). `rename` — атомарно в пределах тома;
/// на другой том — копирование с удалением источника.
fn move_file(src: &Path, dst: &Path) -> Result<(), MirrorError> {
    if let Ok(()) = fs::rename(src, dst) {
        return Ok(());
    }
    copy_file(src, dst)?;
    fs::remove_file(src).map_err(|e| MirrorError::Io(src.to_path_buf(), e.to_string()))
}

/// Есть ли у записи образ (кандидат на удаление/переименование).
fn has_image(row: &Artifact) -> bool {
    if matches!(
        row.status,
        Status::Synced | Status::Error | Status::Orphan | Status::Accepted
    ) {
        return true;
    }
    // `unsupported`/`too_big` образа не имеют, но исчезнувший оригинал должен
    // убирать и их запись — иначе реестр копил бы «файлы, которых нет».
    row.anon_rel.is_empty() && matches!(row.status, Status::Unsupported | Status::TooBig)
}

#[cfg(test)]
pub(crate) mod tests {
    //! Тесты зеркала; поддельные воркеры и стенд (`Stand`, `FakeAnon`,
    //! `FakeExtractor`) переиспользуются тестами [`crate::results`] — второй
    //! такой же набор держать незачем (A4.4).
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Тесты одного файла идут параллельно в одном процессе — в имени
    /// временного каталога есть счётчик (A3 §4 п.16).
    pub(crate) fn tmpdir(name: &str) -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("hds-a42-{name}-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Настройки без ожиданий: тест не спит `wait_stable_sec` на каждом файле.
    pub(crate) fn fast() -> SyncSettings {
        SyncSettings {
            interval_sec: 5,
            wait_stable_sec: 0,
            wait_poll_ms: 1,
            wait_max_sec: 0,
            settle_age_sec: 0,
        }
    }

    pub(crate) struct Stand {
        pub(crate) root: PathBuf,
        pub(crate) project: PathBuf,
        pub(crate) workspace: PathBuf,
        pub(crate) db: PathBuf,
    }

    impl Stand {
        pub(crate) fn new(tag: &str) -> Stand {
            let root = tmpdir(tag);
            let project = root.join("Проект");
            let workspace = root.join("zone").join("P0001").join("workspace");
            fs::create_dir_all(&project).unwrap();
            fs::create_dir_all(&workspace).unwrap();
            let db = root.join("registry").join("P0001.db");
            Stand {
                root,
                project,
                workspace,
                db,
            }
        }

        pub(crate) fn write(&self, rel: &str, text: &str) {
            let path = self.project.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        pub(crate) fn write_ws(&self, rel: &str, text: &str) {
            let path = self.workspace.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        pub(crate) fn remove(&self, rel: &str) {
            fs::remove_file(self.project.join(rel)).unwrap();
        }

        pub(crate) fn rename(&self, from: &str, to: &str) {
            let dst = self.project.join(to);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::rename(self.project.join(from), dst).unwrap();
        }

        pub(crate) fn run(
            &self,
            anon: &mut dyn Anonymizer,
            ext: &mut dyn TextExtractor,
            max_file_mb: u64,
        ) -> SyncReport {
            let registry = ArtifactRegistry::open(&self.db).unwrap();
            let mut mirror = Mirror::new(
                "P0001",
                &self.project,
                &self.workspace,
                &registry,
                anon,
                ext,
                fast(),
                max_file_mb,
            );
            mirror.sync_once().expect("проход зеркала")
        }

        pub(crate) fn registry(&self) -> ArtifactRegistry {
            ArtifactRegistry::open(&self.db).unwrap()
        }

        pub(crate) fn image(&self, rel: &str) -> String {
            fs::read_to_string(self.workspace.join(rel)).unwrap_or_default()
        }
    }

    impl Drop for Stand {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[derive(Default)]
    pub(crate) struct FakeAnon {
        pub(crate) path_calls: usize,
        pub(crate) image_calls: usize,
        pub(crate) text_calls: usize,
        pub(crate) deanon_calls: usize,
        pub(crate) fail_on: Option<String>,
        pub(crate) collide: bool,
    }

    impl FakeAnon {
        pub(crate) fn map(text: &str) -> String {
            text.replace("Иванов Иван", "[PERSON_1]")
                .replace("Петров Пётр", "[PERSON_2]")
        }

        pub(crate) fn deanon(text: &str) -> String {
            text.replace("[PERSON_1]", "Иванов Иван")
                .replace("[PERSON_2]", "Петров Пётр")
        }
    }

    impl Anonymizer for FakeAnon {
        fn anonymize_path(&mut self, _project: &str, rel: &str) -> Result<String, MirrorError> {
            self.path_calls += 1;
            let mut mapped = Self::map(rel);
            if self.collide {
                let mut comps = split_path(&mapped);
                if let Some(last) = comps.last_mut() {
                    let ext = Path::new(last)
                        .extension()
                        .map(|e| format!(".{}", e.to_string_lossy()))
                        .unwrap_or_default();
                    *last = format!("same{ext}");
                }
                mapped = comps.join("\\");
            }
            Ok(mapped)
        }

        fn anonymize_agent(
            &mut self,
            in_path: &Path,
            out_path: &Path,
            project_id: &str,
            artifact_id: i64,
        ) -> Result<(), MirrorError> {
            self.image_calls += 1;
            let name = in_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if self.fail_on.as_deref() == Some(name.as_str()) {
                return Err(MirrorError::Anonymizer("сбой теста".to_string()));
            }
            let text = fs::read_to_string(in_path)
                .map_err(|e| MirrorError::Io(in_path.to_path_buf(), e.to_string()))?;
            let body = format!(
                "<!-- hds-agent/{project_id}/{artifact_id} -->\n{}",
                Self::map(&text)
            );
            fs::write(out_path, body)
                .map_err(|e| MirrorError::Io(out_path.to_path_buf(), e.to_string()))
        }

        fn text_image(
            &mut self,
            _in_path: &Path,
            out_path: &Path,
            project_id: &str,
            artifact_id: i64,
            text: Option<&str>,
        ) -> Result<(), MirrorError> {
            self.text_calls += 1;
            let body = format!(
                "<!-- hds-agent/{project_id}/{artifact_id} -->\n{}",
                Self::map(text.unwrap_or_default())
            );
            fs::write(out_path, body)
                .map_err(|e| MirrorError::Io(out_path.to_path_buf(), e.to_string()))
        }

        fn deanonymize_path(
            &mut self,
            _project: &str,
            rel: &str,
        ) -> Result<hds_anonymize::DeanonymizedPath, MirrorError> {
            let path = Self::deanon(rel);
            Ok(hds_anonymize::DeanonymizedPath {
                path: path.clone(),
                components: split_path(&path),
                unresolved: Vec::new(),
            })
        }

        fn read_marker(&mut self, path: &Path) -> Result<hds_anonymize::MarkerInfo, MirrorError> {
            // Настоящий маркер живёт в properties/первой строке; фейк пишет
            // его первой строкой, поэтому распознаём так же — и после
            // переименования образа маркер остаётся.
            let text = fs::read_to_string(path).unwrap_or_default();
            let first = text.lines().next().unwrap_or("");
            let Some(rest) = first.strip_prefix("<!-- hds-agent/") else {
                return Ok(hds_anonymize::MarkerInfo::default());
            };
            let marker = rest.trim_end_matches(" -->").to_string();
            let mut parts = marker.split('/');
            let project = parts.next().unwrap_or_default().to_string();
            let artifact_id = parts.next().and_then(|id| id.parse::<i64>().ok());
            Ok(hds_anonymize::MarkerInfo {
                marker: Some(marker),
                project_id: Some(project),
                artifact_id,
            })
        }

        fn image_map(&mut self, _path: &Path) -> Result<serde_json::Value, MirrorError> {
            Ok(serde_json::json!([]))
        }

        fn deanonymize_agent(
            &mut self,
            in_path: &Path,
            out_path: &Path,
            _project: &str,
            _restore_from: Option<&Path>,
        ) -> Result<hds_anonymize::DeanonymizeResult, MirrorError> {
            self.deanon_calls += 1;
            let text = fs::read_to_string(in_path)
                .map_err(|e| MirrorError::Io(in_path.to_path_buf(), e.to_string()))?;
            // Как настоящий воркер (A4.3): маркер образа в вариант не переносится,
            // иначе «Принять» положило бы маркер в оригинал пользователя.
            let body = match text.strip_prefix("<!-- hds-agent/") {
                Some(rest) => match rest.split_once("-->") {
                    Some((_, tail)) => tail.trim_start_matches(['\r', '\n']).to_string(),
                    None => text.clone(),
                },
                None => text.clone(),
            };
            fs::write(out_path, Self::deanon(&body))
                .map_err(|e| MirrorError::Io(out_path.to_path_buf(), e.to_string()))?;
            Ok(hds_anonymize::DeanonymizeResult {
                out_path: out_path.to_string_lossy().into_owned(),
                restored: Vec::new(),
                unmatched: Vec::new(),
                warnings: Vec::new(),
                images_restored: 0,
                elapsed_ms: 0.0,
            })
        }
    }

    #[derive(Default)]
    pub(crate) struct FakeExtractor {
        pub(crate) calls: usize,
    }

    impl TextExtractor for FakeExtractor {
        fn extract(&mut self, _path: &Path) -> Result<String, MirrorError> {
            self.calls += 1;
            Ok("<!-- страница 1 -->\n\nТекст документа: Иванов Иван".to_string())
        }
    }

    #[test]
    fn classifies_kinds_and_service_entries() {
        assert_eq!(kind_for("a.docx"), Some(Kind::Image));
        assert_eq!(kind_for("a.XLSX"), Some(Kind::Image));
        assert_eq!(kind_for("a.txt"), Some(Kind::Image));
        assert_eq!(kind_for("a.md"), Some(Kind::Image));
        assert_eq!(kind_for("a.xml"), Some(Kind::Image));
        assert_eq!(kind_for("a.pdf"), Some(Kind::TextImage));
        assert_eq!(kind_for("a.pptx"), Some(Kind::TextImage));
        assert_eq!(kind_for("a.mpp"), Some(Kind::TextImage));
        assert_eq!(kind_for("a.exe"), None);
        assert_eq!(kind_for("без расширения"), None);

        assert!(is_service_dir(".hds-agent"));
        assert!(is_service_dir("_attachments"));
        assert!(is_service_dir("_conflicts"));
        assert!(!is_service_dir("Проект"));
        assert!(is_service_file("~$ТЗ.docx"));
        assert!(is_service_file("x.tmp"));
        assert!(is_service_file("Thumbs.db"));
        assert!(is_service_file("thumbs.db"));
        assert!(!is_service_file("ТЗ.docx"));
    }

    #[test]
    fn fit_component_truncates_and_keeps_extension() {
        let short = "Встреча [PERSON_1].docx";
        assert_eq!(fit_component(short), short);

        let long = format!("{}.docx", "Очень длинное имя ".repeat(20));
        let fitted = fit_component(&long);
        assert!(fitted.chars().count() <= MAX_NAME_LEN, "{fitted}");
        assert!(fitted.ends_with(".docx"), "{fitted}");
        assert!(fitted.contains('-'), "{fitted}");

        let other = format!("{}.docx", "Другое очень длинное имя ".repeat(20));
        assert_ne!(fit_component(&other), fitted, "хеш различает длинные имена");
    }

    #[test]
    fn unique_name_adds_counter() {
        let mut taken = BTreeSet::new();
        assert_eq!(unique_name("a.docx", &taken), "a.docx");
        taken.insert("a.docx".to_string());
        assert_eq!(unique_name("a.docx", &taken), "a (2).docx");
        taken.insert("a (2).docx".to_string());
        assert_eq!(unique_name("a.docx", &taken), "a (3).docx");
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn split_path_understands_both_separators() {
        assert_eq!(
            split_path("a\\b/c\\..\\d"),
            vec!["a", "b", "c", "..", "d"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert!(split_path(".\\").is_empty());
    }

    #[test]
    fn wait_stable_old_ready_fresh_deferred_gone() {
        let dir = tmpdir("wait");
        let path = dir.join("a.txt");
        fs::write(&path, "текст").unwrap();
        // Файл старше settle_age_sec — не ждём.
        let now = SystemTime::now() + Duration::from_secs(10);
        assert_eq!(wait_stable_at(&path, &fast(), now), Stable::Ready);

        // Свежий файл и нулевой максимум ожидания — откладываем.
        let opts = SyncSettings {
            settle_age_sec: 3600,
            wait_stable_sec: 2,
            wait_max_sec: 0,
            ..fast()
        };
        assert_eq!(
            wait_stable_at(&path, &opts, SystemTime::now()),
            Stable::Deferred
        );

        assert_eq!(
            wait_stable_at(&dir.join("нет.txt"), &fast(), now),
            Stable::Gone
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn walk_skips_service_entries_and_keeps_tree() {
        let stand = Stand::new("walk");
        stand.write("a.docx", "1");
        stand.write("~$a.docx", "2");
        stand.write("x.tmp", "3");
        stand.write("Thumbs.db", "4");
        stand.write(".hds-agent\\state.json", "5");
        stand.write("_conflicts\\c.docx", "6");
        stand.write("Подпапка\\b.txt", "7");
        let files = walk_files(&stand.project).unwrap();
        let rels: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();
        assert_eq!(
            rels,
            vec!["a.docx".to_string(), "Подпапка\\b.txt".to_string()]
        );
    }

    #[test]
    fn first_pass_creates_images_and_statuses() {
        let stand = Stand::new("first");
        stand.write(
            "Протоколы\\Встреча Иванов Иван.docx",
            "Контакт: Иванов Иван",
        );
        stand.write("ТЗ.pdf", "pdf-байты");
        stand.write("данные.bin", "binary");
        stand.write("черновик.tmp", "tmp");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        let report = stand.run(&mut anon, &mut ext, 200);

        assert_eq!(report.scanned, 3, "tmp не считается");
        assert_eq!(report.created, 2);
        assert_eq!(report.unsupported, 1);
        assert_eq!(report.errors, 0, "{:?}", report.problems);
        assert_eq!(ext.calls, 1, "извлечение только для pdf");

        let image = stand.image("Протоколы\\Встреча [PERSON_1].docx");
        assert!(image.contains("[PERSON_1]"), "{image}");
        assert!(!image.contains("Иванов Иван"), "{image}");
        assert!(image.starts_with("<!-- hds-agent/P0001/"), "{image}");

        let text_image = stand.image("ТЗ.pdf.md");
        assert!(text_image.contains("[PERSON_1]"), "{text_image}");
        assert!(text_image.contains("hds-agent/P0001/"), "{text_image}");

        let registry = stand.registry();
        let docx = registry
            .by_orig("Протоколы\\Встреча Иванов Иван.docx")
            .unwrap()
            .expect("запись docx");
        assert_eq!(docx.kind, Kind::Image);
        assert_eq!(docx.status, Status::Synced);
        assert!(docx.orig_sha256.is_some());
        assert_eq!(docx.anon_sha256_issued, docx.anon_sha256_seen);

        let pdf = registry.by_orig("ТЗ.pdf").unwrap().expect("запись pdf");
        assert_eq!(pdf.kind, Kind::TextImage);
        assert_eq!(pdf.anon_rel, "ТЗ.pdf.md");

        let bin = registry.by_orig("данные.bin").unwrap().expect("запись bin");
        assert_eq!(bin.status, Status::Unsupported);
        assert!(bin.anon_rel.is_empty());
    }

    #[test]
    fn second_pass_changes_nothing() {
        let stand = Stand::new("idem");
        stand.write(
            "Протоколы\\Встреча Иванов Иван.docx",
            "Контакт: Иванов Иван",
        );
        stand.write("ТЗ.pdf", "pdf");
        stand.write("данные.bin", "binary");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        let first = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(first.created, 2);
        let image_sha = sha256_file(&stand.workspace.join("ТЗ.pdf.md")).unwrap();
        let records_before = stand.registry().count().unwrap();
        let images_before = anon.image_calls;

        let second = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(second.changes(), 0, "{second:?}");
        assert_eq!(second.unchanged, 2);
        assert_eq!(second.unsupported, 1);
        assert_eq!(anon.image_calls, images_before, "образы не пересоздавались");
        assert_eq!(stand.registry().count().unwrap(), records_before);
        assert_eq!(
            sha256_file(&stand.workspace.join("ТЗ.pdf.md")).unwrap(),
            image_sha
        );
    }

    #[test]
    fn changed_original_recreates_image() {
        let stand = Stand::new("changed");
        stand.write("Заметки Иванов Иван.md", "старый текст");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);

        stand.write("Заметки Иванов Иван.md", "новый текст");
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.updated, 1, "{report:?}");
        assert_eq!(report.created, 0);
        let image = stand.image("Заметки [PERSON_1].md");
        assert!(image.contains("новый текст"), "{image}");
        assert!(!image.contains("старый текст"), "{image}");
    }

    #[test]
    fn rename_moves_image_without_reanonymizing() {
        let stand = Stand::new("rename");
        stand.write("Акт Иванов Иван.docx", "текст акта");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);
        let id_before = stand
            .registry()
            .by_orig("Акт Иванов Иван.docx")
            .unwrap()
            .unwrap()
            .id;
        let images_before = anon.image_calls;

        stand.rename("Акт Иванов Иван.docx", "Протоколы\\Акт Петров Пётр.docx");
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.renamed, 1, "{report:?}");
        assert_eq!(report.created, 0);
        assert_eq!(report.removed, 0);
        assert_eq!(
            anon.image_calls, images_before,
            "повторной анонимизации нет"
        );

        let registry = stand.registry();
        assert!(registry.by_orig("Акт Иванов Иван.docx").unwrap().is_none());
        let moved = registry
            .by_orig("Протоколы\\Акт Петров Пётр.docx")
            .unwrap()
            .expect("запись после переименования");
        assert_eq!(moved.id, id_before, "id артефакта сохраняется");
        assert_eq!(moved.anon_rel, "Протоколы\\Акт [PERSON_2].docx");
        assert!(stand
            .workspace
            .join("Протоколы\\Акт [PERSON_2].docx")
            .is_file());
        assert!(!stand.workspace.join("Акт [PERSON_1].docx").exists());
    }

    #[test]
    fn deleted_original_removes_image_and_row() {
        let stand = Stand::new("delete");
        stand.write("Встреча Иванов Иван.docx", "текст");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);
        stand.remove("Встреча Иванов Иван.docx");

        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.removed, 1, "{report:?}");
        assert_eq!(report.changes(), 1);
        assert!(!stand.workspace.join("Встреча [PERSON_1].docx").exists());
        assert_eq!(stand.registry().count().unwrap(), 0);
    }

    #[test]
    fn agent_modified_image_is_not_overwritten() {
        let stand = Stand::new("agent");
        stand.write("Письмо Иванов Иван.md", "текст письма");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);

        let image_path = stand.workspace.join("Письмо [PERSON_1].md");
        fs::write(&image_path, "правка агента").unwrap();
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.agent_changed, 1, "{report:?}");
        assert_eq!(report.variants, 1, "A4.3 собрал вариант: {report:?}");
        assert_eq!(
            fs::read_to_string(&image_path).unwrap(),
            "правка агента",
            "прямой путь не затирает правку агента"
        );
        let row = stand
            .registry()
            .by_orig("Письмо Иванов Иван.md")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, Status::PendingAccept);

        // Повторный прогон статус уже не меняет.
        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
    }

    #[test]
    fn deleted_original_with_agent_change_becomes_orphan() {
        let stand = Stand::new("orphan");
        stand.write("Письмо Иванов Иван.md", "текст письма");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);
        let image_path = stand.workspace.join("Письмо [PERSON_1].md");
        fs::write(&image_path, "правка агента").unwrap();
        stand.remove("Письмо Иванов Иван.md");

        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.orphans, 1, "{report:?}");
        assert!(image_path.is_file(), "образ правки агента не удаляется");
        let row = stand
            .registry()
            .by_orig("Письмо Иванов Иван.md")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, Status::Orphan);
    }

    #[test]
    fn too_big_marks_status_and_writes_nothing() {
        let stand = Stand::new("toobig");
        stand.write("Тяжёлый Иванов Иван.docx", "данные");
        stand.write("данные.bin", "binary");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        let report = stand.run(&mut anon, &mut ext, 0);
        assert_eq!(report.created, 0);
        assert_eq!(report.too_big, 1);
        assert_eq!(report.unsupported, 1);
        assert_eq!(anon.image_calls, 0);
        assert_eq!(
            stand
                .registry()
                .by_orig("Тяжёлый Иванов Иван.docx")
                .unwrap()
                .unwrap()
                .status,
            Status::TooBig
        );
        assert!(!stand.workspace.join("Тяжёлый [PERSON_1].docx").exists());
    }

    #[test]
    fn colliding_names_get_counter_suffix() {
        let stand = Stand::new("collide");
        stand.write("Первый Иванов Иван.md", "один");
        stand.write("Второй Петров Пётр.md", "два");
        let mut anon = FakeAnon {
            collide: true,
            ..FakeAnon::default()
        };
        let mut ext = FakeExtractor::default();
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.created, 2, "{report:?}");
        assert!(stand.workspace.join("same.md").is_file());
        assert!(stand.workspace.join("same (2).md").is_file());
        let registry = stand.registry();
        let first = registry.by_orig("Первый Иванов Иван.md").unwrap().unwrap();
        let second = registry.by_orig("Второй Петров Пётр.md").unwrap().unwrap();
        assert_ne!(first.anon_rel, second.anon_rel);
    }

    #[test]
    fn error_on_one_file_does_not_stop_pass() {
        let stand = Stand::new("error");
        stand.write("Сбой Иванов Иван.docx", "текст");
        stand.write("Норма Петров Пётр.md", "текст");
        let mut anon = FakeAnon {
            fail_on: Some("Сбой Иванов Иван.docx".to_string()),
            ..FakeAnon::default()
        };
        let mut ext = FakeExtractor::default();
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.errors, 1, "{report:?}");
        assert_eq!(report.created, 1);
        let registry = stand.registry();
        assert_eq!(
            registry
                .by_orig("Сбой Иванов Иван.docx")
                .unwrap()
                .unwrap()
                .status,
            Status::Error
        );
        assert_eq!(
            registry
                .by_orig("Норма Петров Пётр.md")
                .unwrap()
                .unwrap()
                .status,
            Status::Synced
        );
    }

    #[test]
    fn pending_variant_turns_changed_original_into_conflict() {
        let stand = Stand::new("conflict");
        stand.write("Договор Иванов Иван.docx", "версия 1");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);

        // A4.3: у записи есть непринятый вариант, а пользователь изменил оригинал.
        let registry = stand.registry();
        let row = registry
            .by_orig("Договор Иванов Иван.docx")
            .unwrap()
            .unwrap();
        registry
            .set_status_by_orig(&row.orig_rel, Status::PendingAccept)
            .unwrap();
        drop(registry);
        stand.write("Договор Иванов Иван.docx", "версия 2 — пользователь");

        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.conflicts, 1, "{report:?}");
        assert_eq!(report.updated, 0, "образ не перезаписан");
        let row = stand
            .registry()
            .by_orig("Договор Иванов Иван.docx")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, Status::Conflict);
    }

    #[test]
    fn unsupported_row_is_removed_with_original() {
        let stand = Stand::new("unsup");
        stand.write("данные.bin", "binary");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).unsupported, 1);
        assert_eq!(stand.registry().count().unwrap(), 1);

        stand.remove("данные.bin");
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.removed, 1, "{report:?}");
        assert_eq!(stand.registry().count().unwrap(), 0);
    }

    // ── A4.3: обратный путь ─────────────────────────────────────────────

    #[test]
    fn agent_new_is_emitted_into_project_once() {
        let stand = Stand::new("new");
        stand.write_ws("Отчёт [PERSON_1].md", "[PERSON_1] пишет отчёт\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();

        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.emitted, 1, "{report:?}");
        assert_eq!(report.errors, 0, "{:?}", report.problems);
        let emitted = stand.project.join("Отчёт Иванов Иван.md");
        assert!(emitted.is_file(), "файл агента на своём месте");
        assert_eq!(
            fs::read_to_string(&emitted).unwrap(),
            "Иванов Иван пишет отчёт\n"
        );

        let registry = stand.registry();
        let row = registry
            .by_orig("Отчёт Иванов Иван.md")
            .unwrap()
            .expect("запись agent_new");
        assert_eq!(row.kind, Kind::AgentNew);
        assert_eq!(row.status, Status::Synced);
        assert_eq!(row.anon_rel, "Отчёт [PERSON_1].md");
        assert_eq!(row.orig_sha256, Some(sha256_file(&emitted).unwrap()));
        drop(registry);

        // Петля TR-7.7: повторный проход ничего не выпускает и не пересоздаёт.
        let second = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(second.changes(), 0, "{second:?}");
        assert_eq!(second.emitted, 0);
        assert!(!stand.project.join("Отчёт Иванов Иван (агент).md").exists());
    }

    #[test]
    fn agent_new_collision_gets_agent_suffix() {
        let stand = Stand::new("agent-suffix");
        // Имена заняты: оригинал и уже выпущенный вариант агента.
        stand.write("Смета Иванов Иван.md", "версия пользователя");
        stand.write("Смета Иванов Иван (агент).md", "уже есть вариант");
        // Новый файл агента занимает «базовое» имя образа, поэтому прямой
        // путь кладёт образ оригинала под «(2)», а выпуск получает «(агент 2)».
        stand.write_ws("Смета [PERSON_1].md", "[PERSON_1] прислал правку");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();

        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.emitted, 1, "{report:?}");
        assert_eq!(report.errors, 0, "{:?}", report.problems);
        let emitted = stand.project.join("Смета Иванов Иван (агент 2).md");
        assert!(emitted.is_file(), "третье имя — (агент 2)");
        assert_eq!(
            fs::read_to_string(&emitted).unwrap(),
            "Иванов Иван прислал правку"
        );
        assert_eq!(
            fs::read_to_string(stand.project.join("Смета Иванов Иван.md")).unwrap(),
            "версия пользователя",
            "существующий файл не перезаписан"
        );
    }

    #[test]
    fn agent_changed_makes_pending_variant_and_base_snapshot() {
        let stand = Stand::new("variant");
        stand.write("Письмо Иванов Иван.md", "версия 1\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);

        let original = stand.project.join("Письмо Иванов Иван.md");
        let original_sha = sha256_file(&original).unwrap();
        stand.write_ws("Письмо [PERSON_1].md", "правка агента\n");

        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.variants, 1, "{report:?}");
        assert_eq!(report.errors, 0, "{:?}", report.problems);

        // Оригинал не тронут (I6), снимок основания — его точная копия.
        assert_eq!(sha256_file(&original).unwrap(), original_sha);
        let base = stand
            .project
            .join(".hds-agent\\base\\Письмо Иванов Иван.md");
        assert_eq!(fs::read_to_string(&base).unwrap(), "версия 1\n");
        assert!(PathBuf::from(format!("{}.images.json", base.to_string_lossy())).is_file());

        let pending = stand
            .project
            .join(".hds-agent\\pending\\Письмо Иванов Иван.md");
        assert_eq!(
            fs::read_to_string(&pending).unwrap(),
            "правка агента\n",
            "вариант деанонимизирован"
        );
        let row = stand
            .registry()
            .by_orig("Письмо Иванов Иван.md")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, Status::PendingAccept);
        assert_eq!(
            row.anon_sha256_issued,
            Some(sha256_file(&stand.workspace.join("Письмо [PERSON_1].md")).unwrap())
        );

        let second = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(second.changes(), 0, "{second:?}");
    }

    #[test]
    fn agent_changed_with_user_edit_is_conflict_without_variant() {
        let stand = Stand::new("variant-conflict");
        stand.write("Договор Иванов Иван.md", "версия 1\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);

        // Пока агент правил образ, пользователь изменил оригинал.
        stand.write_ws("Договор [PERSON_1].md", "правка агента\n");
        stand.write("Договор Иванов Иван.md", "версия 2 — пользователь\n");

        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.conflicts, 1, "{report:?}");
        assert_eq!(report.variants, 0);
        assert!(!stand
            .project
            .join(".hds-agent\\pending\\Договор Иванов Иван.md")
            .exists());
        let row = stand
            .registry()
            .by_orig("Договор Иванов Иван.md")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, Status::Conflict);
    }

    #[test]
    fn deleted_image_keeps_original_and_marks_orphan() {
        let stand = Stand::new("image-deleted");
        stand.write("Письмо Иванов Иван.md", "текст\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);

        fs::remove_file(stand.workspace.join("Письмо [PERSON_1].md")).unwrap();
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.orphans, 1, "{report:?}");
        assert!(
            stand.project.join("Письмо Иванов Иван.md").is_file(),
            "оригинал не удаляется"
        );
        assert!(
            !stand.workspace.join("Письмо [PERSON_1].md").exists(),
            "образ не пересоздаётся"
        );
        let row = stand
            .registry()
            .by_orig("Письмо Иванов Иван.md")
            .unwrap()
            .unwrap();
        assert_eq!(row.status, Status::Orphan);
        // Повторный проход не плодит изменений.
        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
    }

    #[test]
    fn service_workspace_dirs_are_not_emitted() {
        let stand = Stand::new("service-ws");
        stand.write_ws("_attachments\\вложение [PERSON_1].md", "[PERSON_1]");
        stand.write_ws("_conflicts\\вариант [PERSON_1].md", "[PERSON_1]");
        stand.write_ws("_attachments\\tmp.tmp", "tmp");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.emitted, 0, "{report:?}");
        assert_eq!(report.changes(), 0);
        let names: Vec<String> = fs::read_dir(&stand.project)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.is_empty(), "в проекте ничего не появилось: {names:?}");
    }

    #[test]
    fn marked_file_of_known_artifact_is_not_emitted() {
        let stand = Stand::new("marked");
        stand.write("Акт Иванов Иван.md", "текст\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);
        let id = stand
            .registry()
            .by_orig("Акт Иванов Иван.md")
            .unwrap()
            .unwrap()
            .id;

        // Копия образа под другим именем: наш маркер, id существует.
        stand.write_ws(
            "Копия [PERSON_1].md",
            &format!("<!-- hds-agent/P0001/{id} -->\n[PERSON_1]\n"),
        );
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.emitted, 0, "{report:?}");
        assert!(!stand.project.join("Копия Иванов Иван.md").exists());
    }

    #[test]
    fn safe_components_rejects_escapes() {
        let bad = |part: &str| safe_components(&[part.to_string()]).is_none();
        assert!(bad(".."));
        assert!(bad("C:"));
        assert!(bad("a/b"));
        assert!(bad("a\\b"));
        assert!(bad("/abs"));
        let ok = safe_components(&["Протоколы".to_string(), "Отчёт Иванов Иван.md".to_string()])
            .expect("обычный путь");
        assert_eq!(ok.len(), 2);
    }

    #[test]
    fn repeated_agent_edit_rebuilds_pending_with_same_base() {
        let stand = Stand::new("variant-again");
        stand.write("ТЗ Иванов Иван.md", "версия 1\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);

        stand.write_ws("ТЗ [PERSON_1].md", "правка 1\n");
        assert_eq!(stand.run(&mut anon, &mut ext, 200).variants, 1);
        let base = stand.project.join(".hds-agent\\base\\ТЗ Иванов Иван.md");
        let base_sha = sha256_file(&base).unwrap();

        stand.write_ws("ТЗ [PERSON_1].md", "правка 2\n");
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.variants, 1, "{report:?}");
        let pending = stand.project.join(".hds-agent\\pending\\ТЗ Иванов Иван.md");
        assert_eq!(fs::read_to_string(&pending).unwrap(), "правка 2\n");
        assert_eq!(
            sha256_file(&base).unwrap(),
            base_sha,
            "основание не перезаписывается"
        );
        // Оригинал по-прежнему версия 1, статус — ждёт решения.
        assert_eq!(
            fs::read_to_string(stand.project.join("ТЗ Иванов Иван.md")).unwrap(),
            "версия 1\n"
        );
        assert_eq!(
            stand
                .registry()
                .by_orig("ТЗ Иванов Иван.md")
                .unwrap()
                .unwrap()
                .status,
            Status::PendingAccept
        );
    }
}
