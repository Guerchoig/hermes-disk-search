//! Панель «Метки проекта» (A5.5, TR-2.6): метки, где встречаются, склейки.
//!
//! Почему модуль в ядре, а не в UI: «где встречается» требует реестра
//! артефактов, `workspace` и воркера извлечения — это зона `hds-agent`. UI
//! только рисует ответ (своих регулярок меток у него нет, AG-1).
//!
//! Три источника:
//! * база меток — RPC `vault_project_labels` через [`Anonymizer`]: метки с
//!   формами (значения — зона пользователя), псевдонимы и журнал склеек;
//! * образы реестра — текст читается [`crate::validator::read_text`], метки
//!   ищутся `hds_core::labels::find_labels`; считаются **вхождения** метки по
//!   всем файлам, где она есть (точный счётчик — фаза 2, В-A5-2);
//! * `keep_terms` — список «не анонимизировать» (D19).
//!
//! Заголовки docx в текст `hds_extract` не отдаёт (A5.4 §6 п.24), поэтому
//! панель подписана «по тексту документов (тело)».

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use hds_anonymize::{MergeRecord, SnapshotForm};
use serde::Serialize;

use crate::mirror::{rel_to_path, Anonymizer, Mirror, TextExtractor};
use crate::registry_db::{ArtifactRegistry, RegistryError};
use crate::validator::read_text;

/// Вхождения метки в один образ реестра.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LabelOccurrence {
    /// Относительный путь оригинала (папка проекта).
    pub rel: String,
    /// Относительный путь образа (workspace).
    pub anon_rel: String,
    /// Сколько раз метка встретилась в тексте образа.
    pub count: usize,
}

/// Строка таблицы «метка → значение (формы) → где встречается».
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LabelRow {
    pub label: String,
    pub entity_type: String,
    /// Формы значения: падежи, встреченные и порождённые (TR-2.2).
    pub forms: Vec<SnapshotForm>,
    /// Младшие метки, ставшие псевдонимами этой (TR-2.6).
    pub aliases: Vec<String>,
    /// Файлы реестра, где метка есть в образе.
    pub occurrences: Vec<LabelOccurrence>,
}

impl LabelRow {
    /// Число файлов, где метка есть в образе (колонка «где встречается»).
    pub fn files(&self) -> usize {
        self.occurrences.len()
    }

    /// Число форм значения.
    pub fn forms_count(&self) -> usize {
        self.forms.len()
    }
}

/// Ответ `GET /api/agent/labels` целиком.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LabelsReport {
    pub rev: i64,
    pub labels: Vec<LabelRow>,
    pub merges: Vec<MergeRecord>,
    pub keep: Vec<String>,
}

/// Ошибка сборки отчёта: значения в текст ошибки не попадают (I8).
#[derive(Debug)]
pub enum LabelsError {
    /// Реестр артефактов недоступен.
    Registry(RegistryError),
    /// Воркер анонимизации не отдал метки.
    Anonymizer(String),
    /// Не удалось прочитать образ (воркер извлечения).
    Extractor(String),
    /// Файловая ошибка.
    Io(std::path::PathBuf, String),
}

impl std::fmt::Display for LabelsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LabelsError::Registry(e) => write!(f, "реестр: {e}"),
            LabelsError::Anonymizer(m) => write!(f, "метки проекта: {m}"),
            LabelsError::Extractor(m) => write!(f, "извлечение текста: {m}"),
            LabelsError::Io(path, m) => write!(f, "{}: {m}", path.display()),
        }
    }
}

impl std::error::Error for LabelsError {}

impl From<RegistryError> for LabelsError {
    fn from(e: RegistryError) -> LabelsError {
        LabelsError::Registry(e)
    }
}

/// Текст сообщения агенту о склейке: только метки (I8).
pub fn merge_message(keep: &str, dropped: &[String]) -> String {
    format!("метки {} теперь обозначаются {}", dropped.join(", "), keep)
}

/// Текст сообщения агенту о разъединении: только метки (I8).
pub fn unmerge_message(restored: &[String]) -> String {
    format!(
        "склейка меток отменена: {} снова самостоятельные",
        restored.join(", ")
    )
}

/// Снимок `vault_project_labels` + проход по образам реестра.
pub fn report(
    registry: &ArtifactRegistry,
    workspace: &Path,
    anonymizer: &mut dyn Anonymizer,
    extractor: &mut dyn TextExtractor,
    project_id: &str,
) -> Result<LabelsReport, LabelsError> {
    let labels = anonymizer
        .project_labels(project_id)
        .map_err(|e| LabelsError::Anonymizer(e.to_string()))?;

    // Вхождения: (тип, номер) → файлы. Сравнение по частям метки, а не по
    // строке: `[PERSON_1|дат]` — та же сущность (TR-2.4).
    let mut occurrences: BTreeMap<(String, u32), Vec<LabelOccurrence>> = BTreeMap::new();
    for row in registry.list()? {
        if row.anon_rel.is_empty() {
            continue;
        }
        let image = workspace.join(rel_to_path(&row.anon_rel));
        let text = match read_text(&image, extractor) {
            Ok(text) => text,
            // Образ не прочитан — метки в нём не считаем; панель подписана
            // «по тексту документов», а не «файл не открылся» (A5.4 §6 п.25).
            Err(_) => continue,
        };
        let mut counts: BTreeMap<(String, u32), usize> = BTreeMap::new();
        for token in hds_core::labels::find_labels(&text) {
            let Some((kind, n, _gram)) = hds_core::labels::label_parts(&token) else {
                continue;
            };
            *counts.entry((kind, n)).or_insert(0) += 1;
        }
        for (key, count) in counts {
            occurrences.entry(key).or_default().push(LabelOccurrence {
                rel: row.orig_rel.clone(),
                anon_rel: row.anon_rel.clone(),
                count,
            });
        }
    }

    let mut rows: Vec<LabelRow> = Vec::with_capacity(labels.labels.len());
    for item in labels.labels {
        let key = hds_core::labels::label_parts(&item.label)
            .map(|(kind, n, _gram)| (kind, n))
            .unwrap_or_default();
        rows.push(LabelRow {
            label: item.label,
            entity_type: item.entity_type,
            forms: item.forms,
            aliases: item.aliases,
            occurrences: occurrences.remove(&key).unwrap_or_default(),
        });
    }
    Ok(LabelsReport {
        rev: labels.rev,
        labels: rows,
        merges: labels.merges,
        keep: labels.keep,
    })
}

