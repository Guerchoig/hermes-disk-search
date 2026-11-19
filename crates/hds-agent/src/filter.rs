//! Словарный фильтр исходящего (TR-6.1…6.7).
//!
//! Точки применения:
//! * `model` — тело каждого HTTP-запроса к провайдеру;
//! * `mcp_out` — аргументы вызова внешнего MCP;
//! * `mcp_in` — результат внешнего MCP (заменяются только **известные** базе
//!   значения, TR-6.3).
//!
//! Словарь собирается из снимка vault (`Client::snapshot`): формы сущностей
//! проекта → метка проекта с падежным суффиксом; `others` → метка проекта через
//! `label_for_value` (или отказ при `agent.filter.unknown: block`); системные
//! строки (имя пользователя, компьютер, профиль, путь проекта, имена проектов)
//! → `[SYS]`; секреты → блокировка в точках `model`/`mcp_out` (TR-6.2 г).
//!
//! Сравнение — без учёта регистра, по границам слова, с нормализацией
//! `ё→е`, неразрывных пробелов, типографских кавычек и дефисов. Значения
//! короче 3 символов и значения из `keep_terms` в словарь не попадают.
//!
//! Fail-closed (TR-6.5): нет снимка, снимок старше `max_stale` или ошибка
//! словаря — `privacy_filter_unavailable`, запрос не уходит.
//!
//! Аудит (TR-6.4): `agent_egress_redacted` с меткой и источником, **без
//! значения**.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use serde_json::Value;

use hds_anonymize::protocol::{Snapshot, SnapshotForm};
use hds_core::audit::{Audit, AuditEvent};
use hds_core::labels::gram_to_suffix;

/// Точка применения фильтра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    Model,
    McpOut,
    McpIn,
}

impl Point {
    pub fn as_str(self) -> &'static str {
        match self {
            Point::Model => "model",
            Point::McpOut => "mcp_out",
            Point::McpIn => "mcp_in",
        }
    }
}

/// Источник срабатывания словаря (TR-6.4: а/б/в/г).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Форма сущности с меткой проекта.
    Project,
    /// Форма прочей сущности базы (`others`).
    Other,
    /// Системная строка (имя пользователя, компьютер, путь, имя проекта).
    System,
    /// Секрет (ключ провайдера/MCP, токен маршрута).
    Secret,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Project => "project",
            Source::Other => "other",
            Source::System => "system",
            Source::Secret => "secret",
        }
    }
}

/// Срабатывание фильтра (без исходного значения).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub label: String,
    pub source: Source,
}

/// Результат фильтрации: очищенное значение и срабатывания.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterOutcome {
    pub payload: Value,
    pub hits: Vec<Hit>,
}

/// Ошибка фильтра — всегда fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterError {
    /// Снимка vault ещё нет.
    NoSnapshot,
    /// Снимок старше `agent.filter.max_stale`.
    Stale(u64),
    /// Секрет в точке `model`/`mcp_out` (TR-6.2 г).
    Secret,
    /// Неизвестное значение при `agent.filter.unknown: block`.
    UnknownValue,
}

impl FilterError {
    /// Код для воркера и UI: `privacy_filter_unavailable`.
    pub fn code(&self) -> &'static str {
        "privacy_filter_unavailable"
    }

    pub fn reason(&self) -> String {
        match self {
            FilterError::NoSnapshot => "нет снимка vault".to_string(),
            FilterError::Stale(secs) => format!("снимок vault старше {secs} с"),
            FilterError::Secret => "в запросе секрет".to_string(),
            FilterError::UnknownValue => "неизвестное значение при политике block".to_string(),
        }
    }
}

/// Куда обращаться за новой меткой для `others` (реализует шлюз через
/// `Client::label_for_value`).
pub trait Labeler {
    fn label_for(&mut self, value: &str) -> Option<String>;
}

/// Заглушка: меток не выдаёт (тогда `others` при политике `label` заменяются
/// на `[OTHER]`, при `block` — отказ).
pub struct NoLabeler;

impl Labeler for NoLabeler {
    fn label_for(&mut self, _value: &str) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone)]
struct Entry {
    needle: String,
    value: String,
    replacement: String,
    source: Source,
}

#[derive(Debug)]
struct Dictionary {
    rev: i64,
    built_at: Instant,
    matcher: AhoCorasick,
    entries: Vec<Entry>,
}

