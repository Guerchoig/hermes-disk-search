//! Решения по вариантам агента — «Принять»/«Отклонить»/«Конфликт» (TR-7.6, A4.4).
//!
//! Вариант появляется сам (A4.3: `.hds-agent\pending\<rel>` + снимок основания),
//! а решение принимает пользователь. Здесь — **ядро** решений: библиотека для
//! CLI сейчас и для кнопок вкладки «Агент» в A5. UI-логики (диалог сравнения,
//! Word-compare) тут нет намеренно.
//!
//! Четыре ситуации плана §5.7 и что делает код:
//!
//! 1. **«Принять», оригинал не менялся** ([`Decision::Accept`]): копия
//!    оригинала в `.hds-agent\backup\<дата-время>\<rel>` → атомарная замена
//!    оригинала вариантом ([`hds_core::config::replace_file`]) → статус
//!    `accepted` → образ пересоздаётся из **нового** оригинала (issued/seen
//!    заново) → вариант и основание удаляются.
//! 2. **Конфликт** (оригинал изменился, TR-7.6/2) — [`Decision::Conflict`] с
//!    тремя исходами: «оставить вариант агента» (как п.1, с предупреждением о
//!    потере правок и обязательным backup), «взять новую версию» (вариант
//!    удаляется, образ — из нового оригинала) и «поручить агенту» (образ из
//!    нового оригинала, анонимная копия прежнего варианта — в
//!    `<P>\workspace\_conflicts\<имя> (вариант агента).<ext>`, агенту сообщение
//!    «перенеси правки», новое основание).
//! 3. **Файл занят** (Word/Excel держит оригинал): замена невозможна —
//!    [`ResultsError::Busy`], backup **не остаётся** (созданную копию
//!    откатываем), вариант в `pending` и статус не меняются.
//! 4. **«Отклонить»** ([`Decision::Reject`]): образ возвращается к оригиналу,
//!    вариант и основание удаляются, агенту уходит служебное сообщение.
//!
//! Сообщения агенту — очередь `.hds-agent\messages.jsonl` (решение В-6: A4
//! пишет, A5 доставляет в сессию). В сообщении только **метки и пути**: имя
//! образа (`[PERSON_1]`), никогда не значения (I8) — иначе текст сообщения
//! уносил бы в анонимную зону то, что она и должна скрывать.
//!
//! Порядок операций при «Принять» обязателен (backup → замена → статус →
//! образ → очистка): если замена не удалась, не удаляется ничего. Единственное
//! отклонение от буквы плана — «Отклонить» пересоздаёт образ **до** удаления
//! варианта: сбой воркера не должен стоить пользователю последнего варианта.
//!
//! A6.4 добавляет эксплуатационные операции: [`Mirror::backups`] (список копий
//! `.hds-agent\backup\<stamp>\<rel>`), [`Mirror::restore`] («Вернуть» —
//! выбранный штамп или свежая копия), [`Mirror::rebuild`] («Пересоздать образ»
//! для `orphan`) и [`Mirror::relink`] (починка записи по маркеру образа —
//! **только по команде**, в проходе зеркала не вызывается, В-A6-5). Журнал
//! решений — `.hds-agent\decisions.jsonl` (JSONL, ротация при 1 МБ, В-A6-6):
//! только пути, статусы и числа (I8), `hds agent history clear` его не трогает.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use hds_core::config::replace_file;
use hds_core::error::CoreError;
use hds_core::timefmt::{now_epoch, stamp_local};
use serde::{Deserialize, Serialize};

use crate::mirror::{
    copy_file, kind_for, rel_to_path, sha256_file, split_name, split_path, walk_files, Mirror,
    MirrorError, ScannedFile,
};
use crate::registry_db::{Artifact, Kind, RegistryError, Status};

/// Служебный каталог проекта (TR-7.1): не зеркалится и не индексируется.
pub const SERVICE_DIR: &str = ".hds-agent";
/// Очередь сообщений агенту внутри [`SERVICE_DIR`] (решение В-6).
pub const MESSAGES_FILE: &str = "messages.jsonl";
/// Копии прежних версий оригиналов: подкаталог [`SERVICE_DIR`] (TR-7.6).
pub const BACKUP_SUBDIR: &str = "backup";
/// Журнал решений `.hds-agent\decisions.jsonl` (В-A6-6).
pub const DECISIONS_FILE: &str = "decisions.jsonl";
/// Ротация журнала решений: больше этого — в `decisions.jsonl.1` (В-A6-6).
pub const DECISIONS_ROTATE_BYTES: u64 = 1024 * 1024;
/// Каталог workspace для вариантов при конфликте (TR-7.1/7.6).
pub const CONFLICTS_DIR: &str = "_conflicts";
/// Суффикс анонимной копии варианта агента в [`CONFLICTS_DIR`].
pub const CONFLICT_SUFFIX: &str = " (вариант агента)";

/// «Сравнить» (A5.4, В-A5-4): сколько строк diff отдаём вкладке.
pub const DIFF_MAX_LINES: usize = 200;
/// «Сравнить»: потолок символов в diff (панель не должна получать мегабайты).
pub const DIFF_MAX_CHARS: usize = 2000;
/// Сколько строк каждой стороны участвует в diff (защита от гигантских файлов).
pub const DIFF_SCAN_LINES: usize = 400;
/// Потолок длины одной строки diff.
pub const DIFF_LINE_CHARS: usize = 200;

/// Пути варианта и основания документа (A5.4).
///
/// Вкладка «Агент» не склеивает `.hds-agent\pending\…` сама: правило §5
/// «статусы и пути — только из ядра». Свободная функция, потому что панели
/// «Результаты» пути нужны без зеркала (воркеры для этого не поднимаются).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantPaths {
    /// Оригинал в папке проекта.
    pub original: PathBuf,
    /// Вариант агента: `.hds-agent\pending\<rel>`.
    pub pending: PathBuf,
    /// Снимок основания: `.hds-agent\base\<rel>`.
    pub base: PathBuf,
    /// Отчёт валидатора рядом с вариантом: `<rel>.report.json`.
    pub report: PathBuf,
}

/// Пути варианта и основания для относительного пути оригинала.
pub fn variant_paths(project_root: &Path, orig_rel: &str) -> VariantPaths {
    let service = project_root.join(SERVICE_DIR);
    let tail = rel_to_path(orig_rel);
    let pending = service.join("pending").join(&tail);
    VariantPaths {
        original: project_root.join(&tail),
        report: crate::validator::report_path(&pending),
        base: service.join("base").join(tail),
        pending,
    }
}

/// Исход конфликта (TR-7.6, случай 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// «Оставить вариант агента»: оригинал заменяется вариантом, правки
    /// пользователя, сделанные после основания, теряются (копия — в backup).
    Agent,
    /// «Взять новую версию»: вариант отбрасывается, образ — из нового оригинала.
    New,
    /// «Поручить агенту»: образ из нового оригинала, вариант — в `_conflicts`.
    Delegate,
}

impl ConflictChoice {
    /// Разбор `--choice` (`agent`/`new`/`delegate`); `None` — неизвестное слово.
    pub fn parse(s: &str) -> Option<ConflictChoice> {
        match s.trim().to_ascii_lowercase().as_str() {
            "agent" => Some(ConflictChoice::Agent),
            "new" => Some(ConflictChoice::New),
            "delegate" => Some(ConflictChoice::Delegate),
            _ => None,
        }
    }

    /// Строка для журнала и CLI.
    pub fn as_str(self) -> &'static str {
        match self {
            ConflictChoice::Agent => "agent",
            ConflictChoice::New => "new",
            ConflictChoice::Delegate => "delegate",
        }
    }
}

/// Решение пользователя по варианту.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// «Принять» (ситуация 1).
    Accept,
    /// «Отклонить» (ситуация 4).
    Reject,
    /// Разрешение конфликта (ситуация 2).
    Conflict(ConflictChoice),
}

impl Decision {
    /// Разбор решения из тела API: `accept` | `reject` | `conflict`
    /// (для конфликта обязателен `choice`: `agent` | `new` | `delegate`).
    ///
    /// `None` — неизвестное слово: вкладка отвечает 400, а не догадывается.
    pub fn parse(decision: &str, choice: Option<&str>) -> Option<Decision> {
        let action = decision.trim().to_ascii_lowercase();
        // Допускаем и слитую форму `conflict:agent` (так решение пишется в журнал).
        if let Some(rest) = action.strip_prefix("conflict:") {
            return ConflictChoice::parse(rest).map(Decision::Conflict);
        }
        match action.as_str() {
            "accept" => Some(Decision::Accept),
            "reject" => Some(Decision::Reject),
            "conflict" => ConflictChoice::parse(choice.unwrap_or("")).map(Decision::Conflict),
            _ => None,
        }
    }

    /// Строка для журнала и CLI: `accept`, `reject`, `conflict:<choice>`.
    pub fn as_str(self) -> String {
        match self {
            Decision::Accept => "accept".to_string(),
            Decision::Reject => "reject".to_string(),
            Decision::Conflict(choice) => format!("conflict:{}", choice.as_str()),
        }
    }
}

/// Вид сообщения агенту (очередь `.hds-agent\messages.jsonl`, В-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Правки не приняты, образ восстановлен/пересоздан из оригинала.
    DocumentRejected,
    /// Документ обновлён пользователем, правки переносить из `_conflicts`.
    ConflictDelegated,
    /// Метки склеены пользователем: младшие теперь обозначаются старшей (A5.5).
    LabelsMerged,
    /// Склейка меток отменена: метки снова самостоятельные (A5.5).
    LabelsUnmerged,
}

impl MessageKind {
    /// Строка для БД/журнала и тестов.
    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::DocumentRejected => "document_rejected",
            MessageKind::ConflictDelegated => "conflict_delegated",
            MessageKind::LabelsMerged => "labels_merged",
            MessageKind::LabelsUnmerged => "labels_unmerged",
        }
    }

    /// Разбор строки из очереди (A5 будет доставлять эти же записи).
    pub fn parse(s: &str) -> Option<MessageKind> {
        match s.trim() {
            "document_rejected" => Some(MessageKind::DocumentRejected),
            "conflict_delegated" => Some(MessageKind::ConflictDelegated),
            "labels_merged" => Some(MessageKind::LabelsMerged),
            "labels_unmerged" => Some(MessageKind::LabelsUnmerged),
            _ => None,
        }
    }
}

/// Запись очереди сообщений агенту (В-6): по строке JSON на событие.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Unix-время (секунды с дробной частью).
    pub ts: f64,
    /// Вид события.
    pub kind: MessageKind,
    /// Относительный путь **образа** (метки, без значений — I8).
    pub rel: String,
    /// Текст для агента: путь и метки, без значений (I8).
    pub text: String,
}

/// Что сделано — для вывода CLI, журнала и будущих кнопок A5.
#[derive(Debug, Clone)]
pub struct DecisionReport {
    /// Относительный путь оригинала (папка проекта).
    pub orig_rel: String,
    /// Относительный путь образа (workspace), каким он стал после решения.
    pub anon_rel: String,
    /// Итоговый статус записи реестра.
    pub status: Status,
    /// Копия прежнего оригинала (нет у «Отклонить» и у «Взять новую версию»).
    pub backup: Option<PathBuf>,
    /// Штамп каталога копии (`YYYYMMDD-HHMMSS`): по нему «Вернуть» выбирает
    /// версию (A6.4, В-A6-4).
    pub stamp: Option<String>,
    /// sha256 того, что операция записала (восстановленный оригинал или новый
    /// образ): пользователь видит, что вернулось именно выбранное (A6.4).
    pub sha256: Option<String>,
    /// Анонимная копия варианта в `_conflicts` («Поручить агенту»).
    pub conflict_copy: Option<String>,
    /// Сообщение, положенное в очередь агенту.
    pub message: Option<Message>,
    /// Предупреждения пользователю (пути/метки, без значений — I8).
    pub warnings: Vec<String>,
}

impl DecisionReport {
    fn new(row: &Artifact) -> DecisionReport {
        DecisionReport {
            orig_rel: row.orig_rel.clone(),
            anon_rel: row.anon_rel.clone(),
            status: row.status,
            backup: None,
            stamp: None,
            sha256: None,
            conflict_copy: None,
            message: None,
            warnings: Vec::new(),
        }
    }
}

/// Образ, пересозданный после склейки меток (A5.5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RebuiltImage {
    /// Относительный путь оригинала (папка проекта).
    pub rel: String,
    /// Относительный путь образа (workspace).
    pub anon_rel: String,
    pub status: Status,
}

/// Итог пересборки образов после склейки/разъединения меток (A5.5).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RefreshReport {
    pub rebuilt: Vec<RebuiltImage>,
    /// Оригиналы, которые не пересобирались: вариант, конфликт или правка
    /// агента — пересборка потеряла бы их (В-A5.5-5).
    pub skipped: Vec<String>,
}

