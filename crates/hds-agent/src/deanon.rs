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
    /// Относительные имена образов → пути оригиналов в папке проекта (AG-26).
    relative: Vec<RelativePath>,
}

/// Пара «имя образа (как в реестре) → путь оригинала» (AG-26).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RelativePath {
    from: String,
    to: String,
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
            relative: Vec::new(),
        }
    }

    /// Добавить карту относительных путей образа (AG-26).
    ///
    /// В ленте результат `disk-search` и вызовы инструментов приходят
    /// относительными именами образов (`Протоколы\\Встреча с [PERSON_3].docx`),
    /// а не абсолютным путём workspace: без этой карты пользователь видел путь
    /// зоны и не мог открыть файл. Метки внутри имени разворачиваются вместе с
    /// остальным текстом — поэтому имя маскируется **до** разворота меток.
    pub fn with_relative_paths(
        mut self,
        pairs: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut relative: Vec<RelativePath> = Vec::new();
        for (from, to) in pairs {
            if from.trim().is_empty() {
                continue;
            }
            for variant in separator_variants(&from) {
                if !relative.iter().any(|pair| pair.from == variant) {
                    relative.push(RelativePath {
                        from: variant,
                        to: to.clone(),
                    });
                }
            }
        }
        // Длинные имена — первыми: `dir\\file` не должно быть съедено `file`.
        relative.sort_by_key(|pair| std::cmp::Reverse(pair.from.chars().count()));
        self.relative = relative;
        self
    }

    /// Развернуть метки и пути в строке.
    pub fn text(&self, text: &str) -> DeAnonText {
        // AG-26: сначала абсолютный путь зоны → папка проекта, затем маскируем
        // относительные имена образов (в них бывают метки), разворачиваем метки
        // и возвращаем пути оригиналов.
        let prepared = self.replace_abs_paths(text);
        let (masked, targets) = self.mask_relative(&prepared);
        let mut unknown = Vec::new();
        let mut out = String::with_capacity(masked.len());
        let mut rest = masked.as_str();
        while let Some(open) = rest.find('[') {
            out.push_str(&rest[..open]);
            let after = &rest[open..];
            let Some(close) = after.find(']') else {
                out.push_str(after);
                return DeAnonText {
                    text: self.restore_relative(&out, &targets),
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
            text: self.restore_relative(&out, &targets),
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
    /// Пути `<P>\workspace\…` → пути папки проекта.
    ///
    /// Путь из конфига может быть записан и через `/`, и через `\`, а модель
    /// присылает его так, как видит ОС. Поэтому пробуем оба варианта: иначе
    /// (A5.3, стенд) путь `agent.root: 'C:/…/zone'` не совпадал с текстом
    /// `C:\…\zone\P0001\workspace\…` и в ленте оставался путь зоны.
    fn replace_abs_paths(&self, text: &str) -> String {
        let ws = self.workspace.to_string_lossy().to_string();
        let proj = self.project.to_string_lossy().to_string();
        let mut out = text.to_string();
        let mut seen: Vec<String> = Vec::new();
        for (from, to) in [
            (ws.clone(), proj.clone()),
            (ws.replace('\\', "/"), proj.replace('\\', "/")),
            (ws.replace('/', "\\"), proj.replace('/', "\\")),
        ] {
            if from.is_empty() || seen.iter().any(|s| s == &from) {
                continue;
            }
            seen.push(from.clone());
            out = out.replace(&from, &to);
        }
        out
    }

    /// Замаскировать относительные имена образов маркерами (AG-26).
    ///
    /// Маркер `\u{1}<n>\u{1}` не содержит `[`, поэтому разворот меток его не
    /// трогает; после него маркер заменяется путём оригинала. Заменяем только
    /// вхождение в начале пути: если перед именем стоит `\` или `/`, это хвост
    /// уже заменённого абсолютного пути — второй раз его менять нельзя.
    fn mask_relative(&self, text: &str) -> (String, Vec<String>) {
        if self.relative.is_empty() {
            return (text.to_string(), Vec::new());
        }
        let mut out = text.to_string();
        let mut targets: Vec<String> = Vec::new();
        for pair in &self.relative {
            let marker = format!("\u{1}{}\u{1}", targets.len());
            if let Some(replaced) = replace_at_path_start(&out, &pair.from, &marker) {
                out = replaced;
                targets.push(pair.to.clone());
            }
        }
        (out, targets)
    }

    /// Вернуть пути оригиналов вместо маркеров.
    fn restore_relative(&self, text: &str, targets: &[String]) -> String {
        if targets.is_empty() {
            return text.to_string();
        }
        let mut out = text.to_string();
        for (idx, target) in targets.iter().enumerate() {
            out = out.replace(&format!("\u{1}{idx}\u{1}"), target);
        }
        out
    }
}

/// Варианты имени образа с обоими разделителями (реестр пишет платформенный,
/// модель может прислать любой).
fn separator_variants(text: &str) -> Vec<String> {
    let mut out = vec![text.to_string()];
    let forward = text.replace('\\', "/");
    if forward != text {
        out.push(forward);
    }
    let backward = text.replace('/', "\\");
    if backward != text && !out.contains(&backward) {
        out.push(backward);
    }
    out
}

/// Заменить `from` в начале пути: предыдущий символ не `\` и не `/`.
///
/// Так `Протоколы\Встреча.docx` не подменяет хвост уже заменённого
/// `C:\Проект\Протоколы\Встреча.docx` (AG-26).
fn replace_at_path_start(text: &str, from: &str, to: &str) -> Option<String> {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    let mut changed = false;
    while let Some(pos) = rest.find(from) {
        let at_path_start = rest[..pos]
            .chars()
            .next_back()
            .map(|c| c != '\\' && c != '/')
            .unwrap_or(true);
        result.push_str(&rest[..pos]);
        if at_path_start {
            result.push_str(to);
            changed = true;
        } else {
            result.push_str(from);
        }
        rest = &rest[pos + from.len()..];
    }
    result.push_str(rest);
    changed.then_some(result)
}

/// Карта «имя образа → путь оригинала» для [`DeAnonymizer::with_relative_paths`].
///
/// Только записи с непустым образом; путь строится как
/// `<папка проекта>\<orig_rel>` (разделители платформы). Значений из базы
/// меток здесь нет — только имена файлов (AG-26, I8).
pub fn registry_paths(
    registry: &crate::registry_db::ArtifactRegistry,
    project_root: &Path,
) -> Vec<(String, String)> {
    let Ok(rows) = registry.list() else {
        return Vec::new();
    };
    rows.into_iter()
        .filter(|row| !row.anon_rel.is_empty())
        .map(|row| {
            let rel = row.orig_rel.replace('/', std::path::MAIN_SEPARATOR_STR);
            let target = project_root.join(rel);
            (row.anon_rel, target.to_string_lossy().into_owned())
        })
        .collect()
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

    /// Путь из конфига может быть записан через `/` (`agent.root: 'C:/zone'`),
    /// а модель присылает `C:\zone\...`: заменять надо оба вида — иначе в
    /// ленте остаётся путь зоны (найдено на стенде A5.3).
    #[test]
    fn workspace_path_matches_any_separator() {
        let d = DeAnonymizer::new(
            &snapshot(),
            &[],
            Path::new("C:/anon/P1/workspace"),
            Path::new("C:/Проект"),
        );
        assert_eq!(
            d.text("Файл C:\\anon\\P1\\workspace\\Отчёт.docx готов")
                .text,
            "Файл C:\\Проект\\Отчёт.docx готов"
        );
        assert_eq!(
            d.text("Файл C:/anon/P1/workspace/Отчёт.docx готов").text,
            "Файл C:/Проект/Отчёт.docx готов"
        );
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

    /// AG-26: относительное имя образа (результат `disk-search`) становится
    /// путём оригинала; метки в имени разворачиваются; абсолютный путь зоны
    /// не заменяется дважды.
    #[test]
    fn relative_image_paths_become_project_paths() {
        let d = deanon().with_relative_paths(vec![(
            "Протоколы\\Встреча [PERSON_1].docx".to_string(),
            "C:\\Проект\\Протоколы\\Встреча.docx".to_string(),
        )]);
        assert_eq!(
            d.text("Файл Протоколы\\Встреча [PERSON_1].docx готов").text,
            "Файл C:\\Проект\\Протоколы\\Встреча.docx готов"
        );
        // Разделители — любые: модель может вернуть `/`.
        assert_eq!(
            d.text("Файл Протоколы/Встреча [PERSON_1].docx готов").text,
            "Файл C:\\Проект\\Протоколы\\Встреча.docx готов"
        );
        // Абсолютный путь зоны: префикс меняется один раз, хвост не дублируется
        // (метка разворачивается как обычно).
        assert_eq!(
            d.text("Файл C:\\anon\\P1\\workspace\\Протоколы\\Встреча [PERSON_1].docx готов")
                .text,
            "Файл C:\\Проект\\Протоколы\\Встреча Иванов Иван.docx готов"
        );
    }

    /// AG-26: карта строится из реестра, каталог проекта — из `orig_rel`.
    #[test]
    fn registry_paths_map_images_to_originals() {
        use crate::registry_db::{Artifact, ArtifactRegistry, Kind, Status};

        let dir = std::env::temp_dir().join(format!("hds-deanon-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("P0001.db");
        let registry = ArtifactRegistry::open(&db).unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        let orig = format!("Протоколы{sep}Встреча.docx");
        let anon = format!("Протоколы{sep}Встреча [PERSON_1].docx");
        registry
            .insert(&Artifact::new(&orig, &anon, Kind::Image, Status::Synced))
            .unwrap();

        let paths = registry_paths(&registry, Path::new("C:\\Проект"));
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].0, anon);
        assert!(
            paths[0].1.ends_with(&orig),
            "путь оригинала: {:?}",
            paths[0].1
        );
        drop(registry);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
