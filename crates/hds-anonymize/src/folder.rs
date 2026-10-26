//! Пофайловый обход папки: только верхний уровень, форматы — из
//! `hello.formats` (§2.4/§3). Прогресс и ошибки — пофайлово.

use std::path::{Path, PathBuf};

/// Файлы верхнего уровня `dir`, подходящие под список форматов
/// (регистронезависимо, без точки: `docx`, `xml`, …). Сортировка — по имени
/// (стабильный порядок для серии актов). Папки не рекурсируются (решение
/// №8 плана: как `auto_transcribe.inbox_dir: recursive: false`).
pub fn list_files(dir: &Path, formats: &[String]) -> Vec<PathBuf> {
    let norm: Vec<String> = formats.iter().map(|f| f.trim().to_lowercase()).collect();
    let mut files: Vec<PathBuf> = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return files, // нет каталога → пустой список (ошибку отдаст акт)
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        if norm.contains(&ext) {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_only_and_sorted() {
        let tmp = std::env::temp_dir().join(format!("hds-anon-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        std::fs::write(tmp.join("b.txt"), "x").unwrap();
        std::fs::write(tmp.join("a.docx"), "x").unwrap();
        std::fs::write(tmp.join("c.MD"), "x").unwrap();
        std::fs::write(tmp.join("skip.exe"), "x").unwrap();
        std::fs::write(tmp.join("sub").join("d.txt"), "x").unwrap();
        let formats = vec!["txt".to_string(), "docx".to_string(), "md".to_string()];
        let got = list_files(&tmp, &formats);
        let names: Vec<String> = got
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into())
            .collect();
        assert_eq!(names, vec!["a.docx", "b.txt", "c.MD"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