impl RefreshReport {
    /// Ситуации, в которых пересборка безопасна: образ совпадает с тем, что
    /// выпустило зеркало, и варианта в `pending` нет.
    fn rebuildable(status: Status) -> bool {
        matches!(status, Status::Synced | Status::Accepted)
    }
}

/// Одна сторона сравнения: путь, наличие, размер, sha256 и время изменения.
#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    /// Абсолютный путь (зона пользователя: папка проекта/workspace).
    pub path: String,
    pub exists: bool,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    /// Unix-время изменения (секунды с дробной частью).
    pub mtime: Option<f64>,
}

/// Отчёт «Сравнить» (TR-7.6, В-A5-4): метаданные обеих сторон, отчёт
/// валидатора (TR-7.8) и текстовый diff в пределах [`DIFF_MAX_LINES`] /
/// [`DIFF_MAX_CHARS`].
///
/// Word-compare и ячеечный diff xlsx — фаза 2 (долг §4.2 п.7). Здесь документ
/// читается через `hds_extract` и сравнивается построчно: для панели этого
/// достаточно, чтобы увидеть, что именно изменил агент.
#[derive(Debug, Clone, Serialize)]
pub struct CompareReport {
    pub rel: String,
    pub anon_rel: String,
    pub kind: Kind,
    pub status: Status,
    /// Оригинал в папке проекта.
    pub original: FileInfo,
    /// Вариант агента в `.hds-agent\pending`.
    pub variant: FileInfo,
    /// Снимок основания.
    pub base: FileInfo,
    /// Образ в workspace (то, что видит агент).
    pub image: FileInfo,
    /// Отчёт валидатора рядом с вариантом (`None` — отчёта нет).
    pub report: Option<crate::validator::Report>,
    /// Совпадает ли содержимое оригинала и варианта (sha256).
    pub identical: bool,
    /// Построчный diff: `"  "` — общая строка, `"- "` — было, `"+ "` — стало.
    pub diff: Vec<String>,
    /// Diff обрезан лимитом (вкладка подписывает «показано N строк»).
    pub diff_truncated: bool,
    /// Почему diff пуст (файл не читается, формата нет в `hds_extract`).
    pub diff_note: Option<String>,
}

/// Почему решение не выполнено. Текст — для пользователя: что случилось и что
/// делать (CLI печатает его как есть, код возврата 1).
#[derive(Debug)]
pub enum ResultsError {
    /// Нет записи реестра по этому пути.
    NotFound(String),
    /// Запись есть, но решать нечего (не `pending_accept`/`conflict`).
    WrongStatus {
        rel: String,
        status: Status,
    },
    /// Варианта нет в `.hds-agent\pending`.
    NoVariant(String),
    /// Оригинал изменился после создания образа — «Принять» нельзя.
    OriginalChanged(String),
    /// Правок агента нет — делегировать нечего.
    NothingToDelegate(String),
    /// Файл занят (Word/Excel): замена оригинала невозможна.
    Busy(PathBuf),
    /// Путь не является относительным путём внутри проекта.
    BadPath(String),
    /// Вид артефакта не допускает замены оригинала (текстовый образ).
    Kind(Kind),
    /// Копии нет в `.hds-agent\backup` (в том числе неизвестный `--stamp`).
    NoBackup {
        rel: String,
        stamp: Option<String>,
        /// Какие штампы есть — чтобы пользователь выбрал существующий.
        available: Vec<String>,
    },
    /// Операция неприменима к текущему статусу записи (`op`: `rebuild`/`relink`).
    NotApplicable {
        rel: String,
        status: Status,
        op: &'static str,
    },
    /// Образа нет в workspace — восстанавливать запись не из чего.
    NoImage(String),
    /// Маркер образа не наш: чужой проект, чужой `artifact_id` или его нет.
    BadMarker(String),
    Io(PathBuf, String),
    Registry(RegistryError),
    Mirror(MirrorError),
}

impl std::fmt::Display for ResultsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResultsError::NotFound(rel) => write!(
                f,
                "'{rel}': нет записи в реестре — документ не зеркалился (hds agent sync)"
            ),
            ResultsError::WrongStatus { rel, status } => {
                write!(f, "'{rel}': статус {}, решать нечего", status.as_str())?;
                match status {
                    Status::AgentChanged => {
                        write!(f, " — вариант ещё не собран, запустите hds agent sync")
                    }
                    Status::Conflict => write!(
                        f,
                        " — это конфликт: hds agent result conflict --choice agent|new|delegate"
                    ),
                    Status::Synced | Status::Accepted => {
                        write!(f, " — правок агента нет, документ синхронизирован")
                    }
                    _ => Ok(()),
                }
            }
            ResultsError::NoVariant(rel) => write!(
                f,
                "'{rel}': варианта нет в {SERVICE_DIR}\\pending — принимать нечего \
                 (запустите hds agent sync)"
            ),
            ResultsError::OriginalChanged(rel) => write!(
                f,
                "'{rel}': оригинал изменился после создания образа — принимайте через \
                 hds agent result conflict --choice agent (иначе правки пользователя \
                 потерялись бы без предупреждения)"
            ),
            ResultsError::NothingToDelegate(rel) => write!(
                f,
                "'{rel}': правок агента нет — делегировать нечего (образ совпадает с выпущенным)"
            ),
            ResultsError::Busy(path) => write!(
                f,
                "файл занят ({}): закройте его в Word/Excel и повторите — состояние не изменено",
                path.display()
            ),
            ResultsError::BadPath(rel) => write!(
                f,
                "'{rel}': нужен относительный путь внутри папки проекта (без '..' и диска)"
            ),
            ResultsError::Kind(kind) => write!(
                f,
                "вариант вида {} принять нельзя: оригинал такого формата агент не меняет \
                 (TR-7.3a) — используйте reject",
                kind.as_str()
            ),
            ResultsError::NoBackup {
                rel,
                stamp,
                available,
            } => {
                match stamp {
                    Some(stamp) => write!(
                        f,
                        "'{rel}': копии '{stamp}' нет в {SERVICE_DIR}\\{BACKUP_SUBDIR}"
                    )?,
                    None => write!(
                        f,
                        "'{rel}': копий в {SERVICE_DIR}\\{BACKUP_SUBDIR} нет — вернуть нечего \
                         (копия делается при «Принять»)"
                    )?,
                }
                if !available.is_empty() {
                    write!(f, " — доступны: {}", available.join(", "))?;
                }
                Ok(())
            }
            ResultsError::NotApplicable { rel, status, op } => match *op {
                "rebuild" => write!(
                    f,
                    "'{rel}': статус {} — «Пересоздать образ» можно только для orphan \
                     (образ удалён или оригинал заменён агентом)",
                    status.as_str()
                ),
                _ => write!(
                    f,
                    "'{rel}': статус {} — запись восстанавливают только из orphan/error \
                     (остальные обновляет hds agent sync)",
                    status.as_str()
                ),
            },
            ResultsError::NoImage(rel) => write!(
                f,
                "'{rel}': образа нет в workspace — восстанавливать запись не из чего \
                 (запустите hds agent sync)"
            ),
            ResultsError::BadMarker(rel) => write!(
                f,
                "'{rel}': маркер образа не наш (нужен hds-agent/<P>/<id> этого проекта) — \
                 запись не восстановлена"
            ),
            ResultsError::Io(path, why) => write!(f, "{}: {why}", path.display()),
            ResultsError::Registry(err) => write!(f, "{err}"),
            ResultsError::Mirror(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ResultsError {}

impl From<RegistryError> for ResultsError {
    fn from(err: RegistryError) -> ResultsError {
        ResultsError::Registry(err)
    }
}

impl From<MirrorError> for ResultsError {
    fn from(err: MirrorError) -> ResultsError {
        ResultsError::Mirror(err)
    }
}

/// Нормализовать путь из CLI: `a/b` и `a\b` — одно и то же, `..` запрещён.
pub fn normalize_rel(rel: &str) -> Result<String, ResultsError> {
    let parts = split_path(rel);
    if parts.is_empty() || parts.iter().any(|part| part == ".." || part.contains(':')) {
        return Err(ResultsError::BadPath(rel.to_string()));
    }
    Ok(parts.join(std::path::MAIN_SEPARATOR_STR))
}

/// Файл «доставленные сообщения»: очередь не удаляется, а переезжает сюда
/// (аудит решений, AG-20).
pub const DELIVERED_FILE: &str = "messages.delivered.jsonl";

/// Прочитать очередь сообщений агенту (`.hds-agent\messages.jsonl`).
///
/// **AG-20.** Очередь писалась (`decide`, склейки меток), но её никто не читал:
/// агент не узнавал ни об отклонённых правках, ни о перенесённых метках.
/// Битые строки пропускаются — очередь не должна ломать старт сессии.
pub fn read_queue(project_root: &Path) -> Vec<Message> {
    let path = project_root.join(SERVICE_DIR).join(MESSAGES_FILE);
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Message>(line).ok())
        .collect()
}

/// Пометить очередь доставленной: строки переезжают в
/// `.hds-agent\messages.delivered.jsonl`, очередь очищается.
///
/// Так сообщение не повторится в следующем ходе (`initialMessages` передаются
/// только при открытии сессии), но останется в зоне пользователя для аудита.
pub fn mark_delivered(project_root: &Path) -> Result<(), ResultsError> {
    let dir = project_root.join(SERVICE_DIR);
    let queue = dir.join(MESSAGES_FILE);
    let delivered = dir.join(DELIVERED_FILE);
    let text = match std::fs::read_to_string(&queue) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(ResultsError::Io(queue, e.to_string())),
    };
    if text.trim().is_empty() {
        let _ = std::fs::remove_file(&queue);
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&delivered)
        .map_err(|e| ResultsError::Io(delivered.clone(), e.to_string()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| ResultsError::Io(delivered, e.to_string()))?;
    fs::remove_file(&queue).map_err(|e| ResultsError::Io(queue, e.to_string()))
}

/// Текст реплики для агента из очереди: заголовок + строки сообщений.
///
/// В сообщениях только метки и пути (I8), значения не добавляются. `None` —
/// очередь пуста (передавать агенту нечего).
pub fn queue_prompt(messages: &[Message]) -> Option<String> {
    if messages.is_empty() {
        return None;
    }
    let mut out =
        String::from("Служебные сообщения от пользователя (после его последнего запроса):\n");
    for message in messages {
        out.push_str("- ");
        out.push_str(&message.text);
        out.push('\n');
    }
    out.push_str(
        "Учти их в работе: если документ отклонён или заменён — не считай свой образ \
         актуальным; если метки склеены — используй старшую.\n",
    );
    Some(out)
}

/// Дописать сообщение агенту в `.hds-agent\messages.jsonl` (В-6, A5.5).
///
/// Очередь общая для решений по документам и для склеек меток: у меток
/// `rel` пустой, текст — только метки («метки [PERSON_5] теперь обозначаются
/// [PERSON_3]»), значений в нём быть не должно (I8).
pub fn append_message(
    project_root: &Path,
    kind: MessageKind,
    rel: &str,
    text: String,
) -> Result<Message, ResultsError> {
    let message = Message {
        ts: now_epoch(),
        kind,
        rel: rel.to_string(),
        text,
    };
    let dir = project_root.join(SERVICE_DIR);
    fs::create_dir_all(&dir).map_err(|e| ResultsError::Io(dir.clone(), e.to_string()))?;
    let path = dir.join(MESSAGES_FILE);
    let mut line = serde_json::to_string(&message)
        .map_err(|e| ResultsError::Io(path.clone(), e.to_string()))?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| ResultsError::Io(path.clone(), e.to_string()))?;
    file.write_all(line.as_bytes())
        .map_err(|e| ResultsError::Io(path, e.to_string()))?;
    Ok(message)
}

/// Одна копия прежней версии оригинала (A6.4, В-A6-4).
///
/// `sha256` считается только при выборке по конкретному файлу: список всех
/// копий проекта не должен читать гигабайты ради панели.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BackupCopy {
    /// Штамп каталога копии: `YYYYMMDD-HHMMSS` (возможен суффикс `-2`).
    pub stamp: String,
    /// Относительный путь оригинала (как в реестре).
    pub rel: String,
    /// Абсолютный путь копии.
    pub path: PathBuf,
    pub size: u64,
    /// Unix-время изменения копии (секунды с дробной частью).
    pub mtime: f64,
    /// sha256 копии (есть, когда копии выбирались по `rel`).
    pub sha256: Option<String>,
}

/// Запись журнала решений `.hds-agent\decisions.jsonl` (В-A6-6).
///
/// Только пути, статусы и числа (I8): значений из базы меток тут быть не может
/// по построению — все поля собираются из реестра и путей.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionEntry {
    /// Unix-время записи.
    pub ts: f64,
    /// `accept`, `reject`, `conflict:*`, `restore`, `rebuild`, `relink`.
    pub action: String,
    /// Относительный путь оригинала.
    pub orig_rel: String,
    /// Штамп использованной копии (restore / accept / conflict:agent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stamp: Option<String>,
    /// Итоговый статус записи реестра.
    pub status: String,
    /// Относительный путь образа после операции.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Числовой довесок (`sha256=…`, `artifact_id=…`) — без значений (I8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
}

