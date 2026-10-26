//! Доменные типы протокола анонимизации (§3 плана) и разбор ответов воркера.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Режим акта: анонимизация (`<stem>.anonymized.<ext>`) или деанонимизация
/// (`<stem>.deanon.<ext>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Скрыть.
    Anonymize,
    /// Показать.
    Deanonymize,
}

/// Выходной путь по схеме §3: Rust строит путь, sidecar только пишет.
/// У входа `a.anonymized.docx` суффикс `.anonymized` отбрасывается
/// (`a.anonymized.docx` → `a.deanon.docx`).
pub fn out_path(in_path: &Path, out_dir: &Path, mode: Mode) -> PathBuf {
    let stem = in_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let stem = stem.strip_suffix(".anonymized").unwrap_or(stem);
    let ext = in_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let suffix = match mode {
        Mode::Anonymize => ".anonymized",
        Mode::Deanonymize => ".deanon",
    };
    out_dir.join(format!("{stem}{suffix}{ext}"))
}

/// Ответ `hello` воркера (форматы — для пофайлового обхода папки, §3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HelloInfo {
    pub protocol: i64,
    pub python: String,
    pub pid: u32,
    pub backend: String,
    pub providers: Vec<String>,
    pub model: String,
    pub formats: Vec<String>,
    pub capabilities: Vec<String>,
}

impl HelloInfo {
    pub fn has(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn hello_from(v: &Value) -> HelloInfo {
    HelloInfo {
        protocol: v.get("protocol").and_then(|p| p.as_i64()).unwrap_or(0),
        python: v
            .get("python")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        pid: v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32,
        backend: v
            .get("backend")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
        providers: strings(v, "providers"),
        model: v
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string(),
        formats: strings(v, "formats"),
        capabilities: strings(v, "capabilities"),
    }
}

/// Токен и число замен в акте (значения PII наружу не отдаются).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCount {
    pub token: String,
    pub entity_type: String,
    pub count: i64,
}

fn token_counts(v: &Value, key: &str) -> Vec<TokenCount> {
    v.get(key)
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .map(|t| TokenCount {
                    token: t.get("token").and_then(|x| x.as_str()).unwrap_or("").into(),
                    entity_type: t.get("type").and_then(|x| x.as_str()).unwrap_or("").into(),
                    count: t.get("count").and_then(|c| c.as_i64()).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Ответ `warmup`: чем загрузился воркер.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WarmupResult {
    pub backend: String,
    pub providers: Vec<String>,
    pub load_ms: f64,
}

pub fn warmup_from(v: &Value) -> WarmupResult {
    WarmupResult {
        backend: v
            .get("backend")
            .and_then(|b| b.as_str())
            .unwrap_or_default()
            .to_string(),
        providers: strings(v, "providers"),
        load_ms: v.get("load_ms").and_then(|l| l.as_f64()).unwrap_or(0.0),
    }
}

/// Ответ `anonymize` / `anonymize_values` (§3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnonymizeResult {
    pub out_path: String,
    pub tokens: Vec<TokenCount>,
    /// Найденные в файле значения из `values` (только в `anonymize_values`).
    pub matched: Vec<String>,
    pub unmatched: Vec<String>,
    pub warnings: Vec<String>,
    pub elapsed_ms: f64,
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    strings(v, key)
}

pub fn anonymize_from(v: &Value) -> AnonymizeResult {
    AnonymizeResult {
        out_path: v
            .get("out_path")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        tokens: token_counts(v, "tokens"),
        matched: str_list(v, "matched"),
        unmatched: str_list(v, "unmatched"),
        warnings: str_list(v, "warnings"),
        elapsed_ms: v.get("elapsed_ms").and_then(|e| e.as_f64()).unwrap_or(0.0),
    }
}

/// Ответ `deanonymize` / `deanonymize_values` (§3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeanonymizeResult {
    pub out_path: String,
    /// Токены, реально восстановленные в файле.
    pub restored: Vec<String>,
    pub unmatched: Vec<String>,
    pub warnings: Vec<String>,
    pub elapsed_ms: f64,
}

pub fn deanonymize_from(v: &Value) -> DeanonymizeResult {
    DeanonymizeResult {
        out_path: v
            .get("out_path")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string(),
        restored: str_list(v, "restored"),
        unmatched: str_list(v, "unmatched"),
        warnings: str_list(v, "warnings"),
        elapsed_ms: v.get("elapsed_ms").and_then(|e| e.as_f64()).unwrap_or(0.0),
    }
}

/// Плейсхолдер из `inspect` (тип/счётчик — БЕЗ значений, §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub token: String,
    pub entity_type: String,
    pub count: i64,
}

/// Есть ли токен в БД (`inspect.resolved`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub token: String,
    pub in_db: bool,
}

/// Ответ `inspect`: разведка файла БЕЗ значений PII.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InspectResult {
    pub supported: bool,
    pub marker_version: Option<String>,
    pub placeholders: Vec<Placeholder>,
    pub resolved: Vec<Resolved>,
}

pub fn inspect_from(v: &Value) -> InspectResult {
    let placeholders = v
        .get("placeholders")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .map(|p| Placeholder {
                    token: p.get("token").and_then(|x| x.as_str()).unwrap_or("").into(),
                    entity_type: p.get("type").and_then(|x| x.as_str()).unwrap_or("").into(),
                    count: p.get("count").and_then(|c| c.as_i64()).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    let resolved = v
        .get("resolved")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .map(|r| Resolved {
                    token: r.get("token").and_then(|x| x.as_str()).unwrap_or("").into(),
                    in_db: r.get("in_db").and_then(|x| x.as_bool()).unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    InspectResult {
        supported: v
            .get("supported")
            .and_then(|s| s.as_bool())
            .unwrap_or(false),
        marker_version: v
            .get("marker_version")
            .and_then(|m| m.as_str())
            .map(|s| s.to_string()),
        placeholders,
        resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_path_schemes() {
        let dir = Path::new("D:/out");
        assert_eq!(
            out_path(Path::new("D:/in/договор.docx"), dir, Mode::Anonymize),
            dir.join("договор.anonymized.docx")
        );
        assert_eq!(
            out_path(Path::new("D:/in/договор.docx"), dir, Mode::Deanonymize),
            dir.join("договор.deanon.docx")
        );
        // суффикс .anonymized у входа отбрасывается: a.anonymized.docx → a.deanon.docx
        assert_eq!(
            out_path(Path::new("D:/in/a.anonymized.docx"), dir, Mode::Deanonymize),
            dir.join("a.deanon.docx")
        );
    }

    #[test]
    fn is_notification_rejects_replies() {
        assert!(hds_extract::protocol::is_notification(
            r#"{"jsonrpc":"2.0","method":"progress","params":{}}"#
        ));
        assert!(!hds_extract::protocol::is_notification(
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#
        ));
        assert!(!hds_extract::protocol::is_notification("not json"));
    }
}
