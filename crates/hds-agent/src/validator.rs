//! Валидатор результата агента (TR-7.8, A4.5).
//!
//! К каждому выпущенному варианту (`.hds-agent\pending\<rel>`) рядом
//! появляется отчёт `<rel>.report.json`:
//!
//! ```json
//! {"rel": "...", "anon_rel": "...", "ts": 1795166000.5, "rev": 7,
//!  "verdict": "ok", "checks": [{"name": "text_lost", "verdict": "ok",
//!  "detail": "потеряно 3 % слов"}]}
//! ```
//!
//! Отчёт **ничего не блокирует** (TR-7.8): вариант остаётся `pending_accept`,
//! решения A4.4 работают как раньше, а `verdict: "check"` — пометка «проверить»
//! для вкладки «Агент» (A5).
//!
//! Четыре проверки:
//!
//! * [`CHECK_LABELS`] — метки **образа**, которых нет в снимке меток проекта
//!   ([`hds_anonymize::Snapshot`]): агент выдумал или исказил метку;
//! * [`CHECK_GRAMMAR`] — `[X_n|падеж]` без базовой формы `[X_n]` ни в документе,
//!   ни в снимке; разбор — только [`hds_core::labels`] (второй регулярки нет);
//! * [`CHECK_IMAGES`] — `[IMAGE_n]`, оставшиеся в **варианте** у [`Kind::Image`]:
//!   метка есть в карте `.hds-agent\base\<rel>.images.json`, но картинка не
//!   восстановлена. У [`Kind::TextImage`] картинок нет — «не применимо»;
//! * [`CHECK_TEXT_LOST`] — доля слов основания (`.hds-agent\base\<rel>`),
//!   отсутствующих в варианте, больше [`TEXT_LOST_LIMIT`] («документ пересобран
//!   с нуля»).
//!
//! Две стороны документа не путаются (грабля A4.5 №1): в `pending` — вариант со
//! **значениями**, в `workspace` — образ с **метками**. Проверки меток идут по
//! образу и снимку, «доля текста» — вариант против **основания** (сравнивать
//! образ с основанием нельзя: там метки).
//!
//! В отчёте нет текста документа — только метки, пути и числа (I8): детали
//! проверок собираются из меток, а содержимое образа или варианта в отчёт не
//! попадает ни в каком виде.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use hds_anonymize::Snapshot;
use hds_core::labels::{find_labels, gram_to_suffix, label_parts};
use hds_core::timefmt::now_epoch;
use serde::{Deserialize, Serialize};

use crate::mirror::TextExtractor;
use crate::registry_db::Kind;

/// Суффикс файла отчёта: `<rel>.report.json` рядом с вариантом.
pub const REPORT_SUFFIX: &str = ".report.json";

/// Порог «текст пропал» (В-8): доля слов основания, которых нет в варианте.
pub const TEXT_LOST_LIMIT: f64 = 0.40;

/// Имя проверки: метки образа, которых нет в снимке проекта.
pub const CHECK_LABELS: &str = "labels_outside_project";
/// Имя проверки: падежная метка без базовой формы.
pub const CHECK_GRAMMAR: &str = "label_grammar";
/// Имя проверки: `[IMAGE_n]`, не заменённые картинкой.
pub const CHECK_IMAGES: &str = "unplaced_images";
/// Имя проверки: доля пропавшего текста.
pub const CHECK_TEXT_LOST: &str = "text_lost";

/// Сколько меток перечислять в детали проверки (дальше — «и ещё N»).
const DETAIL_LABELS: usize = 5;

/// Путь отчёта для варианта: `pending\<rel>.report.json`.
pub fn report_path(variant: &Path) -> PathBuf {
    PathBuf::from(format!("{}{REPORT_SUFFIX}", variant.to_string_lossy()))
}

/// Вердикт проверки и отчёта: `ok` — вопросов нет, `check` — «проверить».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Check,
}

impl Verdict {
    /// Строка для JSON, журнала и тестов.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Ok => "ok",
            Verdict::Check => "check",
        }
    }
}

/// Одна проверка: имя, вердикт и объяснение без значений (I8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub verdict: Verdict,
    pub detail: String,
}

impl Check {
    fn ok(name: &str, detail: String) -> Check {
        Check {
            name: name.to_string(),
            verdict: Verdict::Ok,
            detail,
        }
    }

    /// Проверка, требующая внимания пользователя («проверить»).
    fn flagged(name: &str, detail: String) -> Check {
        Check {
            name: name.to_string(),
            verdict: Verdict::Check,
            detail,
        }
    }
}

/// Отчёт к варианту (TR-7.8) — ровно тот JSON, что читает A5.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// Относительный путь оригинала в папке проекта.
    pub rel: String,
    /// Относительный путь образа (метки) — из него собран вариант.
    pub anon_rel: String,
    /// Unix-время проверки (секунды с дробной частью).
    pub ts: f64,
    /// `rev` снимка меток, на котором проверяли (0 — снимок недоступен).
    pub rev: i64,
    /// Итог: `check`, если хотя бы одна проверка требует внимания.
    pub verdict: Verdict,
    /// Четыре проверки в порядке [`CHECK_LABELS`], [`CHECK_GRAMMAR`],
    /// [`CHECK_IMAGES`], [`CHECK_TEXT_LOST`].
    pub checks: Vec<Check>,
}