/// Дописать строку в `.hds-agent\decisions.jsonl` с ротацией при 1 МБ.
///
/// Ротация — как у `sync.log` (`LOG_ROTATE_BYTES`): прежний файл уезжает в
/// `decisions.jsonl.1` и заменяет его. Неудача записи не должна отменять само
/// решение: вызывающий превращает её в предупреждение.
pub fn append_decision(project_root: &Path, entry: &DecisionEntry) -> Result<(), ResultsError> {
    let dir = project_root.join(SERVICE_DIR);
    fs::create_dir_all(&dir).map_err(|e| ResultsError::Io(dir.clone(), e.to_string()))?;
    let path = dir.join(DECISIONS_FILE);
    let too_big = fs::metadata(&path)
        .map(|meta| meta.len() > DECISIONS_ROTATE_BYTES)
        .unwrap_or(false);
    if too_big {
        let rotated = PathBuf::from(format!("{}.1", path.display()));
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, &rotated);
    }
    let mut line =
        serde_json::to_string(entry).map_err(|e| ResultsError::Io(path.clone(), e.to_string()))?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| ResultsError::Io(path.clone(), e.to_string()))?;
    file.write_all(line.as_bytes())
        .map_err(|e| ResultsError::Io(path, e.to_string()))
}

/// Штамп копии по пути `.hds-agent\backup\<stamp>\<rel>`.
///
/// Идём вверх до каталога `backup`: `rel` бывает вложенным, поэтому
/// «родитель файла» — не обязательно каталог штампа.
pub fn backup_stamp(backup: &Path) -> Option<String> {
    let mut current = Some(backup);
    while let Some(path) = current {
        let parent = path.parent()?;
        let is_root = parent
            .file_name()
            .map(|name| name == BACKUP_SUBDIR)
            .unwrap_or(false);
        if is_root {
            let name = path.file_name()?.to_string_lossy().into_owned();
            return valid_stamp(&name).then_some(name);
        }
        current = Some(parent);
    }
    None
}

/// Имя штампа — только компонент каталога: ни `..`, ни разделителей, ни диска.
fn valid_stamp(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.contains("..")
}

/// Время изменения файла в секундах Unix (0 — если платформа не отдала).
fn mtime_of(meta: &fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Временный файл рядом с оригиналом: атомарная замена через
/// [`hds_core::config::replace_file`]. Расширение `.tmp` — зеркало его пропускает.
fn restore_tmp_path(original: &Path) -> PathBuf {
    let name = original
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut candidate = original.with_file_name(format!(".{name}.hds-restore.tmp"));
    let mut n = 2u32;
    while candidate.exists() {
        candidate = original.with_file_name(format!(".{name}.hds-restore-{n}.tmp"));
        n += 1;
    }
    candidate
}

impl<'a> Mirror<'a> {
    /// Принять решение по варианту документа (TR-7.6).
    ///
    /// `rel` — относительный путь **оригинала** в папке проекта (как в реестре;
    /// разделитель — платформы, `a/b` тоже принимается).
    pub fn decide(
        &mut self,
        rel: &str,
        decision: Decision,
    ) -> Result<DecisionReport, ResultsError> {
        let rel = normalize_rel(rel)?;
        let row = self
            .registry
            .by_orig(&rel)?
            .ok_or_else(|| ResultsError::NotFound(rel.clone()))?;
        let mut report = match decision {
            Decision::Accept => self.accept_variant(&row)?,
            Decision::Reject => self.reject_variant(&row)?,
            Decision::Conflict(choice) => self.resolve_conflict(&row, choice)?,
        };
        // В-A6-6: журнал решений — после успешного решения; сбой записи не
        // отменяет сделанное, но виден пользователю предупреждением.
        self.journal(&mut report, &decision.as_str(), None);
        Ok(report)
    }

    /// Дописать операцию в `.hds-agent\decisions.jsonl` (В-A6-6).
    ///
    /// Ошибка журнала — предупреждение в отчёте, а не отказ: решение уже
    /// выполнено, и откатывать его из-за журнала нельзя.
    fn journal(&self, report: &mut DecisionReport, action: &str, extra: Option<String>) {
        let entry = DecisionEntry {
            ts: now_epoch(),
            action: action.to_string(),
            orig_rel: report.orig_rel.clone(),
            stamp: report.stamp.clone(),
            status: report.status.as_str().to_string(),
            image: match report.anon_rel.is_empty() {
                true => None,
                false => Some(report.anon_rel.clone()),
            },
            extra,
        };
        if let Err(e) = append_decision(&self.project_root, &entry) {
            report
                .warnings
                .push(format!("журнал решений не записан: {e}"));
        }
    }

    /// Ситуация 1: оригинал не менялся, вариант становится оригиналом.
    fn accept_variant(&mut self, row: &Artifact) -> Result<DecisionReport, ResultsError> {
        require_status(row, &[Status::PendingAccept])?;
        require_replaceable(row)?;
        if !self.pending_path(&row.orig_rel).is_file() {
            return Err(ResultsError::NoVariant(row.orig_rel.clone()));
        }
        let original = self.project_root.join(rel_to_path(&row.orig_rel));
        let current = sha256_file(&original)
            .map_err(|e| ResultsError::Io(original.clone(), e.to_string()))?;
        if row.orig_sha256.as_deref() != Some(current.as_str()) {
            // Молча затирать правки пользователя нельзя — это конфликт.
            return Err(ResultsError::OriginalChanged(row.orig_rel.clone()));
        }
        let mut report = DecisionReport::new(row);
        self.replace_with_variant(row, &mut report)?;
        let anon_rel = self.rebuild_image(row, Status::Accepted)?;
        report.anon_rel = anon_rel;
        report.status = Status::Accepted;
        self.cleanup_decision(row);
        Ok(report)
    }

    /// Ситуация 4: вариант отбрасывается, образ возвращается к оригиналу.
    fn reject_variant(&mut self, row: &Artifact) -> Result<DecisionReport, ResultsError> {
        require_status(
            row,
            &[
                Status::PendingAccept,
                Status::Conflict,
                Status::AgentChanged,
            ],
        )?;
        let mut report = DecisionReport::new(row);
        // Сначала образ, потом удаление: сбой воркера не должен стоить
        // пользователю последнего варианта (§3.4 журнала).
        let anon_rel = self.rebuild_image(row, Status::Synced)?;
        report.anon_rel = anon_rel;
        report.status = Status::Synced;
        self.cleanup_decision(row);
        let text = format!(
            "изменения документа {} отклонены пользователем; образ восстановлен",
            report.anon_rel
        );
        self.notify(&mut report, MessageKind::DocumentRejected, text)?;
        Ok(report)
    }

    /// Ситуация 2: конфликт — три исхода, автоматически не делается ничего.
    fn resolve_conflict(
        &mut self,
        row: &Artifact,
        choice: ConflictChoice,
    ) -> Result<DecisionReport, ResultsError> {
        require_status(row, &[Status::Conflict, Status::PendingAccept])?;
        let mut report = DecisionReport::new(row);
        match choice {
            ConflictChoice::Agent => {
                require_replaceable(row)?;
                if !self.pending_path(&row.orig_rel).is_file() {
                    return Err(ResultsError::NoVariant(row.orig_rel.clone()));
                }
                let original = self.project_root.join(rel_to_path(&row.orig_rel));
                let current = sha256_file(&original)
                    .map_err(|e| ResultsError::Io(original.clone(), e.to_string()))?;
                if row.orig_sha256.as_deref() != Some(current.as_str()) {
                    report.warnings.push(format!(
                        "правки оригинала после основания потеряны ({}): копия прежней \
                         версии — в {SERVICE_DIR}\\{BACKUP_SUBDIR}",
                        row.orig_rel
                    ));
                }
                self.replace_with_variant(row, &mut report)?;
                let anon_rel = self.rebuild_image(row, Status::Accepted)?;
                report.anon_rel = anon_rel;
                report.status = Status::Accepted;
                self.cleanup_decision(row);
            }
            ConflictChoice::New => {
                let anon_rel = self.rebuild_image(row, Status::Synced)?;
                report.anon_rel = anon_rel;
                report.status = Status::Synced;
                self.cleanup_decision(row);
                let text = format!(
                    "документ обновлён пользователем; правки агента не приняты — образ \
                     пересоздан из новой версии ({})",
                    report.anon_rel
                );
                self.notify(&mut report, MessageKind::DocumentRejected, text)?;
            }
            ConflictChoice::Delegate => {
                let copy = self.copy_variant_to_conflicts(row)?;
                report.conflict_copy = Some(copy.clone());
                let anon_rel = self.rebuild_image(row, Status::Synced)?;
                report.anon_rel = anon_rel;
                report.status = Status::Synced;
                self.cleanup_decision(row);
                let text = format!(
                    "документ обновлён; перенеси правки из {copy} в новую версию ({})",
                    report.anon_rel
                );
                self.notify(&mut report, MessageKind::ConflictDelegated, text)?;
            }
        }
        Ok(report)
    }

    /// backup → атомарная замена оригинала вариантом → статус `accepted`.
    ///
    /// Занятый файл (Word/Excel) — [`ResultsError::Busy`]: созданная копия
    /// откатывается, вариант и статус остаются как были (TR-7.6, случай 3).
    fn replace_with_variant(
        &mut self,
        row: &Artifact,
        report: &mut DecisionReport,
    ) -> Result<(), ResultsError> {
        let original = self.project_root.join(rel_to_path(&row.orig_rel));
        let pending = self.pending_path(&row.orig_rel);
        let backup = self.backup_path(&row.orig_rel);
        copy_file(&original, &backup)?;
        match replace_file(&pending, &original) {
            Ok(()) => {}
            Err(CoreError::Io(err)) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                rollback_backup(&backup);
                return Err(ResultsError::Busy(original));
            }
            Err(err) => {
                rollback_backup(&backup);
                return Err(ResultsError::Io(original, err.to_string()));
            }
        }
        report.stamp = backup_stamp(&backup);
        report.backup = Some(backup);
        if !self
            .registry
            .set_status_by_orig(&row.orig_rel, Status::Accepted)?
        {
            return Err(ResultsError::NotFound(row.orig_rel.clone()));
        }
        Ok(())
    }

    /// Пересоздать образ из **текущего** оригинала и записать новое основание.
    ///
    /// Возвращает относительный путь образа в workspace. Образ пересоздаётся
    /// тем же кодом, что и прямой путь (`anonymize mode:"agent"` /
    /// `text_image`), поэтому в workspace оказывается «официальная» версия, а
    /// не правка агента.
    fn rebuild_image(&mut self, row: &Artifact, status: Status) -> Result<String, ResultsError> {
        let kind = kind_for(&row.orig_rel).ok_or(ResultsError::Kind(row.kind))?;
        if !matches!(kind, Kind::Image | Kind::TextImage) {
            return Err(ResultsError::Kind(kind));
        }
        let original = self.project_root.join(rel_to_path(&row.orig_rel));
        let meta = fs::metadata(&original)
            .map_err(|e| ResultsError::Io(original.clone(), e.to_string()))?;
        let orig_sha = sha256_file(&original)
            .map_err(|e| ResultsError::Io(original.clone(), e.to_string()))?;
        let anon_rel = if row.anon_rel.is_empty() {
            // Запись без образа (упавший проход) — имя строим как прямой путь.
            let taken: BTreeSet<String> = self
                .registry
                .list()?
                .into_iter()
                .filter(|other| other.id != row.id && !other.anon_rel.is_empty())
                .map(|other| other.anon_rel)
                .collect();
            self.plan_anon_rel(&row.orig_rel, kind, &taken)?
        } else {
            row.anon_rel.clone()
        };
        let out = self.workspace.join(rel_to_path(&anon_rel));
        let file = ScannedFile {
            rel: row.orig_rel.clone(),
            abs: original,
            size: meta.len(),
            mtime: meta.modified().unwrap_or(UNIX_EPOCH),
        };
        self.write_image(&file, kind, &out, row.id)?;
        let anon_sha =
            sha256_file(&out).map_err(|e| ResultsError::Io(out.clone(), e.to_string()))?;
        self.registry
            .set_baseline(row.id, &orig_sha, &anon_sha, status)?;
        Ok(anon_rel)
    }

    /// Убрать вариант, основание и отчёт валидатора: после решения они не нужны.
    ///
    /// Отчёт (`<rel>.report.json`, A4.5) удаляется **до** чистки каталогов:
    /// иначе каталог, где остался только отчёт, не считался бы пустым.
    fn cleanup_decision(&self, row: &Artifact) {
        let pending_root = self.project_root.join(SERVICE_DIR).join("pending");
        let pending = self.pending_path(&row.orig_rel);
        let _ = fs::remove_file(crate::validator::report_path(&pending));
        let _ = fs::remove_file(&pending);
        prune_under(&pending_root, pending.parent());
        let base_root = self.project_root.join(SERVICE_DIR).join("base");
        let base = self.base_path(&row.orig_rel);
        let _ = fs::remove_file(&base);
        let _ = fs::remove_file(self.image_map_path(&base));
        prune_under(&base_root, base.parent());
    }

    /// Анонимная копия варианта агента в `<P>\workspace\_conflicts\`.
    ///
    /// Копируется **образ** (в нём метки), а не вариант из `pending` (там
    /// реальные значения): `_conflicts` живёт в анонимной зоне, значения туда
    /// попадать не должны (I1/I8).
    fn copy_variant_to_conflicts(&mut self, row: &Artifact) -> Result<String, ResultsError> {
        if row.anon_rel.is_empty() {
            return Err(ResultsError::NothingToDelegate(row.orig_rel.clone()));
        }
        let image = self.workspace.join(rel_to_path(&row.anon_rel));
        if !image.is_file() {
            return Err(ResultsError::NothingToDelegate(row.orig_rel.clone()));
        }
        let seen =
            sha256_file(&image).map_err(|e| ResultsError::Io(image.clone(), e.to_string()))?;
        let has_pending = self.pending_path(&row.orig_rel).is_file();
        if !has_pending && row.anon_sha256_issued.as_deref() == Some(seen.as_str()) {
            return Err(ResultsError::NothingToDelegate(row.orig_rel.clone()));
        }
        let name = split_path(&row.anon_rel).pop().unwrap_or_default();
        let (stem, ext) = split_name(&name);
        let dir = self.workspace.join(CONFLICTS_DIR);
        fs::create_dir_all(&dir).map_err(|e| ResultsError::Io(dir.clone(), e.to_string()))?;
        let mut rel = String::new();
        for n in 0..10_000u32 {
            let candidate = if n == 0 {
                format!("{stem}{CONFLICT_SUFFIX}{ext}")
            } else {
                format!("{stem}{CONFLICT_SUFFIX} ({}){ext}", n + 1)
            };
            if !dir.join(&candidate).exists() {
                rel = candidate;
                break;
            }
        }
        if rel.is_empty() {
            return Err(ResultsError::Io(
                dir,
                "нет свободного имени в _conflicts".to_string(),
            ));
        }
        copy_file(&image, &dir.join(&rel))?;
        Ok(format!("{CONFLICTS_DIR}{}{rel}", std::path::MAIN_SEPARATOR))
    }

    /// Путь копии прежнего оригинала: `.hds-agent\backup\<stamp>\<rel>`.
    ///
    /// Метка — локальное `YYYYMMDD-HHMMSS`; два решения в одну секунду не
    /// должны затереть друг друга, поэтому занятый каталог получает `-2`, `-3`.
    fn backup_path(&self, orig_rel: &str) -> PathBuf {
        let root = self.project_root.join(SERVICE_DIR).join(BACKUP_SUBDIR);
        let stamp = stamp_local(now_epoch());
        let mut dir = root.join(&stamp);
        let mut n = 2u32;
        while dir.exists() {
            dir = root.join(format!("{stamp}-{n}"));
            n += 1;
        }
        dir.join(rel_to_path(orig_rel))
    }

    /// Дописать сообщение агенту в `.hds-agent\messages.jsonl` (В-6).
    fn notify(
        &self,
        report: &mut DecisionReport,
        kind: MessageKind,
        text: String,
    ) -> Result<(), ResultsError> {
        let message = append_message(&self.project_root, kind, &report.anon_rel, text)?;
        report.message = Some(message);
        Ok(())
    }
}

