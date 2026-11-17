//! Доменные типы протокола анонимизации (§3 плана) и разбор ответов воркера.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Режим акта: анонимизация (`<stem>.anonymized.<ext>`), деанонимизация
/// (`<stem>.deanon.<ext>`) или образ анонимной зоны (`mode: "agent"`, имя
/// файла сохраняется — TR-7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Скрыть.
    Anonymize,
    /// Показать.
    Deanonymize,
    /// Образ анонимной зоны: метки проекта, имя файла без суффикса.
    Agent,
}

/// Выходной путь по схеме §3: Rust строит путь, sidecar только пишет.
/// У входа `a.anonymized.docx` суффикс `.anonymized` отбрасывается
/// (`a.anonymized.docx` → `a.deanon.docx`). В режиме [`Mode::Agent`] имя
/// файла сохраняется (имена — дело демона, A4).
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
        Mode::Agent => "",
    };
    out_dir.join(format!("{stem}{suffix}{ext}"))
}

/// Синтаксис метки (TR-1.4) — единый для Python и Rust:
/// `\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]`, то есть `[PERSON_3]` или
/// `[PERSON_3|дат]` / `[PERSON_3|род,мн]`.
pub const LABEL_PATTERN: &str = r"\[([A-Z_]+)_(\d+)(?:\|([а-яё,]+))?\]";

/// Разобрать метку: `"[PERSON_3|дат]"` → `("PERSON", 3, Some("дат"))`.
///
/// Ручной разбор (без `regex`): формат фиксированный, а метки приходят в
/// событиях агента часто — разбор должен быть дешёвым и без паники.
pub fn label_parts(token: &str) -> Option<(String, u32, Option<String>)> {
    let inner = token.strip_prefix('[')?.strip_suffix(']')?;
    let (body, suffix) = match inner.split_once('|') {
        Some((body, suffix)) if !suffix.is_empty() => (body, Some(suffix.to_string())),
        Some(_) => return None,
        None => (inner, None),
    };
    let (kind, number) = body.rsplit_once('_')?;
    if kind.is_empty() || !kind.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return None;
    }
    let number: u32 = number.parse().ok()?;
    if let Some(suffix) = &suffix {
        // суффикс — русские строчные буквы и запятая: «дат», «род,мн»
        if !suffix
            .chars()
            .all(|c| (c.is_alphabetic() && !c.is_ascii()) || c == ',')
        {
            return None;
        }
    }
    Some((kind.to_string(), number, suffix))
}

/// Является ли строка меткой проекта (TR-1.4).
pub fn is_label(token: &str) -> bool {
    label_parts(token).is_some()
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
                    token: t
                        .get("token")
                        .or_else(|| t.get("label"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .into(),
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

/// Ответ `anonymize` / `anonymize_values` (§3, TR-7.4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnonymizeResult {
    pub out_path: String,
    pub tokens: Vec<TokenCount>,
    /// Найденные в файле значения из `values` (только в `anonymize_values`).
    pub matched: Vec<String>,
    pub unmatched: Vec<String>,
    pub warnings: Vec<String>,
    pub elapsed_ms: f64,
    /// `classic` или `agent` (TR-7.4).
    pub mode: String,
    /// Сколько картинок тела заменено метками `[IMAGE_n]` (режим agent).
    pub images: i64,
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
        mode: opt_str(v, "mode").unwrap_or_else(|| "classic".to_string()),
        images: v.get("images").and_then(|i| i.as_i64()).unwrap_or(0),
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

/// Форма значения в снимке меток (`vault_snapshot`, TR-1.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotForm {
    pub value: String,
    pub gram: Option<String>,
    pub origin: String,
}

/// Метка проекта в снимке. Значения форм Rust держит **только в памяти**
/// (исходные значения в журналы не пишутся, TR-1.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotLabel {
    pub label: String,
    pub entity_type: String,
    pub forms: Vec<SnapshotForm>,
}

/// Снимок меток проекта: `{rev, labels, others, keep}` (TR-1.6).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub rev: i64,
    pub labels: Vec<SnapshotLabel>,
    pub others: Vec<String>,
    pub keep: Vec<String>,
}

/// Ответ `anonymize_text` (TR-1.7): текст с метками и список меток.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextResult {
    pub text: String,
    pub labels: Vec<TokenCount>,
    pub project_id: String,
    pub warnings: Vec<String>,
}

/// Ответ `label_for_value` («Скрыть ещё…», TR-1.7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelForValue {
    pub label: Option<String>,
    pub value_type: String,
    pub kept: bool,
    pub confident: bool,
}

/// Ответ `text_image` (D18): путь образа, маркер, метки.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextImageResult {
    pub out_path: String,
    pub marker: String,
    pub labels: Vec<TokenCount>,
    pub warnings: Vec<String>,
}

/// Ответ `office_read`/`office_edit`/`office_create` (TR-3.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OfficeResult {
    pub kind: String,
    pub markdown: String,
    pub applied: i64,
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