impl<'a> Mirror<'a> {
    /// Отчёт панели «Метки проекта» для этого проекта.
    pub fn labels_report(&mut self) -> Result<LabelsReport, LabelsError> {
        let registry = self.registry;
        let project_id = self.project_id.clone();
        report(
            registry,
            &self.workspace,
            &mut *self.anonymizer,
            &mut *self.extractor,
            &project_id,
        )
    }
}

/// Метки проекта по строкам отчёта — для проверки «один тип» в UI.
pub fn labels_of(rows: &[LabelRow]) -> BTreeSet<String> {
    rows.iter().map(|row| row.label.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::tests::{fast, FakeAnon, FakeExtractor, Stand};
    use crate::mirror::Mirror;
    use crate::registry_db::{Artifact, Kind, Status};

    /// Стенд: два образа, в одном `[PERSON_1]` и `[PERSON_2]`, во втором —
    /// только `[PERSON_1]`.
    fn stand(tag: &str) -> Stand {
        let stand = Stand::new(tag);
        stand.write("Письмо.md", "Иванов Иван и Петров Пётр\n");
        stand.write_ws("Письмо [PERSON_1].md", "Контакт: [PERSON_1] и [PERSON_2]\n");
        stand.write("Отчёт.md", "Иванов Иван\n");
        stand.write_ws("Отчёт [PERSON_1].md", "Контакт: [PERSON_1|дат]\n");
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        for (rel, anon) in [
            ("Письмо.md", "Письмо [PERSON_1].md"),
            ("Отчёт.md", "Отчёт [PERSON_1].md"),
        ] {
            let mut row = Artifact::new(rel, anon, Kind::Image, Status::Synced);
            row.anon_sha256_issued = Some("x".to_string());
            registry.upsert(&row).unwrap();
        }
        stand
    }

    #[test]
    fn report_counts_occurrences_per_file() {
        let stand = stand("labels-report");
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        let report =
            report(&registry, &stand.workspace, &mut anon, &mut ext, "P0001").expect("отчёт меток");
        assert_eq!(report.rev, anon.rev);
        assert_eq!(report.labels.len(), 2);
        assert_eq!(report.labels[0].label, "[PERSON_1]");
        // метка есть в двух образах; `[PERSON_2]` — только в «Письме»
        assert_eq!(report.labels[0].files(), 2);
        assert_eq!(report.labels[0].forms_count(), 0);
        assert_eq!(report.labels[1].label, "[PERSON_2]");
        assert_eq!(report.labels[1].files(), 1);
        assert!(report.keep.is_empty());
        assert!(report.merges.is_empty());
    }

    #[test]
    fn report_survives_missing_workspace_file() {
        let stand = stand("labels-missing");
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut row = Artifact::new(
            "Пропавший.md",
            "Пропавший [PERSON_1].md",
            Kind::Image,
            Status::Synced,
        );
        row.anon_sha256_issued = Some("x".to_string());
        registry.upsert(&row).unwrap();
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        let report = report(&registry, &stand.workspace, &mut anon, &mut ext, "P0001").unwrap();
        let missing = report
            .labels
            .iter()
            .all(|row| row.occurrences.iter().all(|o| o.rel != "Пропавший.md"));
        assert!(missing);
    }

    #[test]
    fn messages_carry_labels_only() {
        assert_eq!(
            merge_message("[PERSON_1]", &["[PERSON_2]".to_string()]),
            "метки [PERSON_2] теперь обозначаются [PERSON_1]"
        );
        assert_eq!(
            unmerge_message(&["[PERSON_2]".to_string()]),
            "склейка меток отменена: [PERSON_2] снова самостоятельные"
        );
    }

    #[test]
    fn mirror_report_is_available_through_the_core() {
        let stand = stand("labels-mirror");
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        let mut mirror = Mirror::new(
            "P0001",
            &stand.project,
            &stand.workspace,
            &registry,
            &mut anon,
            &mut ext,
            fast(),
            10,
        );
        let report = mirror.labels_report().expect("отчёт через зеркало");
        assert_eq!(report.labels.len(), 2);
        assert_eq!(labels_of(&report.labels).len(), 2);
    }
}