/// Пересборка образов после склейки меток (A5.5, TR-2.6, В-A5.5-5).
impl<'a> Mirror<'a> {
    /// Пересоздать образы, где встречаются `dropped`-метки.
    ///
    /// После склейки NER отдаёт старшую метку, поэтому образы пересобираются из
    /// **текущего** оригинала тем же кодом, что и прямой путь. Варианты и
    /// конфликты не трогаем: там правки агента, и пересборка их потеряла бы —
    /// такие записи попадают в `skipped` (пользователь решает по
    /// «Результатам»).
    pub fn refresh_label_images(
        &mut self,
        dropped: &[String],
    ) -> Result<RefreshReport, ResultsError> {
        let targets: BTreeSet<(String, u32)> = dropped
            .iter()
            .filter_map(|token| hds_core::labels::label_parts(token))
            .map(|(kind, n, _gram)| (kind, n))
            .collect();
        let mut report = RefreshReport::default();
        if targets.is_empty() {
            return Ok(report);
        }
        for row in self.registry.list()? {
            if row.anon_rel.is_empty() {
                continue;
            }
            let image = self.workspace.join(rel_to_path(&row.anon_rel));
            let Ok(text) = crate::validator::read_text(&image, self.extractor) else {
                // Образ не прочитан (нет файла или воркера извлечения) — молча
                // пропускаем: пересборка начнётся с оригинала, а не с образа.
                continue;
            };
            if !mentions_any(&text, &targets) {
                continue;
            }
            let rebuildable =
                matches!(kind_for(&row.orig_rel), Some(Kind::Image | Kind::TextImage))
                    && RefreshReport::rebuildable(row.status);
            if !rebuildable {
                report.skipped.push(row.orig_rel.clone());
                continue;
            }
            let anon_rel = self.rebuild_image(&row, Status::Synced)?;
            report.rebuilt.push(RebuiltImage {
                rel: row.orig_rel.clone(),
                anon_rel,
                status: Status::Synced,
            });
        }
        Ok(report)
    }
}

/// Есть ли в тексте хотя бы одна из меток набора (`[PERSON_1|дат]` = `[PERSON_1]`).
///
/// Сравниваются части метки (`hds_core::labels`), а не строки: падежный
/// суффикс — не другая сущность (та же грабля, что у валидатора, A4.5).
fn mentions_any(text: &str, targets: &BTreeSet<(String, u32)>) -> bool {
    hds_core::labels::find_labels(text)
        .iter()
        .filter_map(|token| hds_core::labels::label_parts(token))
        .any(|(kind, n, _gram)| targets.contains(&(kind, n)))
}

/// Сравнение варианта с оригиналом (A5.4, TR-7.6 «Сравнить»).
impl<'a> Mirror<'a> {
    /// Отчёт «Сравнить»: метаданные, отчёт валидатора, построчный diff.
    ///
    /// Решения не принимает и ничего не меняет — только читает. Запись
    /// реестра ищется по относительному пути оригинала: панель «Результаты»
    /// показывает только то, что зеркало уже выпустило.
    pub fn compare(&mut self, rel: &str) -> Result<CompareReport, ResultsError> {
        let rel = normalize_rel(rel)?;
        let row = self
            .registry
            .by_orig(&rel)?
            .ok_or_else(|| ResultsError::NotFound(rel.clone()))?;
        let paths = variant_paths(&self.project_root, &row.orig_rel);
        let image = if row.anon_rel.is_empty() {
            PathBuf::new()
        } else {
            self.workspace.join(rel_to_path(&row.anon_rel))
        };
        let original_info = file_info(&paths.original);
        let variant_info = file_info(&paths.pending);
        // Diff строится по тексту: md/txt/xml читаются напрямую, docx/xlsx/
        // pdf/pptx — через `hds_extract` (тот же путь, что у валидатора).
        let mut diff_note = None;
        let (diff, diff_truncated) = match (
            crate::validator::read_text(&paths.original, self.extractor),
            crate::validator::read_text(&paths.pending, self.extractor),
        ) {
            (Ok(old), Ok(new)) => line_diff(&old, &new),
            (Err(e), _) => {
                diff_note = Some(format!("оригинал не прочитан: {e}"));
                (Vec::new(), false)
            }
            (_, Err(e)) => {
                diff_note = Some(format!("вариант не прочитан: {e}"));
                (Vec::new(), false)
            }
        };
        let identical = matches!(
            (&original_info.sha256, &variant_info.sha256),
            (Some(a), Some(b)) if a == b
        );
        Ok(CompareReport {
            rel: row.orig_rel.clone(),
            anon_rel: row.anon_rel.clone(),
            kind: row.kind,
            status: row.status,
            original: original_info,
            variant: variant_info,
            base: file_info(&paths.base),
            image: file_info(&image),
            report: crate::validator::Report::read(&paths.pending),
            identical,
            diff,
            diff_truncated,
            diff_note,
        })
    }
}

/// Копии, откат, пересоздание образа и relink (A6.4: В-A6-4…В-A6-7).
impl<'a> Mirror<'a> {
    /// Копии прежних версий из `.hds-agent\backup\<stamp>\<rel>` (В-A6-4).
    ///
    /// Свежие штампы первыми. `rel = None` — все копии проекта (sha не
    /// считается: список не должен читать содержимое), `Some` — копии одного
    /// файла с sha256. Имя штампа — только компонент каталога: `..`, `\`, `/`
    /// и `:` отбрасываются, поэтому обход не выходит из `backup`.
    pub fn backups(&self, rel: Option<&str>) -> Result<Vec<BackupCopy>, ResultsError> {
        let root = self.project_root.join(SERVICE_DIR).join(BACKUP_SUBDIR);
        let filter = match rel {
            Some(value) => Some(normalize_rel(value)?),
            None => None,
        };
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ResultsError::Io(root, e.to_string())),
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let stamp = entry.file_name().to_string_lossy().into_owned();
            if !valid_stamp(&stamp) {
                continue;
            }
            match &filter {
                Some(rel) => {
                    let path = dir.join(rel_to_path(rel));
                    let meta = match fs::metadata(&path) {
                        Ok(meta) if meta.is_file() => meta,
                        _ => continue,
                    };
                    let sha = sha256_file(&path)
                        .map_err(|e| ResultsError::Io(path.clone(), e.to_string()))?;
                    out.push(BackupCopy {
                        stamp,
                        rel: rel.clone(),
                        path,
                        size: meta.len(),
                        mtime: mtime_of(&meta),
                        sha256: Some(sha),
                    });
                }
                None => {
                    let files = match walk_files(&dir) {
                        Ok(files) => files,
                        // Каталог копии мог исчезнуть между чтением и обходом.
                        Err(_) => continue,
                    };
                    for file in files {
                        out.push(BackupCopy {
                            stamp: stamp.clone(),
                            rel: file.rel,
                            path: file.abs,
                            size: file.size,
                            mtime: file
                                .mtime
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_secs_f64())
                                .unwrap_or(0.0),
                            sha256: None,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| b.stamp.cmp(&a.stamp).then_with(|| a.rel.cmp(&b.rel)));
        Ok(out)
    }

    /// «Вернуть» (TR-7.6, В-A6-4): заменить оригинал копией и пересобрать образ.
    ///
    /// `stamp = None` — самая свежая копия. Копия **копируется** во временный
    /// файл и уже он атомарно заменяет оригинал: набор копий не тратится, а
    /// занятый файл (`os error 32`) даёт [`ResultsError::Busy`] и не меняет
    /// состояние (ни оригинал, ни реестр). В `messages.jsonl` не пишется:
    /// результат виден в UI (AG-20).
    pub fn restore(
        &mut self,
        rel: &str,
        stamp: Option<&str>,
    ) -> Result<DecisionReport, ResultsError> {
        let rel = normalize_rel(rel)?;
        let row = self
            .registry
            .by_orig(&rel)?
            .ok_or_else(|| ResultsError::NotFound(rel.clone()))?;
        // Образ пересобирается сразу после замены: формат проверяем до неё,
        // иначе оригинал вернулся бы, а образ остался прежним.
        match kind_for(&row.orig_rel) {
            Some(Kind::Image | Kind::TextImage) => {}
            other => return Err(ResultsError::Kind(other.unwrap_or(row.kind))),
        }
        let copies = self.backups(Some(&rel))?;
        let copy = match stamp.map(str::trim).filter(|s| !s.is_empty()) {
            Some(want) => copies
                .iter()
                .find(|copy| copy.stamp == want)
                .cloned()
                .ok_or_else(|| ResultsError::NoBackup {
                    rel: rel.clone(),
                    stamp: Some(want.to_string()),
                    available: copies.iter().map(|copy| copy.stamp.clone()).collect(),
                })?,
            None => copies
                .into_iter()
                .next()
                .ok_or_else(|| ResultsError::NoBackup {
                    rel: rel.clone(),
                    stamp: None,
                    available: Vec::new(),
                })?,
        };
        let sha = match copy.sha256.clone() {
            Some(sha) => sha,
            None => sha256_file(&copy.path)
                .map_err(|e| ResultsError::Io(copy.path.clone(), e.to_string()))?,
        };
        let original = self.project_root.join(rel_to_path(&rel));
        if let Some(parent) = original.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| ResultsError::Io(parent.to_path_buf(), e.to_string()))?;
        }
        let tmp = restore_tmp_path(&original);
        copy_file(&copy.path, &tmp)?;
        match replace_file(&tmp, &original) {
            Ok(()) => {}
            Err(CoreError::Io(err)) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                let _ = fs::remove_file(&tmp);
                return Err(ResultsError::Busy(original));
            }
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                return Err(ResultsError::Io(original, err.to_string()));
            }
        }
        let restored = sha256_file(&original)
            .map_err(|e| ResultsError::Io(original.clone(), e.to_string()))?;
        if restored != sha {
            return Err(ResultsError::Io(
                original,
                "sha256 восстановленного файла не совпал с копией".to_string(),
            ));
        }
        let mut report = DecisionReport::new(&row);
        report.backup = Some(copy.path);
        report.stamp = Some(copy.stamp);
        report.sha256 = Some(sha);
        report.anon_rel = self.rebuild_image(&row, Status::Synced)?;
        report.status = Status::Synced;
        let extra = report.sha256.as_ref().map(|sha| format!("sha256={sha}"));
        self.journal(&mut report, "restore", extra);
        Ok(report)
    }

    /// «Пересоздать образ» для записи `orphan` (A6.4).
    ///
    /// У остальных статусов образ либо актуален, либо ждёт решения в
    /// `.hds-agent\pending` — тогда пересборка потеряла бы правки агента
    /// (отказ с понятным текстом).
    pub fn rebuild(&mut self, rel: &str) -> Result<DecisionReport, ResultsError> {
        let rel = normalize_rel(rel)?;
        let row = self
            .registry
            .by_orig(&rel)?
            .ok_or_else(|| ResultsError::NotFound(rel.clone()))?;
        if row.status != Status::Orphan {
            return Err(ResultsError::NotApplicable {
                rel: row.orig_rel.clone(),
                status: row.status,
                op: "rebuild",
            });
        }
        let mut report = DecisionReport::new(&row);
        let anon_rel = self.rebuild_image(&row, Status::Synced)?;
        let image = self.workspace.join(rel_to_path(&anon_rel));
        report.sha256 =
            Some(sha256_file(&image).map_err(|e| ResultsError::Io(image.clone(), e.to_string()))?);
        report.anon_rel = anon_rel;
        report.status = Status::Synced;
        let extra = report.sha256.as_ref().map(|sha| format!("sha256={sha}"));
        self.journal(&mut report, "rebuild", extra);
        Ok(report)
    }

    /// Relink: восстановить запись по маркеру образа (A6.4, В-A6-5).
    ///
    /// **Только по команде**: в проходе зеркала не вызывается. Маркер
    /// `hds-agent/<P>/<id>` должен принадлежать этому проекту, а `artifact_id`
    /// — этой же записи реестра; статус — `orphan`/`error`. Восстановление —
    /// через [`ArtifactRegistry::set_baseline`] (контракт §4: не «руками»).
    pub fn relink(&mut self, rel: &str) -> Result<DecisionReport, ResultsError> {
        let rel = normalize_rel(rel)?;
        let row = self
            .registry
            .by_orig(&rel)?
            .ok_or_else(|| ResultsError::NotFound(rel.clone()))?;
        if !matches!(row.status, Status::Orphan | Status::Error) {
            return Err(ResultsError::NotApplicable {
                rel: row.orig_rel.clone(),
                status: row.status,
                op: "relink",
            });
        }
        if row.anon_rel.is_empty() {
            return Err(ResultsError::NoImage(row.orig_rel.clone()));
        }
        let image = self.workspace.join(rel_to_path(&row.anon_rel));
        if !image.is_file() {
            return Err(ResultsError::NoImage(row.orig_rel.clone()));
        }
        let marker = self.anonymizer.read_marker(&image)?;
        let own_id = match (marker.project_id.as_deref(), marker.artifact_id) {
            (Some(project), Some(id)) if project == self.project_id => self
                .registry
                .by_id(id)?
                .map(|found| found.orig_rel == row.orig_rel)
                .unwrap_or(false),
            _ => false,
        };
        if !own_id {
            return Err(ResultsError::BadMarker(row.orig_rel.clone()));
        }
        let original = self.project_root.join(rel_to_path(&row.orig_rel));
        if !original.is_file() {
            return Err(ResultsError::NotFound(row.orig_rel.clone()));
        }
        let orig_sha = sha256_file(&original)
            .map_err(|e| ResultsError::Io(original.clone(), e.to_string()))?;
        let anon_sha =
            sha256_file(&image).map_err(|e| ResultsError::Io(image.clone(), e.to_string()))?;
        self.registry
            .set_baseline(row.id, &orig_sha, &anon_sha, Status::Synced)?;
        let mut report = DecisionReport::new(&row);
        report.anon_rel = row.anon_rel.clone();
        report.status = Status::Synced;
        report.sha256 = Some(anon_sha);
        let extra = Some(format!("artifact_id={}", row.id));
        self.journal(&mut report, "relink", extra);
        Ok(report)
    }
}