pub fn snapshot_from(v: &Value) -> Snapshot {
    let labels = v
        .get("labels")
        .and_then(|x| x.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| SnapshotLabel {
                    label: opt_str(item, "label").unwrap_or_default(),
                    entity_type: opt_str(item, "type").unwrap_or_default(),
                    forms: item
                        .get("forms")
                        .and_then(|f| f.as_array())
                        .map(|forms| {
                            forms
                                .iter()
                                .map(|form| SnapshotForm {
                                    value: opt_str(form, "value").unwrap_or_default(),
                                    gram: opt_str(form, "gram"),
                                    origin: opt_str(form, "origin").unwrap_or_default(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    Snapshot {
        rev: v.get("rev").and_then(|r| r.as_i64()).unwrap_or(0),
        labels,
        others: str_list(v, "others"),
        keep: str_list(v, "keep"),
    }
}

pub fn text_from(v: &Value) -> TextResult {
    TextResult {
        text: opt_str(v, "text").unwrap_or_default(),
        labels: token_counts(v, "labels"),
        project_id: opt_str(v, "project_id").unwrap_or_default(),
        warnings: str_list(v, "warnings"),
    }
}

pub fn label_from(v: &Value) -> LabelForValue {
    LabelForValue {
        label: opt_str(v, "label"),
        value_type: opt_str(v, "value_type").unwrap_or_default(),
        kept: v.get("kept").and_then(|k| k.as_bool()).unwrap_or(false),
        confident: v
            .get("confident")
            .and_then(|c| c.as_bool())
            .unwrap_or(false),
    }
}

pub fn image_from(v: &Value) -> TextImageResult {
    TextImageResult {
        out_path: opt_str(v, "out_path").unwrap_or_default(),
        marker: opt_str(v, "marker").unwrap_or_default(),
        labels: token_counts(v, "labels"),
        warnings: str_list(v, "warnings"),
    }
}

pub fn office_from(v: &Value) -> OfficeResult {
    OfficeResult {
        kind: opt_str(v, "kind").unwrap_or_default(),
        markdown: opt_str(v, "markdown").unwrap_or_default(),
        applied: v
            .get("applied")
            .and_then(|a| a.as_array())
            .map(|a| a.len() as i64)
            .unwrap_or(0),
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
    fn out_path_agent_keeps_name() {
        let dir = Path::new("D:/ws");
        assert_eq!(
            out_path(Path::new("D:/проект/ТЗ.docx"), dir, Mode::Agent),
            dir.join("ТЗ.docx")
        );
    }

    #[test]
    fn label_parts_parse_tr14() {
        assert_eq!(
            label_parts("[PERSON_3]"),
            Some(("PERSON".to_string(), 3, None))
        );
        assert_eq!(
            label_parts("[PERSON_3|дат]"),
            Some(("PERSON".to_string(), 3, Some("дат".to_string())))
        );
        assert_eq!(
            label_parts("[PERSON_3|род,мн]"),
            Some(("PERSON".to_string(), 3, Some("род,мн".to_string())))
        );
        // терпимый разбор — только на стороне Python (TR-2.4); здесь строго
        assert_eq!(label_parts("[person_3]"), None);
        assert_eq!(label_parts("[PERSON 3]"), None);
        assert_eq!(label_parts("[PERSON_3|]"), None);
        assert_eq!(label_parts("PERSON_3"), None);
        assert_eq!(label_parts("[PERSON_x]"), None);
        assert!(is_label("[ORG_21|твор]"));
        // [IMAGE_n] синтаксически метка (TR-1.4), но тип IMAGE — фильтру
        // исходящего его надо пропускать: это не сущность, а метка картинки
        assert!(is_label("[IMAGE_1]"));
        assert_eq!(label_parts("[IMAGE_1]").unwrap().0, "IMAGE");
    }

    #[test]
    fn snapshot_from_parses_forms() {
        let value: Value = serde_json::from_str(
            r#"{"rev": 5, "labels": [{"label": "[PERSON_1]", "type": "PERSON",
                 "forms": [{"value": "Иванов Иван", "gram": "nomn", "origin": "seen"},
                           {"value": "Иванову Ивану", "gram": null, "origin": "derived"}]}],
                "others": ["Петров Пётр"], "keep": ["ТЗ"]}"#,
        )
        .unwrap();
        let snapshot = snapshot_from(&value);
        assert_eq!(snapshot.rev, 5);
        assert_eq!(snapshot.labels[0].label, "[PERSON_1]");
        assert_eq!(snapshot.labels[0].forms.len(), 2);
        assert_eq!(snapshot.labels[0].forms[0].gram.as_deref(), Some("nomn"));
        assert_eq!(snapshot.labels[0].forms[1].gram, None);
        assert_eq!(snapshot.others, vec!["Петров Пётр".to_string()]);
        assert_eq!(snapshot.keep, vec!["ТЗ".to_string()]);
    }

    #[test]
    fn text_and_image_from_parse_labels_key() {
        let value: Value = serde_json::from_str(
            r#"{"text": "[PERSON_1] написал", "project_id": "P0001",
                "labels": [{"label": "[PERSON_1]", "type": "PERSON", "count": 2}],
                "warnings": ["проверить"]}"#,
        )
        .unwrap();
        let text = text_from(&value);
        assert_eq!(text.project_id, "P0001");
        assert_eq!(text.labels[0].token, "[PERSON_1]");
        assert_eq!(text.labels[0].count, 2);
        assert_eq!(text.warnings.len(), 1);

        let image: Value = serde_json::from_str(
            r#"{"out_path": "D:/ws/ТЗ.pdf.md",
                "marker": "<!-- hds-agent/P0001/42 -->",
                "labels": [{"label": "[ORG_1]", "type": "ORG", "count": 1}],
                "warnings": []}"#,
        )
        .unwrap();
        let parsed = image_from(&image);
        assert!(parsed.out_path.ends_with("ТЗ.pdf.md"));
        assert_eq!(parsed.marker, "<!-- hds-agent/P0001/42 -->");
        assert_eq!(parsed.labels[0].token, "[ORG_1]");
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