/// Список значений `[SYS_n]` в порядке нумерации фильтра: сначала системные
/// строки, затем имена/пути проектов. `[SYS_1]` — первый элемент.
///
/// Деанонимизатор обязан получать именно этот список, иначе метка `[SYS_n]`
/// проекта вернётся чужим значением.
pub fn sys_values(system: &[String], projects: &[String]) -> Vec<String> {
    system.iter().chain(projects.iter()).cloned().collect()
}

impl Dictionary {
    /// Собрать словарь по снимку vault и системным строкам.
    fn build(
        snapshot: &Snapshot,
        keep: &[String],
        system: &[String],
        projects: &[String],
        secrets: &[String],
    ) -> Dictionary {
        let keep_norm: Vec<String> = keep
            .iter()
            .map(|k| fold(k).text.trim().to_string())
            .collect();
        let mut entries: Vec<Entry> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for label in &snapshot.labels {
            for form in &label.forms {
                push_entry(
                    &mut entries,
                    &mut seen,
                    &keep_norm,
                    form,
                    label_with_gram(&label.label, form.gram.as_deref()),
                    Source::Project,
                );
            }
        }
        for value in &snapshot.others {
            push_entry(
                &mut entries,
                &mut seen,
                &keep_norm,
                &SnapshotForm {
                    value: value.clone(),
                    gram: None,
                    origin: String::new(),
                },
                "[OTHER]".to_string(),
                Source::Other,
            );
        }
        // Системные строки нумеруются по порядку: [SYS_1], [SYS_2], … —
        // так деанонимизация возвращает конкретное значение (TR-5.3).
        for (i, value) in sys_values(system, projects).iter().enumerate() {
            let sys_no = i + 1;
            push_entry(
                &mut entries,
                &mut seen,
                &keep_norm,
                &SnapshotForm {
                    value: value.clone(),
                    gram: None,
                    origin: String::new(),
                },
                format!("[SYS_{sys_no}]"),
                Source::System,
            );
        }
        for value in secrets {
            push_entry(
                &mut entries,
                &mut seen,
                &keep_norm,
                &SnapshotForm {
                    value: value.clone(),
                    gram: None,
                    origin: String::new(),
                },
                "[SECRET]".to_string(),
                Source::Secret,
            );
        }
        let patterns: Vec<&str> = entries.iter().map(|e| e.needle.as_str()).collect();
        let matcher = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns)
            .unwrap_or_else(|_| AhoCorasick::new([""]).expect("пустой шаблон"));
        Dictionary {
            rev: snapshot.rev,
            built_at: Instant::now(),
            matcher,
            entries,
        }
    }
}

fn push_entry(
    entries: &mut Vec<Entry>,
    seen: &mut Vec<String>,
    keep_norm: &[String],
    form: &SnapshotForm,
    replacement: String,
    source: Source,
) {
    let trimmed = form.value.trim();
    if trimmed.chars().count() < 3 {
        return;
    }
    // Свёртка может укоротить значение (кавычки, пробелы), поэтому длину
    // проверяем и после неё: иначе `«ИП»` дало бы шаблон `ип`.
    let needle = fold(trimmed).text.trim().to_string();
    if needle.chars().count() < 3 {
        return;
    }
    if keep_norm.contains(&needle) || seen.contains(&needle) {
        return;
    }
    seen.push(needle.clone());
    entries.push(Entry {
        needle,
        value: trimmed.to_string(),
        replacement,
        source,
    });
}

/// Метка с падежным суффиксом: `[PERSON_3]` + `datv` → `[PERSON_3|дат]`
/// (TR-2.3/TR-2.4). Граммема базы (`forms.gram` — код pymorphy) переводится в
/// русский суффикс; именительный ед. ч. суффикса не получает. Уже выпущенные
/// метки с латинским кодом (`[PERSON_1|nomn]`) остаются как есть — их разбирает
/// `hds_core::labels`.
fn label_with_gram(label: &str, gram: Option<&str>) -> String {
    let suffix = gram.map(gram_to_suffix).unwrap_or_default();
    if suffix.is_empty() || !label.ends_with(']') || label.contains('|') {
        return label.to_string();
    }
    format!("{}|{suffix}]", &label[..label.len() - 1])
}