/// Метаданные файла: путь, наличие, размер, sha256, время изменения.
fn file_info(path: &Path) -> FileInfo {
    let meta = fs::metadata(path).ok();
    FileInfo {
        path: path.display().to_string(),
        exists: meta.is_some(),
        size: meta.as_ref().map(|m| m.len()),
        sha256: if path.is_file() {
            sha256_file(path).ok()
        } else {
            None
        },
        mtime: meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64()),
    }
}

/// Строки одной стороны diff: обрезка по [`DIFF_SCAN_LINES`] и
/// [`DIFF_LINE_CHARS`]; `true` — текст пришлось обрезать.
fn diff_lines(text: &str) -> (Vec<String>, bool) {
    let mut lines: Vec<String> = text
        .lines()
        .take(DIFF_SCAN_LINES)
        .map(|line| clip_chars(line, DIFF_LINE_CHARS))
        .collect();
    let more = text.lines().count() > DIFF_SCAN_LINES;
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    (lines, more)
}

/// Строка не длиннее `max` символов (`…` вместо хвоста).
fn clip_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Построчный diff (LCS) с жёсткими лимитами: строк ≤ [`DIFF_MAX_LINES`],
/// символов ≤ [`DIFF_MAX_CHARS`]. Одинаковый текст — пустой diff.
fn line_diff(old: &str, new: &str) -> (Vec<String>, bool) {
    if old == new {
        return (Vec::new(), false);
    }
    let (a, a_more) = diff_lines(old);
    let (b, b_more) = diff_lines(new);
    let mut truncated = a_more || b_more;
    let (n, m) = (a.len(), b.len());
    // LCS по строкам: 400×400 в худшем случае — это 160 тыс. ячеек, дешевле
    // любого внешнего diff и без новых зависимостей.
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out: Vec<String> = Vec::new();
    let mut chars = 0usize;
    let (mut i, mut j) = (0usize, 0usize);
    while i < n || j < m {
        let prefix;
        let mut take_a = false;
        let mut take_b = false;
        if i < n && j < m && a[i] == b[j] {
            prefix = "  ";
            take_a = true;
            take_b = true;
        } else if j >= m || (i < n && dp[i + 1][j] >= dp[i][j + 1]) {
            prefix = "- ";
            take_a = true;
        } else {
            prefix = "+ ";
            take_b = true;
        }
        let line = if take_a { a[i].clone() } else { b[j].clone() };
        if take_a {
            i += 1;
        }
        if take_b {
            j += 1;
        }
        let cost = line.chars().count() + prefix.len();
        if out.len() >= DIFF_MAX_LINES || chars + cost > DIFF_MAX_CHARS {
            truncated = true;
            break;
        }
        chars += cost;
        out.push(format!("{prefix}{line}"));
    }
    (out, truncated)
}

/// Решение возможно только для записи, которая ждёт решения (TR-7.2).
fn require_status(row: &Artifact, allowed: &[Status]) -> Result<(), ResultsError> {
    if allowed.contains(&row.status) {
        return Ok(());
    }
    Err(ResultsError::WrongStatus {
        rel: row.orig_rel.clone(),
        status: row.status,
    })
}

/// Заменить оригинал можно только у образа того же формата.
///
/// Текстовый образ (`*.pdf.md`, `*.pptx.md`, `*.mpp.md`) — только для чтения
/// (TR-7.3a/D22): подменить им pdf значило бы испортить документ.
fn require_replaceable(row: &Artifact) -> Result<(), ResultsError> {
    match kind_for(&row.orig_rel) {
        Some(Kind::Image) => Ok(()),
        other => Err(ResultsError::Kind(other.unwrap_or(row.kind))),
    }
}

/// Откатить созданную копию: при занятом файле backup оставаться не должен
/// (TR-7.6, случай 3 — «backup не создаётся»).
fn rollback_backup(backup: &Path) {
    let _ = fs::remove_file(backup);
    let mut dir = backup.parent();
    while let Some(current) = dir {
        if fs::remove_dir(current).is_err() {
            break;
        }
        if current.file_name().map(|n| n == BACKUP_SUBDIR) == Some(true) {
            break;
        }
        dir = current.parent();
    }
}

/// Убрать опустевшие каталоги вверх до `root` (сам `root` не удаляется).
fn prune_under(root: &Path, dir: Option<&Path>) {
    let mut current = dir;
    while let Some(path) = current {
        if path == root || !path.starts_with(root) {
            break;
        }
        if fs::remove_dir(path).is_err() {
            break;
        }
        current = path.parent();
    }
}

#[cfg(test)]
mod tests {
    //! Четыре ситуации TR-7.6 на копиях: приём, отклонение, три исхода
    //! конфликта и занятый файл. Воркеры — поддельные (`crate::mirror::tests`),
    //! поэтому проверяется логика решений, а не NER.
    use super::*;
    use crate::mirror::tests::{fast, FakeAnon, FakeExtractor, Stand};
    use crate::registry_db::ArtifactRegistry;

    /// AG-20: очередь сообщений читается и после доставки переезжает в
    /// `messages.delivered.jsonl` (повторно агенту не придёт, но останется
    /// для аудита — только метки и пути, I8).
    #[test]
    fn queue_is_read_and_moved_to_delivered() {
        let stand = Stand::new("queue");
        assert!(read_queue(&stand.project).is_empty(), "пустой очереди нет");
        let message = append_message(
            &stand.project,
            MessageKind::DocumentRejected,
            "Протоколы\\[DOC_1].docx",
            "изменения документа [DOC_1] отклонены пользователем".to_string(),
        )
        .expect("очередь");
        // `ts` — f64 из JSON: парсер serde_json может отличаться на 1 ulp от
        // исходного значения, поэтому время сверяем с допуском, а остальные
        // поля записи — точно (иначе тест «мигает» раз в сотню прогонов).
        let queued = read_queue(&stand.project);
        assert_eq!(queued.len(), 1, "одна запись в очереди");
        assert_eq!(queued[0].kind, message.kind);
        assert_eq!(queued[0].rel, message.rel);
        assert_eq!(queued[0].text, message.text);
        assert!(
            (queued[0].ts - message.ts).abs() < 1e-3,
            "время записи: {} vs {}",
            queued[0].ts,
            message.ts
        );
        let prompt = queue_prompt(&read_queue(&stand.project)).expect("текст для агента");
        assert!(prompt.contains("[DOC_1]"), "{prompt}");
        mark_delivered(&stand.project).expect("доставлено");
        assert!(
            read_queue(&stand.project).is_empty(),
            "очередь очищена после доставки"
        );
        let delivered =
            std::fs::read_to_string(stand.project.join(SERVICE_DIR).join(DELIVERED_FILE))
                .expect("журнал доставки");
        assert!(delivered.contains("[DOC_1]"), "{delivered}");
        let _ = std::fs::remove_dir_all(&stand.root);
    }

    /// Стенд с собранным вариантом: образ выпущен, агент его правит, второй
    /// проход кладёт вариант в `.hds-agent\pending` и снимок основания.
    fn stand_with_variant(tag: &str) -> (Stand, FakeAnon, FakeExtractor) {
        let stand = Stand::new(tag);
        stand.write(&rel(), "Контакт: Иванов Иван\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 1);
        stand.write_ws(&anon_rel(), "Контакт: [PERSON_1] — правка агента\n");
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.variants, 1, "вариант должен быть собран: {report:?}");
        (stand, anon, ext)
    }

    /// Решение через `Mirror::decide` — так же, как это делает CLI.
    fn decide(
        stand: &Stand,
        anon: &mut FakeAnon,
        ext: &mut FakeExtractor,
        decision: Decision,
    ) -> Result<DecisionReport, ResultsError> {
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut mirror = Mirror::new(
            "P0001",
            &stand.project,
            &stand.workspace,
            &registry,
            anon,
            ext,
            fast(),
            200,
        );
        mirror.decide(&rel(), decision)
    }

    /// Оригинал в подкаталоге (разделитель платформы — как в реестре).
    fn rel() -> String {
        format!(
            "06 Переписка{}Письмо Иванов Иван.md",
            std::path::MAIN_SEPARATOR
        )
    }

    /// Образ того же документа: имя с меткой, без значений.
    fn anon_rel() -> String {
        format!(
            "06 Переписка{}Письмо [PERSON_1].md",
            std::path::MAIN_SEPARATOR
        )
    }