impl Report {
    /// Худший вердикт из проверок: пустой список — `ok`.
    pub fn verdict_of(checks: &[Check]) -> Verdict {
        if checks.iter().any(|c| c.verdict == Verdict::Check) {
            Verdict::Check
        } else {
            Verdict::Ok
        }
    }

    /// Записать отчёт рядом с вариантом (перезапись — сборка варианта заново).
    pub fn write(&self, variant: &Path) -> std::io::Result<PathBuf> {
        let path = report_path(variant);
        let mut body = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        body.push('\n');
        fs::write(&path, body)?;
        Ok(path)
    }

    /// Прочитать отчёт варианта (A5 и тесты); `None` — отчёта нет или он битый.
    pub fn read(variant: &Path) -> Option<Report> {
        let body = fs::read_to_string(report_path(variant)).ok()?;
        serde_json::from_str(&body).ok()
    }
}

/// Что проверять: пути двух сторон документа и сведения реестра.
pub struct Inputs<'a> {
    /// Относительный путь оригинала (он же `rel` в отчёте).
    pub rel: &'a str,
    /// Относительный путь образа в `<P>\workspace`.
    pub anon_rel: &'a str,
    /// Вид артефакта: у [`Kind::Image`] проверяются картинки.
    pub kind: Kind,
    /// Образ (метки) — `<P>\workspace\<anon_rel>`.
    pub image: &'a Path,
    /// Вариант (значения) — `.hds-agent\pending\<rel>`.
    pub variant: &'a Path,
    /// Снимок основания — `.hds-agent\base\<rel>`.
    pub base: &'a Path,
    /// Карта картинок основания — `.hds-agent\base\<rel>.images.json`.
    pub images: &'a Path,
}

/// Итог валидации: отчёт и ошибки самого валидатора.
///
/// `problems` — только сбои проверки (файл не прочитан, воркер извлечения упал):
/// они идут в `SyncReport::problems`. Подозрительный документ — не ошибка
/// валидатора: он остаётся в `report.checks` и в проблемы не попадает.
pub struct Outcome {
    pub report: Report,
    pub problems: Vec<String>,
}

/// Проверить вариант и собрать отчёт.
///
/// Ошибки самого валидатора не прерывают выпуск варианта: они возвращаются в
/// [`Outcome::problems`], а соответствующая проверка получает вердикт `check`.
/// `snapshot` — результат [`crate::mirror::Anonymizer::snapshot`]; снимок
/// недоступен — метки проверить нельзя, но текст и картинки проверяются.
pub fn validate(
    inputs: &Inputs<'_>,
    snapshot: Result<Snapshot, String>,
    extractor: &mut dyn TextExtractor,
) -> Outcome {
    let mut problems: Vec<String> = Vec::new();
    let (rev, known) = match snapshot {
        Ok(snapshot) => (snapshot.rev, Some(known_labels(&snapshot))),
        Err(e) => {
            problems.push(format!("снимок меток проекта — {e}"));
            (0, None)
        }
    };

    // Образ читается ради меток, вариант — ради картинок и текста, основание —
    // ради доли пропавшего текста. Каждое чтение независимо: сбой одного не
    // отменяет остальные проверки.
    let image_text = read_text(inputs.image, extractor);
    let variant_text = read_text(inputs.variant, extractor);
    let base_text = read_text(inputs.base, extractor);

    let checks = vec![
        check_labels(&image_text, known.as_ref(), &mut problems),
        check_grammar(&image_text, known.as_ref(), &mut problems),
        check_images(inputs, &variant_text, &mut problems),
        check_text_lost(&base_text, &variant_text, &mut problems),
    ];
    let verdict = Report::verdict_of(&checks);
    Outcome {
        report: Report {
            rel: inputs.rel.to_string(),
            anon_rel: inputs.anon_rel.to_string(),
            ts: now_epoch(),
            rev,
            verdict,
            checks,
        },
        problems,
    }
}

/// Метки проекта из снимка: `[PERSON_3|дат]` и `[PERSON_3]` — одна сущность,
/// поэтому сравниваются части [`label_parts`], а не строки (грабля №2).
fn known_labels(snapshot: &Snapshot) -> BTreeSet<(String, u32)> {
    snapshot
        .labels
        .iter()
        .filter_map(|label| label_parts(&label.label))
        .map(|(kind, number, _gram)| (kind, number))
        .collect()
}