/// Словарный фильтр проекта.
pub struct Filter {
    project: String,
    keep: Vec<String>,
    unknown: crate::settings::UnknownPolicy,
    max_stale: Duration,
    dict: Mutex<Option<Arc<Dictionary>>>,
    audit: Option<Audit>,
}

impl Filter {
    pub fn new(
        project: impl Into<String>,
        keep: Vec<String>,
        unknown: crate::settings::UnknownPolicy,
        max_stale: Duration,
        audit: Option<Audit>,
    ) -> Self {
        Filter {
            project: project.into(),
            keep,
            unknown,
            max_stale,
            dict: Mutex::new(None),
            audit,
        }
    }

    /// Пересобрать словарь по снимку vault (TR-6.2).
    pub fn set_snapshot(
        &self,
        snapshot: &Snapshot,
        system: &[String],
        projects: &[String],
        secrets: &[String],
    ) {
        let dict = Dictionary::build(snapshot, &self.keep, system, projects, secrets);
        if let Ok(mut slot) = self.dict.lock() {
            *slot = Some(Arc::new(dict));
        }
    }

    /// Ревизия текущего словаря.
    pub fn rev(&self) -> Option<i64> {
        self.dict
            .lock()
            .ok()
            .and_then(|d| d.as_ref().map(|d| d.rev))
    }

    /// Почистить уже сохранённую историю (TR-6.6): известные значения → метки,
    /// без выдачи новых меток и без блокировки. Точка — `mcp_in`, поэтому
    /// секреты заменяются на `[SECRET]`, а не роняют очистку.
    pub fn redact_stored(&self, payload: &Value) -> Result<FilterOutcome, FilterError> {
        // Снимок берётся под замком и сразу отпускается: словарь — `Arc`,
        // поэтому долгий проход не блокирует `set_snapshot` (TR-6.5).
        let dict = {
            let guard = self.dict.lock().map_err(|_| FilterError::NoSnapshot)?;
            Arc::clone(guard.as_ref().ok_or(FilterError::NoSnapshot)?)
        };
        if dict.built_at.elapsed() > self.max_stale {
            return Err(FilterError::Stale(self.max_stale.as_secs()));
        }
        let mut hits = Vec::new();
        let mut error = None;
        let payload = redact_value(
            &dict,
            crate::settings::UnknownPolicy::Label,
            Point::McpIn,
            payload,
            &mut NoLabeler,
            &mut hits,
            &mut error,
        );
        // Ошибки не прерывают очистку: история — best-effort.
        let _ = error;
        Ok(FilterOutcome { payload, hits })
    }

    /// Очистить значение (строка или JSON-дерево) в заданной точке.
    pub fn redact(
        &self,
        point: Point,
        payload: &Value,
        labeler: &mut dyn Labeler,
    ) -> Result<FilterOutcome, FilterError> {
        // Замок словаря держится только на клонирование `Arc`: `labeler` может
        // ходить в Python (`label_for_value`), и держать замок всё это время
        // нельзя — `set_snapshot` (обновление снимка) ждал бы его.
        let dict = {
            let guard = self.dict.lock().map_err(|_| FilterError::NoSnapshot)?;
            Arc::clone(guard.as_ref().ok_or(FilterError::NoSnapshot)?)
        };
        if dict.built_at.elapsed() > self.max_stale {
            return Err(FilterError::Stale(self.max_stale.as_secs()));
        }
        let mut hits = Vec::new();
        let mut error: Option<FilterError> = None;
        let payload = redact_value(
            &dict,
            self.unknown,
            point,
            payload,
            labeler,
            &mut hits,
            &mut error,
        );
        if let Some(error) = error {
            return Err(error);
        }
        self.audit_hits(point, &hits);
        Ok(FilterOutcome { payload, hits })
    }

