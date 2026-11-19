//! Деанонимизация событий агента (TR-5.3).
//!
//! Поток событий идёт от модели к пользователю: метки `[PERSON_3]` /
//! `[PERSON_3|дат]` заменяются исходными формами по снимку vault (форма
//! выбирается по падежному суффиксу, TR-2.4), пути `<P>\workspace\…` — путями
//! папки проекта. Неизвестная метка остаётся как есть и попадает в список
//! предупреждений — пользователь видит, что подстановка не удалась.
//!
//! Разбор меток — только через `hds_core::labels` (второй регулярки быть не
//! должно, TR-1.4).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use hds_anonymize::protocol::{Snapshot, SnapshotForm};
use hds_core::labels::{find_labels, gram_to_suffix, is_label, label_parts};

/// Результат деанонимизации строки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeAnonText {
    pub text: String,
    /// Метки, для которых не нашлось значения (показываются как есть).
    pub unknown: Vec<String>,
}

/// Деанонимизатор одного проекта.
pub struct DeAnonymizer {
    labels: HashMap<String, Vec<SnapshotForm>>,
    system: Vec<String>,
    workspace: PathBuf,
    project: PathBuf,
}

impl DeAnonymizer {
    /// Собрать по снимку vault и путям проекта.
    pub fn new(snapshot: &Snapshot, system: &[String], workspace: &Path, project: &Path) -> Self {
        let mut labels: HashMap<String, Vec<SnapshotForm>> = HashMap::new();
        for label in &snapshot.labels {
            labels.insert(label.label.clone(), label.forms.clone());
        }
        DeAnonymizer {
            labels,
            system: system.to_vec(),
            workspace: workspace.to_path_buf(),
            project: project.to_path_buf(),
        }
    }