/// Текст файла: `docx`/`xlsx`/`pdf`/`pptx`/`mpp` — `hds_extract`, текстовые
/// форматы — чтение как UTF-8 (битые байты заменяются, а не роняют проверку).
pub(crate) fn read_text(path: &Path, extractor: &mut dyn TextExtractor) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("нет файла {}", path.display()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "docx" | "xlsx" | "pdf" | "pptx" | "mpp" => {
            extractor.extract(path).map_err(|e| e.to_string())
        }
        "txt" | "md" | "xml" => fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|e| e.to_string()),
        other => Err(format!("формат {other} не проверяется")),
    }
}

/// Слова текста для сравнения «сколько пропало»: регистр, пунктуация и
/// служебные комментарии (`<!-- страница n -->`, маркер образа) не влияют.
///
/// Кратность важна (AG-28): повторение слова в основании значит, что вариант
/// обязан содержать его столько же раз. На множествах документ с потерянной
/// половиной текста проходил как «ок», потому что словарь сохранялся.
pub(crate) fn word_counts(text: &str) -> BTreeMap<String, usize> {
    let body = strip_comments(text);
    let mut words: BTreeMap<String, usize> = BTreeMap::new();
    let mut current = String::new();
    for ch in body.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                current.push(lower);
            }
        } else if !current.is_empty() {
            *words.entry(std::mem::take(&mut current)).or_insert(0) += 1;
        }
    }
    if !current.is_empty() {
        *words.entry(current).or_insert(0) += 1;
    }
    words
}

/// Убрать комментарии `<!-- … -->` (страницы текстового образа, маркер).
///
/// Своей регулярки нет намеренно: разбор меток — только `hds_core::labels`, а
/// здесь разбирается не метка, а служебная строка (AG-1).
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            // Незакрытый комментарий — хвост файла; слова из него не считаем.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `labels_outside_project`: метки образа, которых нет в снимке проекта.
///
/// `[IMAGE_n]` — не сущность, а картинка: её проверяет [`CHECK_IMAGES`],
/// поэтому здесь такие метки не считаются «выдуманными».
fn check_labels(
    image_text: &Result<String, String>,
    known: Option<&BTreeSet<(String, u32)>>,
    problems: &mut Vec<String>,
) -> Check {
    let Some(known) = known else {
        return Check::flagged(CHECK_LABELS, "снимок меток проекта недоступен".to_string());
    };
    let text = match image_text {
        Ok(text) => text,
        Err(e) => {
            problems.push(format!("метки не проверены — {e}"));
            return Check::flagged(CHECK_LABELS, "образ не прочитан".to_string());
        }
    };
    let mut outside: BTreeSet<(String, u32)> = BTreeSet::new();
    for token in find_labels(text) {
        if let Some((kind, number, _gram)) = label_parts(&token) {
            if kind != "IMAGE" && !known.contains(&(kind.clone(), number)) {
                outside.insert((kind, number));
            }
        }
    }
    if outside.is_empty() {
        Check::ok(CHECK_LABELS, "меток вне проекта нет".to_string())
    } else {
        Check::flagged(
            CHECK_LABELS,
            format!("метки вне проекта: {}", join_labels(&outside)),
        )
    }
}

/// `label_grammar`: `[X_n|падеж]` без базовой формы `[X_n]`.
///
/// Падежные формы — норма русского текста, поэтому `check` ставится только
/// тогда, когда базовой метки нет ни в документе, ни в снимке проекта.
/// Именительный (`|nomn`) приводится к пустому суффиксу через [`gram_to_suffix`].
fn check_grammar(
    image_text: &Result<String, String>,
    known: Option<&BTreeSet<(String, u32)>>,
    problems: &mut Vec<String>,
) -> Check {
    let text = match image_text {
        Ok(text) => text,
        Err(e) => {
            if !problems
                .iter()
                .any(|p| p == &format!("метки не проверены — {e}"))
            {
                problems.push(format!("падеж не проверен — {e}"));
            }
            return Check::flagged(CHECK_GRAMMAR, "образ не прочитан".to_string());
        }
    };
    let tokens = find_labels(text);
    let mut base_forms: BTreeSet<(String, u32)> = BTreeSet::new();
    for token in &tokens {
        if let Some((kind, number, None)) = label_parts(token) {
            base_forms.insert((kind, number));
        }
    }
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for token in &tokens {
        let Some((kind, number, Some(gram))) = label_parts(token) else {
            continue;
        };
        // `|nomn` — это базовая форма, а не падеж: суффикс пустой.
        if gram_to_suffix(&gram).is_empty() {
            continue;
        }
        let base = (kind, number);
        let in_document = base_forms.contains(&base);
        let in_project = known.map(|set| set.contains(&base)).unwrap_or(false);
        if !in_document && !in_project {
            missing.insert(token.clone());
        }
    }
    if missing.is_empty() {
        Check::ok(CHECK_GRAMMAR, "падежные формы согласованы".to_string())
    } else {
        let list: Vec<String> = missing.iter().take(DETAIL_LABELS).cloned().collect();
        Check::flagged(
            CHECK_GRAMMAR,
            format!("{} без базовой формы", list.join(", ")),
        )
    }
}

