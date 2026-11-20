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

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use hds_core::config::replace_file;
use hds_core::error::CoreError;
use hds_core::timefmt::{now_epoch, stamp_local};
use serde::Serialize;

use crate::mirror::{
    copy_file, kind_for, rel_to_path, sha256_file, split_name, split_path, Mirror, MirrorError,
    ScannedFile,
};
use crate::registry_db::{Artifact, Kind, RegistryError, Status};

/// Служебный каталог проекта (TR-7.1): не зеркалится и не индексируется.
pub const SERVICE_DIR: &str = ".hds-agent";
/// Очередь сообщений агенту внутри [`SERVICE_DIR`] (решение В-6).
pub const MESSAGES_FILE: &str = "messages.jsonl";
/// Копии прежних версий оригиналов: подкаталог [`SERVICE_DIR`] (TR-7.6).
pub const BACKUP_SUBDIR: &str = "backup";
/// Каталог workspace для вариантов при конфликте (TR-7.1/7.6).
pub const CONFLICTS_DIR: &str = "_conflicts";
/// Суффикс анонимной копии варианта агента в [`CONFLICTS_DIR`].
pub const CONFLICT_SUFFIX: &str = " (вариант агента)";

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Правки не приняты, образ восстановлен/пересоздан из оригинала.
    DocumentRejected,
    /// Документ обновлён пользователем, правки переносить из `_conflicts`.
    ConflictDelegated,
}

impl MessageKind {
    /// Строка для БД/журнала и тестов.
    pub fn as_str(self) -> &'static str {
        match self {
            MessageKind::DocumentRejected => "document_rejected",
            MessageKind::ConflictDelegated => "conflict_delegated",
        }
    }

    /// Разбор строки из очереди (A5 будет доставлять эти же записи).
    pub fn parse(s: &str) -> Option<MessageKind> {
        match s.trim() {
            "document_rejected" => Some(MessageKind::DocumentRejected),
            "conflict_delegated" => Some(MessageKind::ConflictDelegated),
            _ => None,
        }
    }
}

/// Запись очереди сообщений агенту (В-6): по строке JSON на событие.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
            conflict_copy: None,
            message: None,
            warnings: Vec::new(),
        }
    }
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
        match decision {
            Decision::Accept => self.accept_variant(&row),
            Decision::Reject => self.reject_variant(&row),
            Decision::Conflict(choice) => self.resolve_conflict(&row, choice),
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

    /// Убрать вариант и основание: после решения они не нужны.
    fn cleanup_decision(&self, row: &Artifact) {
        let pending_root = self.project_root.join(SERVICE_DIR).join("pending");
        let pending = self.pending_path(&row.orig_rel);
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
        let message = Message {
            ts: now_epoch(),
            kind,
            rel: report.anon_rel.clone(),
            text,
        };
        let dir = self.project_root.join(SERVICE_DIR);
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
        report.message = Some(message);
        Ok(())
    }
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
        assert_eq!(MessageKind::parse("что-то"), None);
    }
}