    /// Развернуть метки и пути в строке.
    pub fn text(&self, text: &str) -> DeAnonText {
        let mut unknown = Vec::new();
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            let after = &rest[open..];
            let Some(close) = after.find(']') else {
                out.push_str(after);
                return DeAnonText {
                    text: self.replace_paths(&out),
                    unknown,
                };
            };
            let token = &after[..=close];
            if is_label(token) {
                match self.resolve(token) {
                    Some(value) => out.push_str(&value),
                    None => {
                        unknown.push(token.to_string());
                        out.push_str(token);
                    }
                }
            } else {
                out.push_str(token);
            }
            rest = &after[close + 1..];
        }
        out.push_str(rest);
        DeAnonText {
            text: self.replace_paths(&out),
            unknown,
        }
    }

    /// Развернуть строки в JSON-дереве (события, аргументы инструментов).
    pub fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text).text),
            Value::Array(items) => Value::Array(items.iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), self.value(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn resolve(&self, token: &str) -> Option<String> {
        let (kind, number, suffix) = label_parts(token)?;
        if kind == "SYS" {
            let index = number as usize;
            return self.system.get(index.saturating_sub(1)).cloned();
        }
        let base = format!("[{kind}_{number}]");
        let forms = self.labels.get(&base)?;
        if forms.is_empty() {
            return None;
        }
        // `forms.gram` — код pymorphy (`datv`), суффикс метки — русский
        // (`дат`); сравниваем в одном пространстве (TR-2.4).
        let wanted = suffix.as_deref().map(gram_to_suffix).unwrap_or_default();
        if let Some(form) = forms.iter().find(|f| {
            let gram = f.gram.as_deref().unwrap_or("");
            gram_to_suffix(gram) == wanted
        }) {
            return Some(form.value.clone());
        }
        // Без падежа — именительная форма (TR-2.4), иначе первая известная.
        forms
            .iter()
            .find(|f| gram_to_suffix(f.gram.as_deref().unwrap_or("")).is_empty())
            .or_else(|| forms.first())
            .map(|f| f.value.clone())
    }

    /// Есть ли значение для метки (нужно шлюзу: список неразрешённых меток).
    pub fn resolves(&self, token: &str) -> bool {
        self.resolve(token).is_some()
    }

    /// Метки в JSON-дереве, для которых нет значения (в порядке сортировки).
    ///
    /// В отличие от [`hds_core::labels::find_labels`] отдаёт только те, что
    /// деанонимизатор не смог подставить: UI показывает их пользователю как
    /// «не удалось восстановить».
    pub fn unknown_labels(&self, value: &Value) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        self.collect_unknown(value, &mut out);
        out.sort();
        out.dedup();
        out
    }

    fn collect_unknown(&self, value: &Value, out: &mut Vec<String>) {
        match value {
            Value::String(text) => {
                for label in find_labels(text) {
                    if !self.resolves(&label) && !out.iter().any(|x| x == &label) {
                        out.push(label);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.collect_unknown(item, out);
                }
            }
            Value::Object(map) => {
                for item in map.values() {
                    self.collect_unknown(item, out);
                }
            }
            _ => {}
        }
    }

    /// Путь образа → путь папки проекта (TR-5.3).
    fn replace_paths(&self, text: &str) -> String {
        let ws = self.workspace.to_string_lossy().to_string();
        let ws_fwd = ws.replace('\\', "/");
        let proj = self.project.to_string_lossy().to_string();
        let mut out = text.replace(&ws, &proj);
        if ws_fwd != ws {
            out = out.replace(&ws_fwd, &proj.replace('\\', "/"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hds_anonymize::protocol::SnapshotLabel;

    fn snapshot() -> Snapshot {
        Snapshot {
            rev: 1,
            labels: vec![SnapshotLabel {
                label: "[PERSON_1]".to_string(),
                entity_type: "PERSON".to_string(),
                forms: vec![
                    SnapshotForm {
                        value: "Иванов Иван".to_string(),
                        gram: None,
                        origin: "ner".to_string(),
                    },
                    SnapshotForm {
                        value: "Иванову Ивану".to_string(),
                        gram: Some("дат".to_string()),
                        origin: "ner".to_string(),
                    },
                ],
            }],
            others: vec![],
            keep: vec![],
        }
    }

    fn deanon() -> DeAnonymizer {
        DeAnonymizer::new(
            &snapshot(),
            &["DESKTOP-ABC".to_string()],
            Path::new("C:\\anon\\P1\\workspace"),
            Path::new("C:\\Проект"),
        )
    }

    #[test]
    fn labels_become_values_with_case_suffix() {
        let d = deanon();
        let out = d.text("Передал [PERSON_1] и [PERSON_1|дат].");
        assert_eq!(out.text, "Передал Иванов Иван и Иванову Ивану.");
        assert!(out.unknown.is_empty());
    }

    #[test]
    fn unknown_label_is_kept_and_reported() {
        let d = deanon();
        let out = d.text("Метка [ORG_9] без значения");
        assert_eq!(out.text, "Метка [ORG_9] без значения");
        assert_eq!(out.unknown, vec!["[ORG_9]".to_string()]);
    }

    /// Реальный снимок vault: `forms.gram` — код pymorphy (`nomn`/`datv`),
    /// а метка — с русским суффиксом. Падеж обязан восстанавливаться.
    #[test]
    fn pymorphy_gram_and_russian_suffix_match() {
        let snapshot = Snapshot {
            rev: 2,
            labels: vec![SnapshotLabel {
                label: "[PERSON_1]".to_string(),
                entity_type: "PERSON".to_string(),
                forms: vec![
                    SnapshotForm {
                        value: "Иванов Иван".to_string(),
                        gram: Some("nomn".to_string()),
                        origin: "seen".to_string(),
                    },
                    SnapshotForm {
                        value: "Иванову Ивану".to_string(),
                        gram: Some("datv".to_string()),
                        origin: "derived".to_string(),
                    },
                ],
            }],
            others: vec![],
            keep: vec![],
        };
        let d = DeAnonymizer::new(
            &snapshot,
            &[],
            Path::new("C:\\anon\\P1\\workspace"),
            Path::new("C:\\Проект"),
        );
        assert_eq!(d.text("[PERSON_1]").text, "Иванов Иван");
        assert_eq!(d.text("[PERSON_1|дат]").text, "Иванову Ивану");
        // Терпимый разбор уже выпущенных меток A3.4 с латинским кодом.
        assert_eq!(d.text("[PERSON_1|datv]").text, "Иванову Ивану");
        assert_eq!(d.text("[PERSON_1|nomn]").text, "Иванов Иван");
    }

    /// Без суффикса — именительная форма, даже если первой встретилась
    /// косвенная (TR-2.4).
    #[test]
    fn bare_label_prefers_nominative() {
        let snapshot = Snapshot {
            rev: 3,
            labels: vec![SnapshotLabel {
                label: "[PERSON_1]".to_string(),
                entity_type: "PERSON".to_string(),
                forms: vec![
                    SnapshotForm {
                        value: "Иванову Ивану".to_string(),
                        gram: Some("datv".to_string()),
                        origin: "seen".to_string(),
                    },
                    SnapshotForm {
                        value: "Иванов Иван".to_string(),
                        gram: Some("nomn".to_string()),
                        origin: "derived".to_string(),
                    },
                ],
            }],
            others: vec![],
            keep: vec![],
        };
        let d = DeAnonymizer::new(
            &snapshot,
            &[],
            Path::new("C:\\anon\\P1\\workspace"),
            Path::new("C:\\Проект"),
        );
        assert_eq!(d.text("[PERSON_1]").text, "Иванов Иван");
    }

    #[test]
    fn unknown_labels_lists_only_unresolved() {
        let d = deanon();
        let value = serde_json::json!({
            "text": "[PERSON_1] и [ORG_9]",
            "nested": ["[SYS_1]", "[PERSON_2]"]
        });
        assert_eq!(
            d.unknown_labels(&value),
            vec!["[ORG_9]".to_string(), "[PERSON_2]".to_string()]
        );
    }

    #[test]
    fn system_label_is_restored() {
        let d = deanon();
        assert_eq!(d.text("Хост [SYS_1]").text, "Хост DESKTOP-ABC");
    }

    #[test]
    fn workspace_paths_become_project_paths() {
        let d = deanon();
        let out = d.text("Файл C:\\anon\\P1\\workspace\\Отчёт.docx готов");
        assert_eq!(out.text, "Файл C:\\Проект\\Отчёт.docx готов");
        let fwd = d.text("Файл C:/anon/P1/workspace/Отчёт.docx готов");
        assert_eq!(fwd.text, "Файл C:/Проект/Отчёт.docx готов");
    }

    #[test]
    fn json_tree_is_walked() {
        let d = deanon();
        let value = serde_json::json!({
            "text": "[PERSON_1]",
            "nested": ["[SYS_1]", {"path": "C:\\anon\\P1\\workspace\\a.txt"}]
        });
        let out = d.value(&value);
        assert_eq!(out["text"], "Иванов Иван");
        assert_eq!(out["nested"][0], "DESKTOP-ABC");
        assert_eq!(out["nested"][1]["path"], "C:\\Проект\\a.txt");
    }
}