/// `unplaced_images`: `[IMAGE_n]`, оставшиеся в варианте.
///
/// У [`Kind::TextImage`] картинок нет (`*.pdf.md`) — «не применимо» (В-9).
/// Для [`Kind::Image`] метки картинок сверяются с картой основания: метка есть в
/// карте, но осталась текстом, — картинка не восстановлена.
fn check_images(
    inputs: &Inputs<'_>,
    variant_text: &Result<String, String>,
    problems: &mut Vec<String>,
) -> Check {
    match inputs.kind {
        Kind::Image => {}
        Kind::TextImage => {
            return Check::ok(
                CHECK_IMAGES,
                "не применимо (в текстовом образе картинок нет)".to_string(),
            )
        }
        _ => return Check::ok(CHECK_IMAGES, "не применимо".to_string()),
    }
    let text = match variant_text {
        Ok(text) => text,
        Err(e) => {
            problems.push(format!("картинки не проверены — {e}"));
            return Check::flagged(CHECK_IMAGES, "вариант не прочитан".to_string());
        }
    };
    let mut left: BTreeSet<(String, u32)> = BTreeSet::new();
    for token in find_labels(text) {
        if let Some((kind, number, _gram)) = label_parts(&token) {
            if kind == "IMAGE" {
                left.insert((kind, number));
            }
        }
    }
    if left.is_empty() {
        return Check::ok(CHECK_IMAGES, "непомещённых [IMAGE_n] нет".to_string());
    }
    let map = image_labels(inputs.images);
    if let Err(e) = &map {
        problems.push(format!("карта картинок не прочитана — {e}"));
    }
    let mut parts: Vec<String> = Vec::new();
    for (kind, number) in left.iter().take(DETAIL_LABELS) {
        let in_map = map
            .as_ref()
            .map(|set| set.contains(&(kind.clone(), *number)))
            .unwrap_or(false);
        parts.push(if in_map {
            format!("[{kind}_{number}] есть в карте основания, картинка не восстановлена")
        } else {
            format!("[{kind}_{number}] нет в карте основания")
        });
    }
    if left.len() > DETAIL_LABELS {
        parts.push(format!("и ещё {}", left.len() - DETAIL_LABELS));
    }
    Check::flagged(CHECK_IMAGES, parts.join("; "))
}

/// Метки картинок из `.images.json` (массив от `image_map` или `{"images": […]}`).
fn image_labels(path: &Path) -> Result<BTreeSet<(String, u32)>, String> {
    let body = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(mut map) => map
            .remove("images")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut labels = BTreeSet::new();
    for item in items {
        if let Some(label) = item.get("label").and_then(|l| l.as_str()) {
            if let Some((kind, number, _gram)) = label_parts(label) {
                labels.insert((kind, number));
            }
        }
    }
    Ok(labels)
}

/// `text_lost`: доля слов основания, которых нет в варианте.
///
/// Сравниваются **слова**, а не байты: правка регистра, пунктуации и переносов
/// потерей не считается. Порог — [`TEXT_LOST_LIMIT`] (В-8).
fn check_text_lost(
    base_text: &Result<String, String>,
    variant_text: &Result<String, String>,
    problems: &mut Vec<String>,
) -> Check {
    let base = match base_text {
        Ok(text) => text,
        Err(e) => {
            problems.push(format!("текст основания не проверен — {e}"));
            return Check::flagged(CHECK_TEXT_LOST, "основание не прочитано".to_string());
        }
    };
    let variant = match variant_text {
        Ok(text) => text,
        Err(e) => {
            problems.push(format!("текст варианта не проверен — {e}"));
            return Check::flagged(CHECK_TEXT_LOST, "вариант не прочитан".to_string());
        }
    };
    let base_words = word_counts(base);
    if base_words.is_empty() {
        return Check::ok(CHECK_TEXT_LOST, "в основании нет слов".to_string());
    }
    let variant_words = word_counts(variant);
    // Кратность (AG-28): если слово было в основании трижды, а осталось один
    // раз, потеряны два вхождения. На множествах такая потеря не видна.
    let lost: usize = base_words
        .iter()
        .map(|(word, count)| count.saturating_sub(*variant_words.get(word).unwrap_or(&0)))
        .sum();
    let total: usize = base_words.values().sum();
    let share = lost as f64 / total as f64;
    let percent = (share * 100.0).round() as u32;
    if share > TEXT_LOST_LIMIT {
        Check::flagged(
            CHECK_TEXT_LOST,
            format!("потеряно {percent} % слов (порог 40 %)"),
        )
    } else {
        Check::ok(CHECK_TEXT_LOST, format!("потеряно {percent} % слов"))
    }
}

/// Список меток для детали проверки: `[PERSON_4], [ORG_2]` (не больше
/// [`DETAIL_LABELS`], дальше — «и ещё N»).
fn join_labels(items: &BTreeSet<(String, u32)>) -> String {
    let mut out: Vec<String> = items
        .iter()
        .take(DETAIL_LABELS)
        .map(|(kind, number)| format!("[{kind}_{number}]"))
        .collect();
    if items.len() > DETAIL_LABELS {
        out.push(format!("и ещё {}", items.len() - DETAIL_LABELS));
    }
    out.join(", ")
}