    fn service(stand: &Stand, sub: &str) -> PathBuf {
        stand.project.join(SERVICE_DIR).join(sub)
    }

    fn messages(stand: &Stand) -> String {
        fs::read_to_string(service(stand, MESSAGES_FILE)).unwrap_or_default()
    }

    /// Ситуация 1: оригинал = вариант, backup хранит прежнюю версию, образ
    /// пересоздан из нового оригинала, pending и основание убраны.
    #[test]
    fn accept_replaces_original_with_variant_and_rebuilds_image() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-accept");
        let report = decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        assert_eq!(report.status, Status::Accepted);
        assert_eq!(
            fs::read_to_string(stand.project.join(rel())).unwrap(),
            "Контакт: Иванов Иван — правка агента\n",
            "оригинал заменён деанонимизированным вариантом"
        );
        let backup = report.backup.clone().expect("backup обязателен");
        assert!(backup.is_file(), "{}", backup.display());
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "Контакт: Иванов Иван\n"
        );
        assert!(
            backup.starts_with(service(&stand, BACKUP_SUBDIR)),
            "{}",
            backup.display()
        );

        // Образ пересоздан из нового оригинала: метка и правка, маркера в тексте нет.
        let image = stand.image(&anon_rel());
        assert!(image.contains("[PERSON_1] — правка агента"), "{image}");
        assert!(image.starts_with("<!-- hds-agent/P0001/"), "{image}");
        assert!(!image.contains("Иванов Иван"), "{image}");

        assert!(!service(&stand, "pending").join(rel()).exists());
        assert!(!service(&stand, "base").join(rel()).exists());
        assert!(!service(&stand, "base")
            .join(format!("{}.images.json", rel()))
            .exists());

        let registry = stand.registry();
        let row = registry.by_orig(&rel()).unwrap().unwrap();
        let orig_sha = sha256_file(&stand.project.join(rel())).unwrap();
        let image_sha = sha256_file(&stand.workspace.join(anon_rel())).unwrap();
        assert_eq!(row.status, Status::Accepted);
        assert_eq!(row.orig_sha256.as_deref(), Some(orig_sha.as_str()));
        assert_eq!(row.anon_sha256_issued.as_deref(), Some(image_sha.as_str()));
        assert_eq!(row.anon_sha256_seen.as_deref(), Some(image_sha.as_str()));
        assert!(registry.list_pending().unwrap().is_empty());
        drop(registry);

        // Критерий A4.4: повторный проход — 0 изменений.
        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
        assert!(!service(&stand, MESSAGES_FILE).exists(), "приём молчаливый");
    }
    /// Принятый артефакт живёт дальше: правка пользователя обновляет образ,
    /// новая правка агента снова даёт вариант (статус `accepted` не «замирает»).
    #[test]
    fn accepted_artifact_keeps_being_mirrored() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-accepted-live");
        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");

        stand.write(&rel(), "Контакт: Петров Пётр\n");
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.updated, 1, "{report:?}");
        let image = stand.image(&anon_rel());
        assert!(image.contains("[PERSON_2]"), "{image}");

        stand.write_ws(
            &anon_rel(),
            "<!-- hds-agent/P0001/1 -->\nКонтакт: [PERSON_2] — вторая правка\n",
        );
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.variants, 1, "{report:?}");
        assert_eq!(
            stand.registry().by_orig(&rel()).unwrap().unwrap().status,
            Status::PendingAccept
        );
    }

    /// Ситуация 2: «Принять» при изменившемся оригинале запрещено — иначе
    /// правки пользователя потерялись бы молча.
    #[test]
    fn accept_refuses_when_original_changed_after_base() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-stale");
        stand.write(&rel(), "Контакт: Петров Пётр — правка пользователя\n");
        assert_eq!(
            stand.run(&mut anon, &mut ext, 200).conflicts,
            1,
            "прямой проход фиксирует конфликт"
        );

        let err = decide(&stand, &mut anon, &mut ext, Decision::Accept).expect_err("нельзя");
        match &err {
            ResultsError::WrongStatus { status, .. } => assert_eq!(*status, Status::Conflict),
            other => panic!("ожидался конфликт, получено: {other}"),
        }
        assert!(err.to_string().contains("conflict"), "{err}");
        assert_eq!(
            fs::read_to_string(stand.project.join(rel())).unwrap(),
            "Контакт: Петров Пётр — правка пользователя\n"
        );
        assert!(service(&stand, "pending").join(rel()).is_file());
    }

    /// Ситуация 2, исход «оставить вариант агента»: оригинал заменяется, но с
    /// backup и предупреждением о потере правок.
    #[test]
    fn conflict_agent_choice_replaces_original_and_warns() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-conflict-agent");
        stand.write(&rel(), "Контакт: Петров Пётр — правка пользователя\n");
        assert_eq!(stand.run(&mut anon, &mut ext, 200).conflicts, 1);

        let report = decide(
            &stand,
            &mut anon,
            &mut ext,
            Decision::Conflict(ConflictChoice::Agent),
        )
        .expect("оставить вариант");
        assert_eq!(report.status, Status::Accepted);
        assert!(
            !report.warnings.is_empty(),
            "потеря правок должна быть предупреждена"
        );
        assert_eq!(
            fs::read_to_string(stand.project.join(rel())).unwrap(),
            "Контакт: Иванов Иван — правка агента\n"
        );
        // backup хранит именно версию пользователя — она перезаписана.
        let backup = report.backup.expect("backup обязателен");
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "Контакт: Петров Пётр — правка пользователя\n"
        );
        assert!(!service(&stand, "pending").join(rel()).exists());
        assert!(!service(&stand, "base").join(rel()).exists());
        assert!(stand.image(&anon_rel()).contains("[PERSON_1]"));
        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
    }

    /// Ситуация 2, исход «взять новую версию»: оригинал не трогается, образ —
    /// из новой официальной версии, агент получает сообщение с меткой.
    #[test]
    fn conflict_new_takes_official_version_and_notifies_agent() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-conflict-new");
        stand.write(&rel(), "Контакт: Петров Пётр — правка пользователя\n");
        assert_eq!(stand.run(&mut anon, &mut ext, 200).conflicts, 1);

        let report = decide(
            &stand,
            &mut anon,
            &mut ext,
            Decision::Conflict(ConflictChoice::New),
        )
        .expect("взять новую версию");
        assert_eq!(report.status, Status::Synced);
        assert!(report.backup.is_none(), "оригинал не заменяется");
        assert_eq!(
            fs::read_to_string(stand.project.join(rel())).unwrap(),
            "Контакт: Петров Пётр — правка пользователя\n"
        );
        let image = stand.image(&anon_rel());
        assert!(
            image.contains("[PERSON_2]"),
            "образ из новой версии: {image}"
        );
        assert!(!image.contains("правка агента"), "{image}");
        assert!(!service(&stand, "pending").join(rel()).exists());
        assert!(!service(&stand, "base").join(rel()).exists());

        let text = messages(&stand);
        assert_eq!(text.lines().count(), 1, "{text}");
        let line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(line["kind"], "document_rejected");
        assert_eq!(line["rel"], anon_rel());
        assert!(line["ts"].as_f64().unwrap() > 0.0);
        assert!(
            line["text"].as_str().unwrap().contains("[PERSON_1]"),
            "в сообщении метка, а не значение: {text}"
        );
        assert!(!text.contains("Петров Пётр"), "значения не уходят: {text}");

        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
        assert_eq!(
            messages(&stand).lines().count(),
            1,
            "сообщений не прибавилось"
        );
    }
    /// Ситуация 2, исход «поручить агенту»: вариант остаётся доступным агенту
    /// анонимной копией в `_conflicts`, образ — из новой версии.
    #[test]
    fn conflict_delegate_moves_variant_to_conflicts() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-conflict-delegate");
        stand.write(&rel(), "Контакт: Петров Пётр — правка пользователя\n");
        assert_eq!(stand.run(&mut anon, &mut ext, 200).conflicts, 1);

        let report = decide(
            &stand,
            &mut anon,
            &mut ext,
            Decision::Conflict(ConflictChoice::Delegate),
        )
        .expect("поручить агенту");
        assert_eq!(report.status, Status::Synced);
        let copy = report.conflict_copy.clone().expect("копия варианта");
        assert!(copy.starts_with(CONFLICTS_DIR), "{copy}");
        let copy_text = fs::read_to_string(stand.workspace.join(rel_to_path(&copy))).unwrap();
        assert!(
            copy_text.contains("[PERSON_1] — правка агента"),
            "{copy_text}"
        );
        assert!(
            !copy_text.contains("Иванов Иван"),
            "копия анонимна: {copy_text}"
        );

        let image = stand.image(&anon_rel());
        assert!(image.contains("[PERSON_2]"), "{image}");
        assert!(!image.contains("правка агента"), "{image}");
        assert!(!service(&stand, "pending").join(rel()).exists());
        assert!(!service(&stand, "base").join(rel()).exists());

        let text = messages(&stand);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("conflict_delegated"), "{text}");
        assert!(text.contains("_conflicts"), "{text}");
        assert!(!text.contains("Иванов Иван"), "значения не уходят: {text}");

        // Повторный проход не плодит ни копий в `_conflicts`, ни сообщений.
        let copies = fs::read_dir(stand.workspace.join(CONFLICTS_DIR))
            .unwrap()
            .count();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
        assert_eq!(
            fs::read_dir(stand.workspace.join(CONFLICTS_DIR))
                .unwrap()
                .count(),
            copies
        );
        assert_eq!(messages(&stand).lines().count(), 1);
    }

    /// Ситуация 4: образ возвращается к оригиналу, вариант и основание убраны,
    /// агенту уходит одно сообщение — и повторный проход его не дублирует.
    #[test]
    fn reject_restores_image_and_notifies_agent_once() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-reject");
        let report = decide(&stand, &mut anon, &mut ext, Decision::Reject).expect("отклонить");
        assert_eq!(report.status, Status::Synced);
        assert!(
            report.backup.is_none(),
            "оригинал не меняется — backup не нужен"
        );
        assert_eq!(
            fs::read_to_string(stand.project.join(rel())).unwrap(),
            "Контакт: Иванов Иван\n"
        );
        let image = stand.image(&anon_rel());
        assert!(image.contains("[PERSON_1]"), "{image}");
        assert!(
            !image.contains("правка агента"),
            "образ вернулся к оригиналу: {image}"
        );
        assert!(!service(&stand, "pending").join(rel()).exists());
        assert!(!service(&stand, "base").join(rel()).exists());

        let text = messages(&stand);
        assert_eq!(text.lines().count(), 1, "{text}");
        let line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(line["kind"], "document_rejected");
        assert!(line["text"]
            .as_str()
            .unwrap()
            .contains("образ восстановлен"));
        assert!(!text.contains("Иванов Иван"), "значения не уходят: {text}");

        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
        assert_eq!(messages(&stand).lines().count(), 1);
    }
    /// Ситуация 3: файл держит Word/Excel — «файл занят», ничего не меняется.
    ///
    /// Открываем оригинал так же, как офисный редактор: `FILE_SHARE_READ` без
    /// `FILE_SHARE_DELETE`. Чтение (sha256) разрешено, замена — нет.
    #[cfg(windows)]
    #[test]
    fn accept_on_busy_file_keeps_everything_as_is() {
        use std::os::windows::fs::OpenOptionsExt;
        let (stand, mut anon, mut ext) = stand_with_variant("a44-busy");
        let original = stand.project.join(rel());
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0000_0001)
            .open(&original)
            .expect("открыть как Word");

        let err = decide(&stand, &mut anon, &mut ext, Decision::Accept).expect_err("занят");
        assert!(matches!(err, ResultsError::Busy(_)), "{err}");
        assert!(err.to_string().contains("файл занят"), "{err}");
        drop(held);

        // Состояние не изменилось: вариант на месте, статус прежний, backup нет.
        assert!(service(&stand, "pending").join(rel()).is_file());
        assert_eq!(
            stand.registry().by_orig(&rel()).unwrap().unwrap().status,
            Status::PendingAccept
        );
        let backup_root = service(&stand, BACKUP_SUBDIR);
        assert_eq!(
            fs::read_dir(&backup_root).map(|it| it.count()).unwrap_or(0),
            0,
            "при занятом файле backup не создаётся"
        );
        assert_eq!(
            fs::read_to_string(&original).unwrap(),
            "Контакт: Иванов Иван\n"
        );

        // После закрытия файла «Принять» проходит.
        let report = decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("после закрытия");
        assert_eq!(report.status, Status::Accepted);
    }

    /// Ошибки — с подсказкой, что делать: путь, отсутствие записи, повторное решение.
    #[test]
    fn decide_reports_actionable_errors() {
        let (stand, mut anon, mut ext) = stand_with_variant("a44-errors");
        {
            let registry = ArtifactRegistry::open(&stand.db).unwrap();
            let mut mirror = Mirror::new(
                "P0001",
                &stand.project,
                &stand.workspace,
                &registry,
                &mut anon,
                &mut ext,
                fast(),
                200,
            );
            let err = mirror
                .decide("..\\..\\секрет.md", Decision::Accept)
                .expect_err("путь вне проекта");
            assert!(matches!(err, ResultsError::BadPath(_)), "{err}");
            let err = mirror
                .decide("Нет такого.md", Decision::Accept)
                .expect_err("нет записи");
            assert!(matches!(err, ResultsError::NotFound(_)), "{err}");
            assert!(err.to_string().contains("реестр"), "{err}");
        }

        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        let err = decide(&stand, &mut anon, &mut ext, Decision::Accept).expect_err("уже принято");
        match &err {
            ResultsError::WrongStatus { status, .. } => assert_eq!(*status, Status::Accepted),
            other => panic!("ожидался статус, получено: {other}"),
        }
        let err = decide(&stand, &mut anon, &mut ext, Decision::Reject).expect_err("нечего");
        assert!(matches!(err, ResultsError::WrongStatus { .. }), "{err}");
    }

    /// Пути из CLI: `a/b` и `a\b` — одно и то же, выход из проекта запрещён.
    #[test]
    fn normalize_rel_accepts_slashes_and_rejects_escape() {
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            normalize_rel("a/b\\c.md").unwrap(),
            format!("a{sep}b{sep}c.md")
        );
        assert!(normalize_rel("").is_err());
        assert!(normalize_rel("../x.md").is_err());
        assert!(normalize_rel("a/../x.md").is_err());
        assert!(normalize_rel("C:\\x.md").is_err());
    }

    /// Разбор `--choice` и строк сообщений — то, чем пользуется CLI и A5.
    #[test]
    fn choices_and_message_kinds_parse_back() {
        assert_eq!(
            ConflictChoice::parse(" AGENT "),
            Some(ConflictChoice::Agent)
        );
        assert_eq!(ConflictChoice::parse("new"), Some(ConflictChoice::New));
        assert_eq!(
            ConflictChoice::parse("delegate"),
            Some(ConflictChoice::Delegate)
        );
        assert_eq!(ConflictChoice::parse("оба"), None);
        assert_eq!(
            Decision::Conflict(ConflictChoice::New).as_str(),
            "conflict:new"
        );
        assert_eq!(
            MessageKind::parse("document_rejected"),
            Some(MessageKind::DocumentRejected)
        );
        assert_eq!(
            MessageKind::parse(MessageKind::ConflictDelegated.as_str()),
            Some(MessageKind::ConflictDelegated)
        );
        // A5.5: сообщения о склейке меток
        assert_eq!(
            MessageKind::parse("labels_merged"),
            Some(MessageKind::LabelsMerged)
        );
        assert_eq!(
            MessageKind::parse(MessageKind::LabelsUnmerged.as_str()),
            Some(MessageKind::LabelsUnmerged)
        );
        assert_eq!(MessageKind::LabelsMerged.as_str(), "labels_merged");
        assert_eq!(MessageKind::parse("что-то"), None);
    }

    /// A5.5: «Разъединить» пересобирает образ по старшей метке, а вариант в
    /// `pending` не трогает — его правки потерялись бы (В-A5.5-5).
    #[test]
    fn refresh_rebuilds_synced_images_and_skips_variants() {
        let stand = Stand::new("a55-refresh");
        stand.write("А.md", "Контакт: Иванов Иван\n");
        stand.write("Б.md", "Контакт: Петров Пётр\n");
        stand.write("В.md", "Контакт: Петров Пётр\n");
        let mut anon = FakeAnon::default();
        let mut ext = FakeExtractor::default();
        assert_eq!(stand.run(&mut anon, &mut ext, 200).created, 3);
        // «В» получает правку агента → вариант в pending (статус pending_accept);
        // метка в правке остаётся, поэтому файл нельзя пересобирать
        stand.write_ws(
            "В.md",
            "<!-- hds-agent/P0001/3 -->\nКонтакт: [PERSON_2] — правка\n",
        );
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(report.variants, 1, "{report:?}");

        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut mirror = Mirror::new(
            "P0001",
            &stand.project,
            &stand.workspace,
            &registry,
            &mut anon,
            &mut ext,
            fast(),
            200,
        );
        let refresh = mirror
            .refresh_label_images(&["[PERSON_2]".to_string()])
            .expect("пересборка образов");
        assert_eq!(refresh.rebuilt.len(), 1, "{refresh:?}");
        assert_eq!(refresh.rebuilt[0].rel, "Б.md");
        assert_eq!(refresh.rebuilt[0].status, Status::Synced);
        // вариант агента не пересобирается: правки потерялись бы
        assert_eq!(refresh.skipped, vec!["В.md".to_string()]);
        // образ «В» с правкой на месте
        assert_eq!(
            fs::read_to_string(stand.workspace.join("В.md")).unwrap(),
            "<!-- hds-agent/P0001/3 -->\nКонтакт: [PERSON_2] — правка\n"
        );
        drop(registry);

        // После пересборки один проход — без изменений (критерий TR-2.6).
        assert_eq!(stand.run(&mut anon, &mut ext, 200).changes(), 0);
    }

    /// A5.5: очередь агенту — строка JSON, только метки, без значений (I8).
    #[test]
    fn append_message_writes_labels_only_queue() {
        let stand = Stand::new("a55-message");
        let message = append_message(
            &stand.project,
            MessageKind::LabelsMerged,
            "",
            crate::labels::merge_message("[PERSON_1]", &["[PERSON_2]".to_string()]),
        )
        .expect("сообщение в очередь");
        assert_eq!(message.rel, "");
        let text = messages(&stand);
        assert_eq!(text.lines().count(), 1, "{text}");
        let line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(line["kind"], "labels_merged");
        assert_eq!(line["rel"], "");
        let body = line["text"].as_str().unwrap();
        assert!(body.contains("[PERSON_2]"), "{body}");
        assert!(body.contains("[PERSON_1]"), "{body}");
        let unmerge = append_message(
            &stand.project,
            MessageKind::LabelsUnmerged,
            "",
            crate::labels::unmerge_message(&["[PERSON_2]".to_string()]),
        )
        .expect("второе сообщение");
        assert_eq!(unmerge.kind, MessageKind::LabelsUnmerged);
        assert_eq!(messages(&stand).lines().count(), 2);
    }

    /// A4.5: отчёт валидатора уходит вместе с вариантом — любым решением.
    #[test]
    fn decision_removes_validator_report() {
        let report_of =
            |stand: &Stand| crate::validator::report_path(&service(stand, "pending").join(rel()));

        let (stand, mut anon, mut ext) = stand_with_variant("a45-report-accept");
        let report = report_of(&stand);
        assert!(report.is_file(), "отчёт создан вместе с вариантом");
        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        assert!(!report.exists(), "после «Принять» отчёт не нужен");
        assert!(!service(&stand, "pending").join(rel()).exists());
        let left = fs::read_dir(service(&stand, "pending"))
            .map(|dir| dir.count())
            .unwrap_or(0);
        assert_eq!(left, 0, "каталог вариантов пуст: отчёт не пережил решение");

        let (stand, mut anon, mut ext) = stand_with_variant("a45-report-reject");
        let report = report_of(&stand);
        assert!(report.is_file(), "отчёт создан вместе с вариантом");
        decide(&stand, &mut anon, &mut ext, Decision::Reject).expect("отклонить");
        assert!(!report.exists(), "после «Отклонить» отчёт не нужен");
    }

    /// A5.4: «Сравнить» — метаданные обеих сторон, отчёт валидатора и diff.
    #[test]
    fn compare_reports_metadata_report_and_diff() {
        let (stand, mut anon, mut ext) = stand_with_variant("a54-compare");
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut mirror = Mirror::new(
            "P0001",
            &stand.project,
            &stand.workspace,
            &registry,
            &mut anon,
            &mut ext,
            fast(),
            200,
        );
        let report = mirror.compare(&rel()).expect("сравнение");
        assert_eq!(report.rel, rel());
        assert_eq!(report.status, Status::PendingAccept);
        assert!(
            report.original.exists && report.variant.exists,
            "{report:?}"
        );
        assert!(report.base.exists && report.image.exists, "{report:?}");
        let original_sha = report.original.sha256.clone().expect("sha оригинала");
        let variant_sha = report.variant.sha256.clone().expect("sha варианта");
        assert_ne!(original_sha, variant_sha);
        assert!(!report.identical);
        // Отчёт валидатора уже написан зеркалом (TR-7.8): четыре проверки.
        let validator = report.report.as_ref().expect("отчёт валидатора");
        assert_eq!(validator.checks.len(), 4, "{validator:?}");
        assert_eq!(validator.rel, rel());
        // Diff: удалённая и добавленная строки, без обрезки на маленьком файле.
        assert!(
            report.diff.iter().any(|line| line.starts_with("- ")),
            "{:?}",
            report.diff
        );
        assert!(
            report.diff.iter().any(|line| line.starts_with("+ ")),
            "{:?}",
            report.diff
        );
        assert!(!report.diff_truncated);
        assert!(report.diff_note.is_none(), "{report:?}");
    }

    /// A5.4: запись без варианта не мешает сравнению (в панели она не видна,
    /// но маршрут не должен падать), а неизвестный путь — 404-ошибка ядра.
    #[test]
    fn compare_tolerates_missing_variant_and_unknown_rel() {
        let (stand, mut anon, mut ext) = stand_with_variant("a54-compare-missing");
        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut mirror = Mirror::new(
            "P0001",
            &stand.project,
            &stand.workspace,
            &registry,
            &mut anon,
            &mut ext,
            fast(),
            200,
        );
        let report = mirror.compare(&rel()).expect("сравнение после решения");
        assert_eq!(report.status, Status::Accepted);
        assert!(!report.variant.exists, "вариант удалён решением");
        assert!(report.report.is_none());
        assert!(
            report.diff_note.is_some(),
            "варианта нет — почему diff пуст"
        );

        let err = mirror.compare("нет такого.md").expect_err("нет записи");
        assert!(matches!(err, ResultsError::NotFound(_)), "{err}");
        assert!(mirror.compare("../вне.md").is_err(), "путь наружу запрещён");
    }

    /// A5.4: лимиты diff — 200 строк и 2 000 символов, `truncated` честный.
    #[test]
    fn compare_diff_respects_limits() {
        let build = |extra: &str| {
            let mut text = String::new();
            for i in 0..1000 {
                text.push_str(&format!("строка {i}\n"));
            }
            text.push_str(extra);
            text
        };
        let (diff, truncated) = line_diff(&build(""), &build("добавлено\n"));
        assert!(truncated, "1000 строк не влезают в лимит");
        assert!(diff.len() <= DIFF_MAX_LINES, "{}", diff.len());
        let chars: usize = diff.iter().map(|line| line.chars().count()).sum();
        assert!(chars <= DIFF_MAX_CHARS, "{chars}");

        // Одинаковый текст — пустой diff, а не 200 строк контекста.
        let (same, truncated) = line_diff("одно\nдва\n", "одно\nдва\n");
        assert!(same.is_empty() && !truncated);
    }

    /// A5.4: разбор `decision`/`choice` из тела маршрута.
    #[test]
    fn decision_parse_accepts_known_forms() {
        assert_eq!(Decision::parse("accept", None), Some(Decision::Accept));
        assert_eq!(Decision::parse(" Reject ", None), Some(Decision::Reject));
        assert_eq!(
            Decision::parse("conflict", Some("agent")),
            Some(Decision::Conflict(ConflictChoice::Agent))
        );
        assert_eq!(
            Decision::parse("conflict", Some("delegate")),
            Some(Decision::Conflict(ConflictChoice::Delegate))
        );
        // Слитая форма — так решение попадает в журнал (`conflict:<choice>`).
        assert_eq!(
            Decision::parse("conflict:new", None),
            Some(Decision::Conflict(ConflictChoice::New))
        );
        assert_eq!(Decision::parse("conflict", None), None);
        assert_eq!(Decision::parse("conflict", Some("как-нибудь")), None);
        assert_eq!(Decision::parse("принять", None), None);
        assert_eq!(Decision::parse("", None), None);
    }

    /// A6.4: операция на стенде — тот же `Mirror`, что у CLI и вкладки.
    fn with_mirror<T>(
        stand: &Stand,
        anon: &mut FakeAnon,
        ext: &mut FakeExtractor,
        action: impl for<'m> FnOnce(&mut Mirror<'m>) -> T,
    ) -> T {
        let registry = ArtifactRegistry::open(&stand.db).unwrap();
        let mut mirror = Mirror::new(
            "P0001",
            &stand.project,
            &stand.workspace,
            &registry,
            anon,
            ext,
            fast(),
            200,
        );
        action(&mut mirror)
    }

    /// В-A6-4: копии видны списком (свежие первыми), фильтр по `rel` даёт sha,
    /// посторонние файлы в `backup` и пустой путь отбрасываются.
    #[test]
    fn backups_lists_copies_newest_first_and_filters_by_rel() {
        let (stand, mut anon, mut ext) = stand_with_variant("a64-backups");
        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        let root = service(&stand, BACKUP_SUBDIR);
        let old = root.join("20000101-000000").join(rel());
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, "Контакт: Иванов Иван — старая версия\n").unwrap();
        // Посторонний файл в корне backup — не копия и в список не попадает.
        fs::write(root.join("not-a-stamp"), "x").unwrap();

        let (all, filtered, none, bad_path) = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            (
                mirror.backups(None).unwrap(),
                mirror.backups(Some(&rel())).unwrap(),
                mirror.backups(Some("Нет такого.md")).unwrap(),
                mirror.backups(Some("")).is_err(),
            )
        });
        assert_eq!(all.len(), 2, "{all:?}");
        assert_eq!(filtered.len(), 2, "{filtered:?}");
        assert_ne!(all[0].stamp, "20000101-000000", "свежие штампы первыми");
        assert_eq!(all[0].stamp, filtered[0].stamp);
        assert!(
            all.iter().all(|copy| copy.sha256.is_none()),
            "список всех копий не читает содержимое"
        );
        assert!(
            filtered.iter().all(|copy| copy.sha256.is_some()),
            "копии одного файла — с sha256"
        );
        assert!(none.is_empty());
        assert!(bad_path, "пустой путь — ошибка пути");

        let old_copy = filtered
            .iter()
            .find(|copy| copy.stamp == "20000101-000000")
            .expect("старая копия");
        assert_eq!(old_copy.size, fs::metadata(&old).unwrap().len());
        assert_eq!(old_copy.path, old);
    }

    /// В-A6-4: «Вернуть» даёт sha выбранной копии, копия не тратится, образ
    /// пересоздаётся из восстановленного оригинала, журнал решений пишется.
    #[test]
    fn restore_returns_chosen_copy_and_rebuilds_image() {
        let (stand, mut anon, mut ext) = stand_with_variant("a64-restore");
        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        let accepted = fs::read_to_string(stand.project.join(rel())).unwrap();
        assert!(accepted.contains("правка агента"), "{accepted}");
        let copies = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            mirror.backups(Some(&rel())).unwrap()
        });
        let copy = copies[0].clone();
        let copy_sha = copy.sha256.clone().expect("sha копии");

        // Без --stamp — свежая копия.
        let report = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            mirror.restore(&rel(), None)
        })
        .expect("вернуть");
        assert_eq!(report.status, Status::Synced);
        assert_eq!(report.stamp.as_deref(), Some(copy.stamp.as_str()));
        assert_eq!(report.sha256.as_deref(), Some(copy_sha.as_str()));
        assert_eq!(
            sha256_file(&stand.project.join(rel())).unwrap(),
            copy_sha,
            "sha восстановленного оригинала = sha копии"
        );
        let restored = fs::read_to_string(stand.project.join(rel())).unwrap();
        assert!(restored.contains("Иванов Иван"), "{restored}");
        assert!(!restored.contains("правка агента"), "{restored}");
        assert!(copy.path.is_file(), "копия остаётся в backup");

        let image = stand.image(&anon_rel());
        assert!(image.contains("[PERSON_1]"), "{image}");
        assert!(
            !image.contains("правка агента"),
            "образ из восстановленного оригинала: {image}"
        );
        let image_sha = sha256_file(&stand.workspace.join(anon_rel())).unwrap();
        let registry = stand.registry();
        let row = registry.by_orig(&rel()).unwrap().unwrap();
        assert_eq!(row.status, Status::Synced);
        assert_eq!(row.anon_sha256_issued.as_deref(), Some(image_sha.as_str()));
        assert_eq!(row.anon_sha256_seen.as_deref(), Some(image_sha.as_str()));
        drop(registry);

        // Неизвестный штамп — отказ со списком доступных.
        let err = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            mirror.restore(&rel(), Some("19990101-000000"))
        })
        .expect_err("неизвестный штамп");
        match &err {
            ResultsError::NoBackup { available, .. } => {
                assert_eq!(available, &vec![copy.stamp.clone()], "{available:?}")
            }
            other => panic!("ожидался NoBackup: {other}"),
        }
        assert!(err.to_string().contains("доступны"), "{err}");

        // По штампу — та же копия.
        let report = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            mirror.restore(&rel(), Some(copy.stamp.as_str()))
        })
        .expect("по штампу");
        assert_eq!(report.stamp.as_deref(), Some(copy.stamp.as_str()));

        // Журнал решений: пути и статусы; текста документа и значений нет (I8).
        let journal = fs::read_to_string(service(&stand, DECISIONS_FILE)).unwrap();
        assert!(journal.contains("\"action\":\"accept\""), "{journal}");
        assert!(journal.contains("\"action\":\"restore\""), "{journal}");
        assert!(
            journal.contains(&format!("\"stamp\":\"{}\"", copy.stamp)),
            "штамп копии в записи accept: {journal}"
        );
        assert!(
            !journal.contains("Контакт:") && !journal.contains("правка агента"),
            "содержимое документа не пишется: {journal}"
        );
    }

    /// TR-7.6, случай 3: занятый файл при «Вернуть» — `Busy`, состояние прежнее.
    #[cfg(windows)]
    #[test]
    fn restore_on_busy_file_keeps_state() {
        use std::os::windows::fs::OpenOptionsExt;
        let (stand, mut anon, mut ext) = stand_with_variant("a64-busy");
        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        let original = stand.project.join(rel());
        let before = sha256_file(&original).unwrap();
        let status = stand.registry().by_orig(&rel()).unwrap().unwrap().status;
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0000_0001)
            .open(&original)
            .expect("открыть как Word");

        let err = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            mirror.restore(&rel(), None)
        })
        .expect_err("занят");
        assert!(matches!(err, ResultsError::Busy(_)), "{err}");
        assert_eq!(
            sha256_file(&original).unwrap(),
            before,
            "оригинал не изменён"
        );
        assert_eq!(
            stand.registry().by_orig(&rel()).unwrap().unwrap().status,
            status
        );
        let copies = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            mirror.backups(Some(&rel())).unwrap()
        });
        assert_eq!(copies.len(), 1, "копия на месте");
        let tmp = original.with_file_name(format!(
            ".{}.hds-restore.tmp",
            original.file_name().unwrap().to_string_lossy()
        ));
        assert!(!tmp.exists(), "временный файл убран");
        drop(held);

        let report = with_mirror(&stand, &mut anon, &mut ext, |mirror| {
            mirror.restore(&rel(), None)
        })
        .expect("после закрытия");
        assert_eq!(report.status, Status::Synced);
    }

    /// A6.4: «Пересоздать образ» — только `orphan`; у `pending_accept` отказ.
    #[test]
    fn rebuild_only_for_orphan() {
        let (stand, mut anon, mut ext) = stand_with_variant("a64-rebuild");
        let err = with_mirror(&stand, &mut anon, &mut ext, |mirror| mirror.rebuild(&rel()))
            .expect_err("не orphan");
        match &err {
            ResultsError::NotApplicable { status, op, .. } => {
                assert_eq!(*status, Status::PendingAccept);
                assert_eq!(*op, "rebuild");
            }
            other => panic!("ожидался отказ по статусу: {other}"),
        }

        // orphan: образ удалён агентом, оригинал остался.
        fs::remove_file(stand.workspace.join(anon_rel())).unwrap();
        stand
            .registry()
            .set_status_by_orig(&rel(), Status::Orphan)
            .unwrap();
        let report = with_mirror(&stand, &mut anon, &mut ext, |mirror| mirror.rebuild(&rel()))
            .expect("пересоздать");
        assert_eq!(report.status, Status::Synced);
        assert!(stand.workspace.join(anon_rel()).is_file(), "образ появился");
        let image = stand.image(&anon_rel());
        assert!(image.contains("[PERSON_1]"), "{image}");
        let registry = stand.registry();
        let row = registry.by_orig(&rel()).unwrap().unwrap();
        assert_eq!(row.status, Status::Synced);
        assert_eq!(row.anon_sha256_issued, row.anon_sha256_seen);
        drop(registry);
        assert!(report.sha256.is_some(), "в отчёте sha нового образа");

        // Журнал: операция rebuild записана.
        let journal = fs::read_to_string(service(&stand, DECISIONS_FILE)).unwrap();
        assert!(journal.contains("\"action\":\"rebuild\""), "{journal}");
    }

    /// В-A6-5: relink — только по команде. Проход зеркала запись `error` не
    /// восстанавливает, `relink` по своему маркеру — восстанавливает; чужой
    /// маркер и отсутствующий образ — понятный отказ.
    #[test]
    fn relink_by_marker_only_on_command() {
        let (stand, mut anon, mut ext) = stand_with_variant("a64-relink");
        decide(&stand, &mut anon, &mut ext, Decision::Accept).expect("принять");
        let row = stand.registry().by_orig(&rel()).unwrap().unwrap();
        let image_body = stand.image(&anon_rel());
        let marker = image_body.lines().next().unwrap_or("").to_string();
        assert!(
            marker.starts_with(&format!("<!-- hds-agent/P0001/{}", row.id)),
            "{marker}"
        );

        // Ошибка обработки: образ и оригинал сходятся, поэтому проход зеркала
        // запись не трогает — значит relink сам не срабатывает.
        stand
            .registry()
            .set_status_by_orig(&rel(), Status::Error)
            .unwrap();
        let report = stand.run(&mut anon, &mut ext, 200);
        assert_eq!(
            report.changes(),
            0,
            "в проходе relink не срабатывает: {report:?}"
        );
        assert_eq!(
            stand.registry().by_orig(&rel()).unwrap().unwrap().status,
            Status::Error,
            "проход оставил запись как есть"
        );

        let report = with_mirror(&stand, &mut anon, &mut ext, |mirror| mirror.relink(&rel()))
            .expect("relink по команде");
        assert_eq!(report.status, Status::Synced);
        let row = stand.registry().by_orig(&rel()).unwrap().unwrap();
        assert_eq!(row.status, Status::Synced);
        assert_eq!(row.anon_sha256_issued, row.anon_sha256_seen);

        // Чужой маркер — отказ, статус не меняется.
        stand
            .registry()
            .set_status_by_orig(&rel(), Status::Orphan)
            .unwrap();
        stand.write_ws(
            &anon_rel(),
            "<!-- hds-agent/P0002/1 -->\nКонтакт: [PERSON_1]\n",
        );
        let err = with_mirror(&stand, &mut anon, &mut ext, |mirror| mirror.relink(&rel()))
            .expect_err("чужой маркер");
        assert!(matches!(err, ResultsError::BadMarker(_)), "{err}");
        assert_eq!(
            stand.registry().by_orig(&rel()).unwrap().unwrap().status,
            Status::Orphan
        );

        // Образа нет — восстанавливать нечего.
        fs::remove_file(stand.workspace.join(anon_rel())).unwrap();
        let err = with_mirror(&stand, &mut anon, &mut ext, |mirror| mirror.relink(&rel()))
            .expect_err("нет образа");
        assert!(matches!(err, ResultsError::NoImage(_)), "{err}");

        // Не тот статус — отказ с текстом операции.
        let (stand2, mut anon2, mut ext2) = stand_with_variant("a64-relink-status");
        let err = with_mirror(&stand2, &mut anon2, &mut ext2, |mirror| {
            mirror.relink(&rel())
        })
        .expect_err("нечего восстанавливать");
        match &err {
            ResultsError::NotApplicable { op, .. } => assert_eq!(*op, "relink"),
            other => panic!("ожидался отказ по статусу: {other}"),
        }
    }

    /// В-A6-6: журнал решений — JSONL, ротация при 1 МБ, только пути/статусы.
    #[test]
    fn decisions_journal_rotates_and_keeps_paths_only() {
        let stand = Stand::new("a64-journal");
        let entry = DecisionEntry {
            ts: 1.0,
            action: "restore".to_string(),
            orig_rel: "Документы\\Письмо [PERSON_1].md".to_string(),
            stamp: Some("20260101-120000".to_string()),
            status: "synced".to_string(),
            image: Some("Документы\\Письмо [PERSON_1].md".to_string()),
            extra: Some("sha256=abc123".to_string()),
        };
        append_decision(&stand.project, &entry).expect("запись журнала");
        let path = service(&stand, DECISIONS_FILE);
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        let line: DecisionEntry = serde_json::from_str(text.trim()).expect("JSONL");
        assert_eq!(line, entry);
        assert!(
            !text.contains("Иванов"),
            "только пути/статусы/числа (I8): {text}"
        );

        // Ротация: прежний журнал уезжает в `.1`, новый начинается с нуля.
        fs::write(&path, vec![b'x'; (DECISIONS_ROTATE_BYTES + 1) as usize]).unwrap();
        append_decision(&stand.project, &entry).expect("после ротации");
        assert!(
            PathBuf::from(format!("{}.1", path.display())).is_file(),
            "прежний журнал в .1"
        );
        let fresh = fs::read_to_string(&path).unwrap();
        assert_eq!(fresh.lines().count(), 1, "новый журнал — одна строка");
    }
}