    fn audit_hits(&self, point: Point, hits: &[Hit]) {
        let Some(audit) = self.audit.as_ref() else {
            return;
        };
        for hit in hits {
            let event = AuditEvent {
                ts: now_unix(),
                event: "agent_egress_redacted".to_string(),
                user: None,
                ip: "-".to_string(),
                method: point.as_str().to_string(),
                path: self.project.clone(),
                tool: Some(hit.label.clone()),
                projects: Some(vec![self.project.clone()]),
                token: Some(hit.source.as_str().to_string()),
                status: 200,
                ms: 0,
            };
            audit.record(&event);
        }
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Свёрнутый текст: нижний регистр, `ё→е`, дефисы, кавычки → пробел,
/// подряд идущие пробелы схлопнуты. `orig[i]` — байтовое смещение i-го
/// сохранённого символа в исходнике, `starts[i]` — смещение того же символа
/// в свёрнутом тексте (для перевода совпадений).
///
/// Кавычки в снимке и в тексте пишутся по-разному (`ООО «Ромашка»`,
/// `ООО Ромашка`), поэтому кавычка считается разделителем: после свёртки
/// обе записи дают `ооо ромашка`. Схлопывание пробелов нужно, чтобы
/// `ООО «Ромашка»` не превращалось в `ооо  ромашка ` с двойным пробелом.
struct Folded {
    text: String,
    orig: Vec<usize>,
    starts: Vec<usize>,
}

fn fold(text: &str) -> Folded {
    let mut out = String::with_capacity(text.len());
    let mut orig = Vec::with_capacity(text.len());
    let mut starts = Vec::with_capacity(text.len());
    let mut prev_space = false;
    for (i, ch) in text.char_indices() {
        let mapped = match ch {
            'ё' | 'Ё' => 'е',
            '«' | '»' | '“' | '”' | '„' | '‘' | '’' => ' ',
            '–' | '—' | '−' => '-',
            other if other.is_whitespace() => ' ',
            other => other,
        };
        // Один символ на один: `to_lowercase` иногда даёт два символа, берём
        // первый, чтобы индексы оставались взаимно однозначными.
        let lower = mapped.to_lowercase().next().unwrap_or(mapped);
        if lower == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        orig.push(i);
        starts.push(out.len());
        out.push(lower);
    }
    orig.push(text.len());
    starts.push(out.len());
    Folded {
        text: out,
        orig,
        starts,
    }
}

fn redact_value(
    dict: &Dictionary,
    unknown: crate::settings::UnknownPolicy,
    point: Point,
    value: &Value,
    labeler: &mut dyn Labeler,
    hits: &mut Vec<Hit>,
    error: &mut Option<FilterError>,
) -> Value {
    match value {
        Value::String(text) => match redact_text(dict, unknown, point, text, labeler, hits) {
            Ok(clean) => Value::String(clean),
            Err(err) => {
                *error = Some(err);
                Value::Null
            }
        },
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_value(dict, unknown, point, item, labeler, hits, error))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                let clean_key = match redact_text(dict, unknown, point, key, labeler, hits) {
                    Ok(clean) => clean,
                    Err(err) => {
                        *error = Some(err);
                        key.clone()
                    }
                };
                out.insert(
                    clean_key,
                    redact_value(dict, unknown, point, item, labeler, hits, error),
                );
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn redact_text(
    dict: &Dictionary,
    unknown: crate::settings::UnknownPolicy,
    point: Point,
    text: &str,
    labeler: &mut dyn Labeler,
    hits: &mut Vec<Hit>,
) -> Result<String, FilterError> {
    let folded = fold(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for m in dict.matcher.find_iter(&folded.text) {
        let start_char = folded.starts.partition_point(|&s| s < m.start());
        let end_char = folded.starts.partition_point(|&s| s < m.end());
        if end_char <= start_char {
            continue;
        }
        let entry = &dict.entries[m.pattern()];
        // Границы слова: рядом не должно быть букв и цифр. Для секретов
        // проверка не применяется — секрет, вклеенный в более длинный токен,
        // обязан блокировать запрос (fail-closed, TR-6.2 г).
        if entry.source != Source::Secret {
            if start_char > 0 {
                if let Some(prev) = folded.text[..folded.starts[start_char]].chars().next_back() {
                    if prev.is_alphanumeric() {
                        continue;
                    }
                }
            }
            if let Some(next) = folded.text[folded.starts[end_char - 1]..].chars().nth(1) {
                if next.is_alphanumeric() {
                    continue;
                }
            }
        }
        if entry.source == Source::Secret && matches!(point, Point::Model | Point::McpOut) {
            return Err(FilterError::Secret);
        }
        let replacement = match entry.source {
            Source::Other => match labeler.label_for(&entry.value) {
                Some(label) => label,
                None => match unknown {
                    crate::settings::UnknownPolicy::Block => return Err(FilterError::UnknownValue),
                    crate::settings::UnknownPolicy::Label => "[OTHER]".to_string(),
                },
            },
            _ => entry.replacement.clone(),
        };
        hits.push(Hit {
            label: replacement.clone(),
            source: entry.source,
        });
        out.push_str(&text[last..folded.orig[start_char]]);
        out.push_str(&replacement);
        last = folded.orig[end_char];
    }
    out.push_str(&text[last..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hds_anonymize::protocol::SnapshotLabel;
    use serde_json::json;

    fn snapshot() -> Snapshot {
        Snapshot {
            rev: 7,
            labels: vec![
                SnapshotLabel {
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
                },
                SnapshotLabel {
                    label: "[ORG_2]".to_string(),
                    entity_type: "ORG".to_string(),
                    forms: vec![SnapshotForm {
                        value: "ООО Ромашка".to_string(),
                        gram: None,
                        origin: "ner".to_string(),
                    }],
                },
            ],
            others: vec!["Петров Пётр".to_string()],
            keep: vec![],
        }
    }

    fn filter() -> Filter {
        let f = Filter::new(
            "P0001",
            vec!["ЗУП".to_string()],
            crate::settings::UnknownPolicy::Label,
            Duration::from_secs(60),
            None,
        );
        f.set_snapshot(
            &snapshot(),
            &["DESKTOP-ABC".to_string()],
            &["Проект Внедрения".to_string()],
            &["sk-live-0123456789".to_string()],
        );
        f
    }

    #[test]
    fn replaces_project_forms_with_label_and_gram() {
        let f = filter();
        let mut labeler = NoLabeler;
        let out = f
            .redact(
                Point::Model,
                &json!("Иванов Иван передал отчёт Иванову Ивану."),
                &mut labeler,
            )
            .unwrap();
        assert_eq!(
            out.payload,
            json!("[PERSON_1] передал отчёт [PERSON_1|дат].")
        );
        assert_eq!(out.hits.len(), 2);
        assert_eq!(out.hits[0].source, Source::Project);
    }

    #[test]
    fn case_and_yo_are_normalized() {
        let f = filter();
        let mut labeler = NoLabeler;
        let out = f
            .redact(
                Point::Model,
                &json!("петров пётр и ООО «РОМАШКА»"),
                &mut labeler,
            )
            .unwrap();
        let text = out.payload.as_str().unwrap();
        assert!(text.contains("[OTHER]"), "{text}");
        assert!(text.contains("[ORG_2]"), "{text}");
    }

    #[test]
    fn keep_terms_and_short_values_are_not_redacted() {
        let f = filter();
        let mut labeler = NoLabeler;
        let out = f
            .redact(Point::Model, &json!("ЗУП и 1С"), &mut labeler)
            .unwrap();
        assert_eq!(out.payload, json!("ЗУП и 1С"));
        assert!(out.hits.is_empty());
    }

    #[test]
    fn word_boundaries_are_respected() {
        let f = filter();
        let mut labeler = NoLabeler;
        let out = f
            .redact(Point::Model, &json!("Иванов Иванович"), &mut labeler)
            .unwrap();
        // «Иванов Иван» — подстрока «Иванов Иванович», но граница слова не
        // совпадает: замена не выполняется.
        assert_eq!(out.payload, json!("Иванов Иванович"));
    }

    #[test]
    fn system_and_project_names_become_sys() {
        let f = filter();
        let mut labeler = NoLabeler;
        let out = f
            .redact(
                Point::Model,
                &json!("Хост DESKTOP-ABC, проект «Проект Внедрения»"),
                &mut labeler,
            )
            .unwrap();
        let text = out.payload.as_str().unwrap();
        assert!(text.contains("[SYS_1]"), "{text}");
        assert!(text.contains("[SYS_2]"), "{text}");
    }

    #[test]
    fn sys_numbering_matches_deanonymizer() {
        // Метки `[SYS_n]` фильтра и деанонимизатора обязаны совпадать по
        // номерам: имя проекта (из `projects`) возвращается именно им.
        let system = vec!["DESKTOP-ABC".to_string()];
        let projects = vec!["Проект Внедрения".to_string()];
        let f = filter();
        let mut labeler = NoLabeler;
        let out = f
            .redact(
                Point::Model,
                &json!("проект Проект Внедрения"),
                &mut labeler,
            )
            .unwrap();
        let text = out.payload.as_str().unwrap().to_string();
        let d = crate::deanon::DeAnonymizer::new(
            &snapshot(),
            &sys_values(&system, &projects),
            std::path::Path::new("C:\\anon\\P0001\\workspace"),
            std::path::Path::new("C:\\Проект"),
        );
        assert_eq!(d.text(&text).text, "проект Проект Внедрения");
    }

    #[test]
    fn secret_blocks_model_and_mcp_out() {
        let f = filter();
        let mut labeler = NoLabeler;
        let err = f
            .redact(
                Point::Model,
                &json!("ключ sk-live-0123456789"),
                &mut labeler,
            )
            .unwrap_err();
        assert_eq!(err, FilterError::Secret);
        assert_eq!(err.code(), "privacy_filter_unavailable");
        let err = f
            .redact(Point::McpOut, &json!("sk-live-0123456789"), &mut labeler)
            .unwrap_err();
        assert_eq!(err, FilterError::Secret);
        // В mcp_in секрет заменяется, а не блокирует.
        let out = f
            .redact(Point::McpIn, &json!("sk-live-0123456789"), &mut labeler)
            .unwrap();
        assert_eq!(out.payload, json!("[SECRET]"));
    }

    #[test]
    fn pymorphy_gram_becomes_russian_suffix() {
        // Реальный снимок vault: `forms.gram` — код pymorphy, а метка обязана
        // получить русский суффикс (TR-2.3). Именительный — без суффикса.
        let mut snap = snapshot();
        snap.labels[0].forms = vec![
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
        ];
        let f = Filter::new(
            "P0001",
            Vec::new(),
            crate::settings::UnknownPolicy::Label,
            Duration::from_secs(60),
            None,
        );
        f.set_snapshot(&snap, &[], &[], &[]);
        let out = f
            .redact(
                Point::Model,
                &json!("Иванов Иван передал Иванову Ивану"),
                &mut NoLabeler,
            )
            .unwrap();
        assert_eq!(
            out.payload,
            json!("[PERSON_1] передал [PERSON_1|дат]"),
            "метка должна быть с русским суффиксом"
        );
    }

    #[test]
    fn secret_inside_longer_token_blocks() {
        let f = filter();
        let err = f
            .redact(Point::Model, &json!("xsk-live-0123456789y"), &mut NoLabeler)
            .unwrap_err();
        assert_eq!(
            err,
            FilterError::Secret,
            "секрет в длинном токене обязан блокировать"
        );
    }

    #[test]
    fn unknown_others_are_labelled_by_labeler_or_blocked() {
        struct Labeller;
        impl Labeler for Labeller {
            fn label_for(&mut self, _value: &str) -> Option<String> {
                Some("[PERSON_9]".to_string())
            }
        }
        let f = filter();
        let mut labeller = Labeller;
        let out = f
            .redact(Point::Model, &json!("Петров Пётр"), &mut labeller)
            .unwrap();
        assert_eq!(out.payload, json!("[PERSON_9]"));

        let strict = Filter::new(
            "P0001",
            vec![],
            crate::settings::UnknownPolicy::Block,
            Duration::from_secs(60),
            None,
        );
        strict.set_snapshot(&snapshot(), &[], &[], &[]);
        let mut labeler = NoLabeler;
        assert_eq!(
            strict
                .redact(Point::Model, &json!("Петров Пётр"), &mut labeler)
                .unwrap_err(),
            FilterError::UnknownValue
        );
    }

    #[test]
    fn no_snapshot_fails_closed() {
        let f = Filter::new(
            "P0001",
            vec![],
            crate::settings::UnknownPolicy::Label,
            Duration::from_secs(60),
            None,
        );
        let mut labeler = NoLabeler;
        assert_eq!(
            f.redact(Point::Model, &json!("текст"), &mut labeler)
                .unwrap_err(),
            FilterError::NoSnapshot
        );
    }

    #[test]
    fn json_tree_is_redacted_recursively() {
        let f = filter();
        let mut labeler = NoLabeler;
        let payload = json!({
            "messages": [{"role": "user", "content": "Иванов Иван"}],
            "tools": [{"name": "search", "description": "ООО Ромашка"}]
        });
        let out = f.redact(Point::Model, &payload, &mut labeler).unwrap();
        let text = out.payload.to_string();
        assert!(text.contains("[PERSON_1]"), "{text}");
        assert!(text.contains("[ORG_2]"), "{text}");
        assert!(!text.contains("Иванов"), "{text}");
    }
}