#[cfg(test)]
mod tests {
    //! Проверки валидатора без Python: образ, вариант и основание — обычные
    //! текстовые файлы, поэтому NER и воркеры не нужны.
    use super::*;
    use crate::mirror::tests::{tmpdir, FakeExtractor};

    /// Каталог с файлами проверки (временный, со счётчиком — тесты параллельны).
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            Fixture { dir: tmpdir(tag) }
        }

        fn write(&self, name: &str, body: &str) -> PathBuf {
            let path = self.dir.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, body).unwrap();
            path
        }
    }

    /// Снимок меток проекта с нужными метками и `rev` (значения не нужны).
    fn snap(labels: &[&str], rev: i64) -> Snapshot {
        Snapshot {
            rev,
            labels: labels
                .iter()
                .map(|label| hds_anonymize::SnapshotLabel {
                    label: label.to_string(),
                    entity_type: String::new(),
                    forms: Vec::new(),
                })
                .collect(),
            others: Vec::new(),
            keep: Vec::new(),
        }
    }

    /// Ряд различных слов: `слово1 слово2 …` — доля потерь считается точно.
    fn series(n: usize) -> String {
        (1..=n)
            .map(|i| format!("слово{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Поддельное извлечение: содержимое файла как UTF-8.
    ///
    /// Настоящий `hds_extract` разбирает пакеты DOCX/PDF (это проверяет его
    /// собственный крейт), а валидатору важно лишь то, что текст docx/xlsx/pdf
    /// приходит от извлекателя, а txt/md/xml читаются напрямую.
    struct PlainExtractor {
        calls: usize,
    }

    impl PlainExtractor {
        fn new() -> PlainExtractor {
            PlainExtractor { calls: 0 }
        }
    }

    impl TextExtractor for PlainExtractor {
        fn extract(&mut self, path: &Path) -> Result<String, crate::mirror::MirrorError> {
            self.calls += 1;
            fs::read_to_string(path)
                .map_err(|e| crate::mirror::MirrorError::Io(path.to_path_buf(), e.to_string()))
        }
    }

    fn names(report: &Report) -> Vec<&str> {
        report.checks.iter().map(|c| c.name.as_str()).collect()
    }

    fn check<'a>(report: &'a Report, name: &str) -> &'a Check {
        report
            .checks
            .iter()
            .find(|c| c.name == name)
            .expect("проверка есть в отчёте")
    }

    #[test]
    fn word_counts_ignores_case_punctuation_and_service_lines() {
        let words = word_counts("<!-- страница 1 -->\nПривет, МИР! Привет — 1С:ERP\n");
        assert_eq!(words.len(), 4, "{words:?}");
        for expected in ["привет", "мир", "1с", "erp"] {
            assert!(words.contains_key(expected), "{expected} — {words:?}");
        }
        assert_eq!(words["привет"], 2, "кратность сохраняется: {words:?}");
        assert!(!words.contains_key("страница"), "служебная строка не слово");
        // Маркер образа — тоже служебная строка.
        let words = word_counts("<!-- hds-agent/P0001/7 -->\nтекст");
        assert_eq!(words.len(), 1, "{words:?}");
    }

    #[test]
    fn text_lost_counts_repeated_words_once_per_occurrence() {
        // AG-28: словарь основания сохранён, но половина вхождений потеряна —
        // на множествах это было «ок», на кратности — «проверить».
        let fix = Fixture::new("v-repeats");
        let base_text = "протокол ".repeat(10) + "решение";
        let base = fix.write("base.md", &base_text);
        let images = fix.write("base.md.images.json", "[]");
        let variant_text = "протокол ".repeat(4) + "решение";
        let variant = fix.write("variant.md", &variant_text);
        let mut ext = FakeExtractor::default();
        let outcome = validate(
            &Inputs {
                rel: "Документ.md",
                anon_rel: "Документ [DOC_1].md",
                kind: Kind::Image,
                image: &base,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&[], 3)),
            &mut ext,
        );
        let lost = check(&outcome.report, CHECK_TEXT_LOST);
        assert_eq!(
            lost.verdict,
            Verdict::Check,
            "потеряно 6 из 11 вхождений: {lost:?}"
        );
    }

    #[test]
    fn text_lost_threshold_is_forty_percent() {
        let fix = Fixture::new("v-text");
        let base = fix.write("base.md", &series(100));
        let images = fix.write("base.md.images.json", "[]");
        let mut ext = FakeExtractor::default();

        // 39 % пропавших слов — ещё не повод для «проверить».
        let variant_ok = fix.write("ok.md", &series(61));
        let outcome = validate(
            &Inputs {
                rel: "Документ.md",
                anon_rel: "Документ [DOC_1].md",
                kind: Kind::Image,
                image: &base,
                variant: &variant_ok,
                base: &base,
                images: &images,
            },
            Ok(snap(&[], 3)),
            &mut ext,
        );
        let lost = check(&outcome.report, CHECK_TEXT_LOST);
        assert_eq!(lost.verdict, Verdict::Ok, "{lost:?}");
        assert!(lost.detail.contains("потеряно 39 %"), "{lost:?}");

        // 41 % — документ пересобран, пометка «проверить».
        let variant_bad = fix.write("bad.md", &series(59));
        let outcome = validate(
            &Inputs {
                rel: "Документ.md",
                anon_rel: "Документ [DOC_1].md",
                kind: Kind::Image,
                image: &base,
                variant: &variant_bad,
                base: &base,
                images: &images,
            },
            Ok(snap(&[], 3)),
            &mut ext,
        );
        let lost = check(&outcome.report, CHECK_TEXT_LOST);
        assert_eq!(lost.verdict, Verdict::Check, "{lost:?}");
        assert!(lost.detail.contains("41 %"), "{lost:?}");
        assert!(lost.detail.contains("порог 40 %"), "{lost:?}");
        assert_eq!(outcome.report.verdict, Verdict::Check);
        assert!(
            outcome.problems.is_empty(),
            "пометка — не ошибка валидатора"
        );
    }

    #[test]
    fn text_lost_is_not_fooled_by_service_lines() {
        let fix = Fixture::new("v-pages");
        let base = fix.write(
            "base.md",
            "<!-- страница 1 -->\nПервый абзац\n<!-- страница 2 -->\nВторой абзац\n",
        );
        let variant = fix.write("variant.pdf.md", "первый абзац\n\nвторой абзац\n");
        let images = fix.write("base.pdf.images.json", "[]");
        let outcome = validate(
            &Inputs {
                rel: "Документ.pdf",
                anon_rel: "Документ [DOC_1].pdf.md",
                kind: Kind::TextImage,
                image: &variant,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&[], 1)),
            &mut FakeExtractor::default(),
        );
        let lost = check(&outcome.report, CHECK_TEXT_LOST);
        assert_eq!(lost.verdict, Verdict::Ok, "{lost:?}");
        assert!(lost.detail.contains("потеряно 0 %"), "{lost:?}");
    }

    #[test]
    fn labels_outside_project_flags_invented_label() {
        let fix = Fixture::new("v-labels");
        let image = fix.write(
            "Письмо [PERSON_1].md",
            "Контакт: [PERSON_1], [PERSON_9|дат], [IMAGE_1]\n",
        );
        let base = fix.write("base/Письмо.md", "Контакт: Иванов Иван\n");
        let variant = fix.write("pending/Письмо.md", "Контакт: Иванов Иван\n");
        let images = fix.write("base/Письмо.md.images.json", "[]");
        let outcome = validate(
            &Inputs {
                rel: "Письмо.md",
                anon_rel: "Письмо [PERSON_1].md",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[PERSON_1]", "[PERSON_2]"], 7)),
            &mut FakeExtractor::default(),
        );
        let labels = check(&outcome.report, CHECK_LABELS);
        assert_eq!(labels.verdict, Verdict::Check, "{labels:?}");
        assert!(labels.detail.contains("[PERSON_9]"), "{labels:?}");
        // `[PERSON_1]` — в проекте; `[IMAGE_1]` — картинка, не сущность.
        assert!(!labels.detail.contains("[PERSON_1]"), "{labels:?}");
        assert!(!labels.detail.contains("[IMAGE_1]"), "{labels:?}");
        assert_eq!(outcome.problems.len(), 0, "{:?}", outcome.problems);
    }

    #[test]
    fn label_grammar_needs_base_form_but_accepts_project_labels() {
        let fix = Fixture::new("v-grammar");
        let base = fix.write("base/Док.md", "Текст\n");
        let variant = fix.write("pending/Док.md", "Текст\n");
        let images = fix.write("base/Док.md.images.json", "[]");
        let mut ext = FakeExtractor::default();

        // `[PERSON_2|дат]` есть в снимке проекта — базовая форма известна.
        let image = fix.write("Док [PERSON_2].md", "[PERSON_2|дат], [PERSON_2]\n");
        let outcome = validate(
            &Inputs {
                rel: "Док.md",
                anon_rel: "Док [PERSON_2].md",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[PERSON_2]"], 5)),
            &mut ext,
        );
        assert_eq!(
            check(&outcome.report, CHECK_GRAMMAR).verdict,
            Verdict::Ok,
            "{:?}",
            check(&outcome.report, CHECK_GRAMMAR)
        );
        assert_eq!(check(&outcome.report, CHECK_LABELS).verdict, Verdict::Ok);

        // `[ORG_4|дат]` без базовой формы ни в документе, ни в снимке.
        let image = fix.write("Док [ORG_4].md", "[ORG_4|дат]\n");
        let outcome = validate(
            &Inputs {
                rel: "Док.md",
                anon_rel: "Док [ORG_4].md",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[PERSON_2]"], 5)),
            &mut ext,
        );
        let grammar = check(&outcome.report, CHECK_GRAMMAR);
        assert_eq!(grammar.verdict, Verdict::Check, "{grammar:?}");
        assert!(grammar.detail.contains("[ORG_4|дат]"), "{grammar:?}");
        assert!(grammar.detail.contains("базовой формы"), "{grammar:?}");
        // Та же метка выдумана — обе проверки независимы.
        assert_eq!(check(&outcome.report, CHECK_LABELS).verdict, Verdict::Check);

        // Базовая форма есть в документе — падеж согласован, даже если метки
        // нет в снимке (её поймает `labels_outside_project`).
        let image = fix.write("Док [ORG_5].md", "[ORG_5] — [ORG_5|дат]\n");
        let outcome = validate(
            &Inputs {
                rel: "Док.md",
                anon_rel: "Док [ORG_5].md",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&[], 5)),
            &mut ext,
        );
        assert_eq!(check(&outcome.report, CHECK_GRAMMAR).verdict, Verdict::Ok);
        assert_eq!(check(&outcome.report, CHECK_LABELS).verdict, Verdict::Check);
    }

    #[test]
    fn label_grammar_accepts_nominative_code() {
        let fix = Fixture::new("v-nomn");
        let image = fix.write("Док [PERSON_1].md", "[PERSON_1|nomn] и [PERSON_1|,мн]\n");
        let base = fix.write("base/Док.md", "Текст\n");
        let variant = fix.write("pending/Док.md", "Текст\n");
        let images = fix.write("base/Док.md.images.json", "[]");
        let outcome = validate(
            &Inputs {
                rel: "Док.md",
                anon_rel: "Док [PERSON_1].md",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[PERSON_1]"], 2)),
            &mut FakeExtractor::default(),
        );
        assert_eq!(check(&outcome.report, CHECK_GRAMMAR).verdict, Verdict::Ok);
    }

    #[test]
    fn unplaced_images_flags_labels_left_in_variant() {
        let fix = Fixture::new("v-images");
        let image = fix.write("Док [DOC_1].docx", "[DOC_1], [IMAGE_1]\n");
        let base = fix.write("base/Док.docx", "Текст\n");
        // Карта основания знает `[IMAGE_1]` — картинка должна была вернуться.
        let images = fix.write(
            "base/Док.docx.images.json",
            r#"[{"label": "[IMAGE_1]", "part": "word/media/image1.png"}]"#,
        );
        let variant = fix.write("pending/Док.docx", "Текст [IMAGE_1]\n");
        let mut ext = PlainExtractor::new();
        let outcome = validate(
            &Inputs {
                rel: "Док.docx",
                anon_rel: "Док [DOC_1].docx",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[DOC_1]"], 4)),
            &mut ext,
        );
        let images_check = check(&outcome.report, CHECK_IMAGES);
        assert_eq!(images_check.verdict, Verdict::Check, "{images_check:?}");
        assert!(
            images_check.detail.contains("[IMAGE_1]"),
            "{images_check:?}"
        );
        assert!(
            images_check.detail.contains("не восстановлена"),
            "{images_check:?}"
        );

        // Метка есть в варианте, но в карте её нет — такой картинки не было.
        let images = fix.write("base/Док.docx.images.json", "[]");
        let outcome = validate(
            &Inputs {
                rel: "Док.docx",
                anon_rel: "Док [DOC_1].docx",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[DOC_1]"], 4)),
            &mut ext,
        );
        let images_check = check(&outcome.report, CHECK_IMAGES);
        assert_eq!(images_check.verdict, Verdict::Check, "{images_check:?}");
        assert!(
            images_check.detail.contains("нет в карте основания"),
            "{images_check:?}"
        );

        // Картинка восстановлена — в тексте варианта меток нет.
        let variant = fix.write("pending/Док.docx", "Текст с картинкой\n");
        let outcome = validate(
            &Inputs {
                rel: "Док.docx",
                anon_rel: "Док [DOC_1].docx",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[DOC_1]"], 4)),
            &mut ext,
        );
        assert_eq!(
            check(&outcome.report, CHECK_IMAGES).verdict,
            Verdict::Ok,
            "{:?}",
            check(&outcome.report, CHECK_IMAGES)
        );
    }

    #[test]
    fn unplaced_images_not_applicable_for_text_image() {
        let fix = Fixture::new("v-text-image");
        let base = fix.write("base/Док.pdf", "Текст\n");
        let variant = fix.write("pending/Док.pdf.md", "Текст [IMAGE_1]\n");
        let images = fix.write("base/Док.pdf.images.json", "[]");
        let outcome = validate(
            &Inputs {
                rel: "Док.pdf",
                anon_rel: "Док [DOC_1].pdf.md",
                kind: Kind::TextImage,
                image: &variant,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&[], 1)),
            &mut PlainExtractor::new(),
        );
        let images_check = check(&outcome.report, CHECK_IMAGES);
        assert_eq!(images_check.verdict, Verdict::Ok, "{images_check:?}");
        assert!(
            images_check.detail.starts_with("не применимо"),
            "{images_check:?}"
        );
    }

    #[test]
    fn report_is_written_in_order_and_without_values() {
        let fix = Fixture::new("v-report");
        let image = fix.write("Письмо [PERSON_1].md", "Контакт: [PERSON_1], [PERSON_9]\n");
        let base = fix.write("base/Письмо.md", "Контакт: Иванов Иван\n");
        let variant = fix.write("pending/Письмо.md", "Контакт: Иванов Иван, правка\n");
        let images = fix.write("base/Письмо.md.images.json", "[]");
        let outcome = validate(
            &Inputs {
                rel: "Письмо.md",
                anon_rel: "Письмо [PERSON_1].md",
                kind: Kind::Image,
                image: &image,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Ok(snap(&["[PERSON_1]"], 7)),
            &mut FakeExtractor::default(),
        );
        let report = &outcome.report;
        assert_eq!(report.verdict, Verdict::Check, "метка вне проекта");
        assert_eq!(report.rev, 7);
        assert_eq!(report.rel, "Письмо.md");
        assert_eq!(report.anon_rel, "Письмо [PERSON_1].md");
        assert!(report.ts > 0.0);
        assert_eq!(
            names(report),
            vec![CHECK_LABELS, CHECK_GRAMMAR, CHECK_IMAGES, CHECK_TEXT_LOST],
            "порядок проверок — контракт отчёта"
        );

        let path = report.write(&variant).expect("отчёт записан");
        assert_eq!(path, report_path(&variant));
        let body = fs::read_to_string(&path).unwrap();
        assert!(!body.contains("Иванов"), "значения в отчёте (I8): {body}");
        assert!(!body.contains("правка"), "текст документа в отчёте (I8)");
        assert!(body.contains("\"verdict\": \"check\""), "{body}");
        assert!(body.contains("\"rev\": 7"), "{body}");

        // Отчёт читается обратно (так его увидит A5). `ts` сравнивается с
        // допуском: разбор JSON во float не обязан давать тот же бит (1 ULP).
        let read = Report::read(&variant).expect("отчёт разбирается");
        assert_eq!(read.rel, report.rel);
        assert_eq!(read.anon_rel, report.anon_rel);
        assert_eq!(read.rev, report.rev);
        assert_eq!(read.verdict, report.verdict);
        assert_eq!(read.checks, report.checks);
        assert!(
            (read.ts - report.ts).abs() < 0.001,
            "{} vs {}",
            read.ts,
            report.ts
        );
    }

    #[test]
    fn validator_failures_become_check_and_problems() {
        let fix = Fixture::new("v-errors");
        let base = fix.write("base/Док.md", "Текст документа\n");
        let images = fix.write("base/Док.md.images.json", "[]");
        let missing = fix.dir.join("нет такого.md");
        let outcome = validate(
            &Inputs {
                rel: "Док.md",
                anon_rel: "Док [DOC_1].md",
                kind: Kind::Image,
                image: &missing,
                variant: &missing,
                base: &base,
                images: &images,
            },
            Ok(snap(&[], 1)),
            &mut FakeExtractor::default(),
        );
        assert_eq!(outcome.report.verdict, Verdict::Check);
        assert_eq!(check(&outcome.report, CHECK_LABELS).verdict, Verdict::Check);
        assert_eq!(
            check(&outcome.report, CHECK_TEXT_LOST).verdict,
            Verdict::Check
        );
        assert!(
            !outcome.problems.is_empty(),
            "сбой проверки виден в problems"
        );
        assert!(
            outcome.problems.iter().any(|p| p.contains("нет файла")),
            "{:?}",
            outcome.problems
        );

        // Снимок недоступен: `rev` = 0, метки не проверяются, текст проверяется.
        let variant = fix.write("pending/Док.md", "Текст документа\n");
        let outcome = validate(
            &Inputs {
                rel: "Док.md",
                anon_rel: "Док [DOC_1].md",
                kind: Kind::Image,
                image: &variant,
                variant: &variant,
                base: &base,
                images: &images,
            },
            Err("воркер не ответил".to_string()),
            &mut FakeExtractor::default(),
        );
        assert_eq!(outcome.report.rev, 0);
        assert_eq!(check(&outcome.report, CHECK_LABELS).verdict, Verdict::Check);
        assert_eq!(check(&outcome.report, CHECK_TEXT_LOST).verdict, Verdict::Ok);
        assert!(outcome
            .problems
            .iter()
            .any(|p| p.contains("снимок меток проекта")));
    }

    #[test]
    fn unsupported_format_is_reported_not_guessed() {
        let fix = Fixture::new("v-xls");
        let path = fix.write("Док.xls", "не проверяется");
        let mut ext = FakeExtractor::default();
        assert!(read_text(&path, &mut ext)
            .unwrap_err()
            .contains("не проверяется"));
        assert_eq!(ext.calls, 0, "извлекатель не зовётся для чужих форматов");
    }
}
