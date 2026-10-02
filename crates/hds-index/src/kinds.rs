//! Таблицы расширений → вид файла: дословный порт
//! * `hds/extractors.py` (`TEXT_EXTS`/`PDF_EXTS`/`DOCX_EXTS`/`XLSX_EXTS`/`PPTX_EXTS`,
//!   `kind_for_ext`),
//! * `hds/extract_static.py` (`MPP_EXTS`/`IMAGE_EXTS`, `kind_for_ext_media`),
//! * `hds/extract_av.py` (`AUDIO_EXTS`/`VIDEO_EXTS`, `kind_for_ext_media`),
//! * `hds/indexer.py:_kind_of` (порядок: базовые виды → статика → медиа).
//!
//! Вид нужен обходу для лимита размера (`index.max_file_mb` против
//! `index.max_media_mb`, `_limit_mb` в Python) и конвейеру — для выбора
//! извлекателя. Сами извлекатели переезжают в sidecar (B6), таблицы — здесь.

use std::path::Path;

/// Вид файла: строка, совпадающая с `kind` в Python-версии (`files.kind` в БД).
pub type Kind = &'static str;

pub const PDF_EXTS: &[&str] = &[".pdf"];
pub const DOCX_EXTS: &[&str] = &[".docx"];
pub const XLSX_EXTS: &[&str] = &[".xlsx", ".xlsm"];
pub const PPTX_EXTS: &[&str] = &[".pptx"];

pub const TEXT_EXTS: &[&str] = &[
    ".txt",
    ".md",
    ".markdown",
    ".log",
    ".csv",
    ".tsv",
    ".json",
    ".jsonl",
    ".xml",
    ".yaml",
    ".yml",
    ".ini",
    ".cfg",
    ".conf",
    ".env",
    ".bat",
    ".cmd",
    ".ps1",
    ".py",
    ".pyw",
    ".js",
    ".mjs",
    ".ts",
    ".java",
    ".cs",
    ".cpp",
    ".c",
    ".h",
    ".hpp",
    ".sql",
    ".html",
    ".htm",
    ".css",
    ".php",
    ".rb",
    ".go",
    ".rs",
    ".sh",
    ".vbs",
    ".reg",
    ".url",
    ".srt",
    ".ass",
    ".tex",
    ".rst",
    ".rtf",
];

pub const MPP_EXTS: &[&str] = &[".mpp", ".mpt", ".mpx"];
pub const IMAGE_EXTS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff", ".webp", ".gif",
];
pub const AUDIO_EXTS: &[&str] = &[
    ".mp3", ".wav", ".m4a", ".flac", ".ogg", ".wma", ".aac", ".opus",
];
pub const VIDEO_EXTS: &[&str] = &[
    ".mp4", ".avi", ".mkv", ".mov", ".wmv", ".flv", ".webm", ".mpg", ".mpeg", ".3gp", ".mts",
];

/// `MEDIA_KINDS` из `hds/indexer.py` — виды, к которым применяется
/// `index.max_media_mb` (а не `max_file_mb`).
pub const MEDIA_KINDS: &[&str] = &["media"];

/// Расширение как `os.path.splitext(path)[1].lower()`.
///
/// Точная семантика CPython: берётся последняя точка в имени файла, но
/// **ведущие точки расширением не считаются** (`".bashrc"` и `"..pdf"` дают
/// пустое расширение, `"a..pdf"` — `".pdf"`). Расхождение здесь стоило паритета
/// на реальном файле `D:\…\объявления\..pdf` — см. `tools/parity/W2_REPORT.md`.
pub fn ext_of(path: &Path) -> String {
    let name = match path.file_name() {
        Some(n) => n.to_string_lossy(),
        None => return String::new(),
    };
    if let Some(dot) = name.rfind('.') {
        if name[..dot].chars().any(|c| c != '.') {
            return name[dot..].to_lowercase();
        }
    }
    String::new()
}

/// `hds/extractors.py:kind_for_ext` — только «пакетные» форматы.
pub fn kind_for_ext(ext: &str) -> Option<Kind> {
    if PDF_EXTS.contains(&ext) {
        return Some("pdf");
    }
    if DOCX_EXTS.contains(&ext) {
        return Some("docx");
    }
    if XLSX_EXTS.contains(&ext) {
        return Some("xlsx");
    }
    if PPTX_EXTS.contains(&ext) {
        return Some("pptx");
    }
    if TEXT_EXTS.contains(&ext) {
        return Some("text");
    }
    None
}

/// `hds/extract_static.py:kind_for_ext_media` — MS Project и картинки.
pub fn kind_for_ext_static(ext: &str) -> Option<Kind> {
    if MPP_EXTS.contains(&ext) {
        return Some("mpp");
    }
    if IMAGE_EXTS.contains(&ext) {
        return Some("image");
    }
    None
}

/// `hds/extract_av.py:kind_for_ext_media` — аудио/видео.
pub fn kind_for_ext_av(ext: &str) -> Option<Kind> {
    if AUDIO_EXTS.contains(&ext) || VIDEO_EXTS.contains(&ext) {
        return Some("media");
    }
    None
}

/// `hds/indexer.py:_kind_of` — порядок проверок сохранён (base → static → av).
pub fn kind_of(ext: &str) -> Option<Kind> {
    kind_for_ext(ext)
        .or_else(|| kind_for_ext_static(ext))
        .or_else(|| kind_for_ext_av(ext))
}

/// Вид файла по пути (расширение выделяется как в Python).
pub fn kind_of_path(path: &Path) -> Option<Kind> {
    kind_of(&ext_of(path))
}

/// True для видов, к которым применяется лимит медиа.
pub fn is_media(kind: Kind) -> bool {
    MEDIA_KINDS.contains(&kind)
}

/// Служебные lock-файлы Office (`~$док.xlsx`): вечно меняются, `openpyxl` на них
/// падает `BadZipFile` — в Python пропускаются как нетиповые (`_extract_file`).
pub fn is_office_lock_file(path: &Path) -> bool {
    match path.file_name() {
        Some(n) => n.to_string_lossy().starts_with("~$"),
        None => false,
    }
}
