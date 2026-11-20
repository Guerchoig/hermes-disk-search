//! Реестр артефактов зеркала — `data\agent\registry\<P>.db` (TR-7.2, фаза A4).
//!
//! Таблица одна (`artifacts`); пишут только демон `hds agent sync` и кнопки
//! вкладки «Агент» (A5). Поля и их смысл:
//!
//! * `orig_rel` — путь файла в папке проекта, `anon_rel` — путь образа в
//!   `<P>\workspace`. Связь «оригинал ↔ образ» хранится **здесь**, а не
//!   вычисляется из имени: имена анонимизированы, коллизии разрешаются
//!   суффиксом ` (2)` (TR-7.1);
//! * `orig_sha256` — sha256 оригинала, из которого сделан образ (основание,
//!   TR-7.6); у файлов, выпущенных агентом в папку проекта, — sha256 выпуска
//!   (TR-7.7);
//! * `anon_sha256_issued` — что выпустил демон, `anon_sha256_seen` — что
//!   видели в последнем обходе: правка агентом = `seen != issued` (TR-7.7);
//! * `status` — состояние из списка TR-7.2;
//! * `ts` — время последнего изменения записи (Unix-секунды с дробной частью).
//!
//! `artifact_id` (`id`) выделяет демон **до** анонимизации и передаёт в
//! `anonymize {mode:"agent", artifact_id}`: маркер `hds-agent/<P>/<id>` в
//! свойствах образа связывает файл с реестром даже после потери БД.
//!
//! Ошибка БД не валит демон: вызывающий превращает её в статус `error` и
//! продолжает обход (TR-7.2). В журнал попадают путь БД, относительные пути и
//! статусы — не содержимое файлов (I8).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Версия схемы (`PRAGMA user_version`). Растёт при несовместимых правках.
pub const SCHEMA_VERSION: i64 = 1;

/// Схема таблицы реестра (TR-7.2).
const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS artifacts (
    id                 INTEGER PRIMARY KEY,
    orig_rel           TEXT NOT NULL UNIQUE,
    anon_rel           TEXT NOT NULL,
    kind               TEXT NOT NULL,
    orig_sha256        TEXT,
    anon_sha256_issued TEXT,
    anon_sha256_seen   TEXT,
    status             TEXT NOT NULL,
    ts                 REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS artifacts_anon ON artifacts(anon_rel);
CREATE INDEX IF NOT EXISTS artifacts_status ON artifacts(status);
";

/// Вид артефакта (TR-7.2): `image` — образ того же формата, `text_image` —
/// `*.pdf.md`/`*.pptx.md`/`*.mpp.md`, `agent_new` — файл, созданный агентом,
/// `attachment` — вложение из чата (`workspace\_attachments`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Image,
    TextImage,
    AgentNew,
    Attachment,
    /// Вид из более новой версии: читается, но демон такой записью не управляет.
    Unknown,
}

impl Kind {
    /// Строка для БД и журнала.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::TextImage => "text_image",
            Kind::AgentNew => "agent_new",
            Kind::Attachment => "attachment",
            Kind::Unknown => "unknown",
        }
    }

    /// Разбор значения из БД; неизвестное — [`Kind::Unknown`], а не ошибка:
    /// реестр новее демона не должен останавливать обход.
    pub fn parse(s: &str) -> Kind {
        match s.trim().to_ascii_lowercase().as_str() {
            "image" => Kind::Image,
            "text_image" => Kind::TextImage,
            "agent_new" => Kind::AgentNew,
            "attachment" => Kind::Attachment,
            _ => Kind::Unknown,
        }
    }
}

/// Состояние артефакта (TR-7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Образ выпущен и совпадает с тем, что выпустил демон.
    Synced,
    /// Формат не поддержан зеркалом.
    Unsupported,
    /// Файл больше `index.max_file_mb`.
    TooBig,
    /// Новый файл агента, ещё не выпущен в папку проекта.
    AgentNew,
    /// Агент изменил образ; вариант собирается/собран в `pending`.
    AgentChanged,
    /// Вариант лежит в `.hds-agent\pending` и ждёт решения (TR-7.6).
    PendingAccept,
    /// Вариант принят: оригинал заменён.
    Accepted,
    /// Оригинал и образ разошлись (TR-7.6, случай 2).
    Conflict,
    /// Образ удалён агентом, оригинал оставлен.
    Orphan,
    /// Ошибка обработки (БД, NER, файл занят — с записью в журнал).
    Error,
    /// Значение из более новой версии: читается, демон его не меняет.
    Unknown,
}

impl Status {
    /// Строка для БД и журнала.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Synced => "synced",
            Status::Unsupported => "unsupported",
            Status::TooBig => "too_big",
            Status::AgentNew => "agent_new",
            Status::AgentChanged => "agent_changed",
            Status::PendingAccept => "pending_accept",
            Status::Accepted => "accepted",
            Status::Conflict => "conflict",
            Status::Orphan => "orphan",
            Status::Error => "error",
            Status::Unknown => "unknown",
        }
    }

    /// Разбор значения из БД (неизвестное — [`Status::Unknown`]).
    pub fn parse(s: &str) -> Status {
        match s.trim().to_ascii_lowercase().as_str() {
            "synced" => Status::Synced,
            "unsupported" => Status::Unsupported,
            "too_big" => Status::TooBig,
            "agent_new" => Status::AgentNew,
            "agent_changed" => Status::AgentChanged,
            "pending_accept" => Status::PendingAccept,
            "accepted" => Status::Accepted,
            "conflict" => Status::Conflict,
            "orphan" => Status::Orphan,
            "error" => Status::Error,
            _ => Status::Unknown,
        }
    }
}

/// Запись реестра (TR-7.2).
///
/// `id` == 0 у новой записи: [`ArtifactRegistry::insert`] выделит его, а
/// [`ArtifactRegistry::upsert`] найдёт существующую запись по `orig_rel`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Artifact {
    /// `artifact_id` для маркера `hds-agent/<P>/<id>`; 0 — «ещё не выделен».
    pub id: i64,
    /// Относительный путь в папке проекта (`Протоколы\Встреча.docx`).
    pub orig_rel: String,
    /// Относительный путь образа в `<P>\workspace`.
    pub anon_rel: String,
    /// Вид артефакта.
    pub kind: Kind,
    /// sha256 оригинала-основания (у `agent_new` — sha256 выпуска).
    pub orig_sha256: Option<String>,
    /// sha256 того, что демон записал в workspace.
    pub anon_sha256_issued: Option<String>,
    /// sha256 того, что видели в последнем обходе.
    pub anon_sha256_seen: Option<String>,
    /// Состояние (TR-7.2).
    pub status: Status,
    /// Unix-время последнего изменения записи (секунды, с дробной частью).
    pub ts: f64,
}

impl Artifact {
    /// Новая запись без хэшей (id выделит реестр).
    pub fn new(orig_rel: &str, anon_rel: &str, kind: Kind, status: Status) -> Artifact {
        Artifact {
            id: 0,
            orig_rel: orig_rel.to_string(),
            anon_rel: anon_rel.to_string(),
            kind,
            orig_sha256: None,
            anon_sha256_issued: None,
            anon_sha256_seen: None,
            status,
            ts: 0.0,
        }
    }

    /// Тот же артефакт с другим статусом (удобно в тестах и вызовах демона).
    pub fn with_status(mut self, status: Status) -> Artifact {
        self.status = status;
        self
    }

    /// Совпадают ли выпущенный и увиденный образы (изменение агентом, TR-7.7).
    /// `None` хотя бы с одной стороны — «неизвестно», сравнение невозможно.
    pub fn agent_changed(&self) -> bool {
        match (&self.anon_sha256_issued, &self.anon_sha256_seen) {
            (Some(issued), Some(seen)) => issued != seen,
            _ => false,
        }
    }
}

/// Причина отказа реестра.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// БД не открылась (каталог, права, битый файл) или схема не создалась.
    Open(PathBuf, String),
    /// Ошибка SQL.
    Sql(String),
    /// Запись не найдена.
    NotFound(i64),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Open(path, why) => {
                write!(f, "реестр не открылся ({}): {why}", path.display())
            }
            RegistryError::Sql(why) => write!(f, "ошибка SQL реестра: {why}"),
            RegistryError::NotFound(id) => write!(f, "запись реестра {id} не найдена"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<rusqlite::Error> for RegistryError {
    fn from(err: rusqlite::Error) -> RegistryError {
        RegistryError::Sql(err.to_string())
    }
}

/// Unix-время сейчас (секунды с дробной частью).
fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Соответствие колонок `SELECT`: id, orig_rel, anon_rel, kind, orig_sha256,
/// anon_sha256_issued, anon_sha256_seen, status, ts.
const SELECT_COLUMNS: &str = "id, orig_rel, anon_rel, kind, orig_sha256, \
                              anon_sha256_issued, anon_sha256_seen, status, ts";

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: row.get(0)?,
        orig_rel: row.get(1)?,
        anon_rel: row.get(2)?,
        kind: Kind::parse(&row.get::<_, String>(3)?),
        orig_sha256: row.get(4)?,
        anon_sha256_issued: row.get(5)?,
        anon_sha256_seen: row.get(6)?,
        status: Status::parse(&row.get::<_, String>(7)?),
        ts: row.get(8)?,
    })
}

/// Открытый реестр проекта.
///
/// `rusqlite::Connection` не `Sync`: демон держит один реестр и работает с ним
/// из одного потока (как `hds_index::watch`). Параллельные писатели — это
/// отдельные открытия той же БД; их разводит WAL + `busy_timeout` (есть тест).
pub struct ArtifactRegistry {
    conn: Connection,
    path: PathBuf,
}

impl ArtifactRegistry {
    /// Открыть (или создать) реестр: каталог, схема, WAL, `busy_timeout`.
    pub fn open(path: &Path) -> Result<ArtifactRegistry, RegistryError> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| RegistryError::Open(path.to_path_buf(), e.to_string()))?;
            }
        }
        let conn = Connection::open(path)
            .map_err(|e| RegistryError::Open(path.to_path_buf(), e.to_string()))?;
        // `busy_timeout` — первым: смена `journal_mode` берёт блокировку БД,
        // а до установки таймаута конкурентная запись падала бы сразу.
        conn.execute_batch(
            "PRAGMA busy_timeout=30000;\n\
             PRAGMA journal_mode=WAL;\n\
             PRAGMA synchronous=NORMAL;\n\
             PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| RegistryError::Open(path.to_path_buf(), e.to_string()))?;
        conn.execute_batch(SCHEMA)
            .map_err(|e| RegistryError::Open(path.to_path_buf(), e.to_string()))?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| RegistryError::Open(path.to_path_buf(), e.to_string()))?;
        Ok(ArtifactRegistry {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Путь к файлу БД.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Версия схемы из файла (`PRAGMA user_version`).
    pub fn schema_version(&self) -> Result<i64, RegistryError> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    /// Добавить запись; `id` артефакта игнорируется, возвращается выделенный.
    ///
    /// Вызывается **до** анонимизации: id уходит в маркер
    /// `hds-agent/<P>/<id>` (TR-7.4).
    pub fn insert(&self, artifact: &Artifact) -> Result<i64, RegistryError> {
        let ts = if artifact.ts > 0.0 {
            artifact.ts
        } else {
            now_ts()
        };
        self.conn.execute(
            "INSERT INTO artifacts (orig_rel, anon_rel, kind, orig_sha256, \
             anon_sha256_issued, anon_sha256_seen, status, ts) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                artifact.orig_rel,
                artifact.anon_rel,
                artifact.kind.as_str(),
                artifact.orig_sha256,
                artifact.anon_sha256_issued,
                artifact.anon_sha256_seen,
                artifact.status.as_str(),
                ts
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Добавить или обновить запись по `orig_rel`; id существующей сохраняется.
    ///
    /// `None` в хэшах не стирает уже известное значение (`COALESCE`): демон
    /// часто знает только часть (основание есть, `seen` ещё нет).
    pub fn upsert(&self, artifact: &Artifact) -> Result<i64, RegistryError> {
        let ts = if artifact.ts > 0.0 {
            artifact.ts
        } else {
            now_ts()
        };
        self.conn.execute(
            "INSERT INTO artifacts (orig_rel, anon_rel, kind, orig_sha256, \
             anon_sha256_issued, anon_sha256_seen, status, ts) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(orig_rel) DO UPDATE SET \
                anon_rel = excluded.anon_rel, \
                kind = excluded.kind, \
                orig_sha256 = COALESCE(excluded.orig_sha256, artifacts.orig_sha256), \
                anon_sha256_issued = COALESCE(excluded.anon_sha256_issued, \
                                              artifacts.anon_sha256_issued), \
                anon_sha256_seen = COALESCE(excluded.anon_sha256_seen, \
                                            artifacts.anon_sha256_seen), \
                status = excluded.status, \
                ts = excluded.ts",
            params![
                artifact.orig_rel,
                artifact.anon_rel,
                artifact.kind.as_str(),
                artifact.orig_sha256,
                artifact.anon_sha256_issued,
                artifact.anon_sha256_seen,
                artifact.status.as_str(),
                ts
            ],
        )?;
        match self.by_orig(&artifact.orig_rel)? {
            Some(found) => Ok(found.id),
            None => Err(RegistryError::Sql(
                "upsert не нашёл запись после вставки".to_string(),
            )),
        }
    }

    /// Запись по `artifact_id`.
    pub fn by_id(&self, id: i64) -> Result<Option<Artifact>, RegistryError> {
        let value = self
            .conn
            .query_row(
                &format!("SELECT {SELECT_COLUMNS} FROM artifacts WHERE id = ?1"),
                params![id],
                row_to_artifact,
            )
            .optional()?;
        Ok(value)
    }

    /// Запись по относительному пути оригинала (папка проекта).
    pub fn by_orig(&self, orig_rel: &str) -> Result<Option<Artifact>, RegistryError> {
        self.one(
            &format!("SELECT {SELECT_COLUMNS} FROM artifacts WHERE orig_rel = ?1"),
            orig_rel,
        )
    }

    /// Запись по относительному пути образа (workspace).
    pub fn by_anon(&self, anon_rel: &str) -> Result<Option<Artifact>, RegistryError> {
        self.one(
            &format!(
                "SELECT {SELECT_COLUMNS} FROM artifacts WHERE anon_rel = ?1 \
                 ORDER BY id LIMIT 1"
            ),
            anon_rel,
        )
    }

    /// Сменить статус по `artifact_id`; `true`, если запись есть.
    pub fn set_status(&self, id: i64, status: Status) -> Result<bool, RegistryError> {
        let changed = self.conn.execute(
            "UPDATE artifacts SET status = ?2, ts = ?3 WHERE id = ?1",
            params![id, status.as_str(), now_ts()],
        )?;
        Ok(changed > 0)
    }

    /// Сменить статус по пути оригинала; `true`, если запись есть.
    pub fn set_status_by_orig(
        &self,
        orig_rel: &str,
        status: Status,
    ) -> Result<bool, RegistryError> {
        let changed = self.conn.execute(
            "UPDATE artifacts SET status = ?2, ts = ?3 WHERE orig_rel = ?1",
            params![orig_rel, status.as_str(), now_ts()],
        )?;
        Ok(changed > 0)
    }

    /// Записать, что демон выпустил образ: `anon_sha256_issued` и статус.
    pub fn set_issued(
        &self,
        id: i64,
        anon_sha256: &str,
        status: Status,
    ) -> Result<(), RegistryError> {
        let changed = self.conn.execute(
            "UPDATE artifacts SET anon_sha256_issued = ?2, anon_sha256_seen = ?2, \
             status = ?3, ts = ?4 WHERE id = ?1",
            params![id, anon_sha256, status.as_str(), now_ts()],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound(id));
        }
        Ok(())
    }

    /// Записать новое **основание** и образ, пересозданный из него (A4.4).
    ///
    /// Отличие от [`ArtifactRegistry::set_issued`]: «Принять», «Взять новую
    /// версию» и «Поручить агенту» могут пересоздать образ из **другого**
    /// оригинала (пользователь успел его изменить — TR-7.6, случай 2), поэтому
    /// `orig_sha256` тоже обновляется. Иначе следующий проход считал бы
    /// оригинал изменившимся и пересоздавал образ заново — «повторный sync —
    /// 0 изменений» перестало бы выполняться.
    pub fn set_baseline(
        &self,
        id: i64,
        orig_sha256: &str,
        anon_sha256: &str,
        status: Status,
    ) -> Result<(), RegistryError> {
        let changed = self.conn.execute(
            "UPDATE artifacts SET orig_sha256 = ?2, anon_sha256_issued = ?3, \
             anon_sha256_seen = ?3, status = ?4, ts = ?5 WHERE id = ?1",
            params![id, orig_sha256, anon_sha256, status.as_str(), now_ts()],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound(id));
        }
        Ok(())
    }

    pub fn mark_seen(&self, id: i64, anon_sha256: Option<&str>) -> Result<(), RegistryError> {
        let changed = self.conn.execute(
            "UPDATE artifacts SET anon_sha256_seen = ?2, ts = ?3 WHERE id = ?1",
            params![id, anon_sha256, now_ts()],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound(id));
        }
        Ok(())
    }

    /// Записать основание (`orig_sha256`) — версия оригинала, из которой
    /// сделан образ (TR-7.6).
    pub fn set_base(&self, id: i64, orig_sha256: &str) -> Result<(), RegistryError> {
        let changed = self.conn.execute(
            "UPDATE artifacts SET orig_sha256 = ?2, ts = ?3 WHERE id = ?1",
            params![id, orig_sha256, now_ts()],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound(id));
        }
        Ok(())
    }

    /// Перенести запись на новый путь оригинала (переименование/перемещение,
    /// TR-7.3) — образ переименовывается тем же вызовом.
    pub fn rename_orig(
        &self,
        id: i64,
        orig_rel: &str,
        anon_rel: &str,
    ) -> Result<(), RegistryError> {
        let changed = self.conn.execute(
            "UPDATE artifacts SET orig_rel = ?2, anon_rel = ?3, ts = ?4 WHERE id = ?1",
            params![id, orig_rel, anon_rel, now_ts()],
        )?;
        if changed == 0 {
            return Err(RegistryError::NotFound(id));
        }
        Ok(())
    }

    /// Все записи (для панели «Файлы проекта», A5, и отчётов).
    pub fn list(&self) -> Result<Vec<Artifact>, RegistryError> {
        self.collect(
            &format!("SELECT {SELECT_COLUMNS} FROM artifacts ORDER BY orig_rel"),
            None,
        )
    }

    /// Записи с заданным статусом.
    pub fn list_by_status(&self, status: Status) -> Result<Vec<Artifact>, RegistryError> {
        self.collect(
            &format!(
                "SELECT {SELECT_COLUMNS} FROM artifacts WHERE status = ?1 \
                 ORDER BY ts, orig_rel"
            ),
            Some(status.as_str()),
        )
    }

    /// Варианты, ждущие решения пользователя (`pending_accept`, TR-7.6).
    pub fn list_pending(&self) -> Result<Vec<Artifact>, RegistryError> {
        self.list_by_status(Status::PendingAccept)
    }

    /// Число записей.
    pub fn count(&self) -> Result<i64, RegistryError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get(0))?)
    }

    /// Удалить запись; `true`, если она была.
    pub fn delete(&self, id: i64) -> Result<bool, RegistryError> {
        let changed = self
            .conn
            .execute("DELETE FROM artifacts WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    /// Одна запись по строковому ключу (общий путь `by_orig`/`by_anon`).
    fn one(&self, sql: &str, key: &str) -> Result<Option<Artifact>, RegistryError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params![key])?;
        match rows.next()? {
            Some(row) => Ok(Some(row_to_artifact(row)?)),
            None => Ok(None),
        }
    }

    /// Несколько записей; `key = None` — без условия.
    fn collect(&self, sql: &str, key: Option<&str>) -> Result<Vec<Artifact>, RegistryError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = match key {
            Some(value) => stmt.query(params![value])?,
            None => stmt.query([])?,
        };
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(row_to_artifact(row)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Тесты одного файла идут параллельно в одном процессе — общий
    /// временный каталог приводил бы к удалению чужих файлов (A3 §4 п.16),
    /// поэтому в имени есть счётчик, а не только pid.
    fn tmpdir(name: &str) -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("hds-agent-a41-{name}-{}-{seq}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(orig: &str) -> Artifact {
        let mut a = Artifact::new(orig, &format!("Копии\\{orig}"), Kind::Image, Status::Synced);
        a.orig_sha256 = Some("base-1".to_string());
        a.anon_sha256_issued = Some("issued-1".to_string());
        a.anon_sha256_seen = Some("issued-1".to_string());
        a
    }

    #[test]
    fn schema_strings_match_plan() {
        // Контракт TR-7.2: имена видов и статусов не меняются (их видят UI и
        // журнал).
        let kinds: Vec<&str> = [
            Kind::Image,
            Kind::TextImage,
            Kind::AgentNew,
            Kind::Attachment,
        ]
        .iter()
        .map(|k| k.as_str())
        .collect();
        assert_eq!(
            kinds,
            vec!["image", "text_image", "agent_new", "attachment"]
        );
        let statuses: Vec<&str> = [
            Status::Synced,
            Status::Unsupported,
            Status::TooBig,
            Status::AgentNew,
            Status::AgentChanged,
            Status::PendingAccept,
            Status::Accepted,
            Status::Conflict,
            Status::Orphan,
            Status::Error,
        ]
        .iter()
        .map(|s| s.as_str())
        .collect();
        assert_eq!(
            statuses,
            vec![
                "synced",
                "unsupported",
                "too_big",
                "agent_new",
                "agent_changed",
                "pending_accept",
                "accepted",
                "conflict",
                "orphan",
                "error"
            ]
        );
        for kind in [
            Kind::Image,
            Kind::TextImage,
            Kind::AgentNew,
            Kind::Attachment,
        ] {
            assert_eq!(Kind::parse(kind.as_str()), kind);
        }
        for status in [
            Status::Synced,
            Status::Unsupported,
            Status::TooBig,
            Status::AgentNew,
            Status::AgentChanged,
            Status::PendingAccept,
            Status::Accepted,
            Status::Conflict,
            Status::Orphan,
            Status::Error,
        ] {
            assert_eq!(Status::parse(status.as_str()), status);
        }
        // Неизвестное (реестр новее демона) не падает.
        assert_eq!(Kind::parse("video"), Kind::Unknown);
        assert_eq!(Status::parse("ignored"), Status::Unknown);
    }

    #[test]
    fn open_creates_dir_schema_and_wal() {
        let dir = tmpdir("open");
        let path = dir.join("registry").join("P0001.db");
        let reg = ArtifactRegistry::open(&path).unwrap();
        assert!(path.is_file(), "файл БД должен появиться");
        assert_eq!(reg.path(), path.as_path());
        assert_eq!(reg.schema_version().unwrap(), SCHEMA_VERSION);
        let mode: String = reg
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        let busy: i64 = reg
            .conn
            .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
            .unwrap();
        assert!(busy >= 5000, "busy_timeout={busy} — ожидался ≥5000 мс");
        assert_eq!(reg.count().unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn insert_allocates_id_and_roundtrips() {
        let dir = tmpdir("insert");
        let reg = ArtifactRegistry::open(&dir.join("P0001.db")).unwrap();
        let a = sample("Протоколы\\Встреча.docx");
        let id = reg.insert(&a).unwrap();
        assert!(id > 0);
        let by_id = reg.by_id(id).unwrap().expect("запись по id");
        assert_eq!(by_id.orig_rel, a.orig_rel);
        assert_eq!(by_id.anon_rel, a.anon_rel);
        assert_eq!(by_id.kind, Kind::Image);
        assert_eq!(by_id.status, Status::Synced);
        assert!(by_id.ts > 0.0, "ts заполняется, если не задан");
        let by_orig = reg.by_orig(&a.orig_rel).unwrap().expect("запись по orig");
        assert_eq!(by_orig.id, id);
        let by_anon = reg.by_anon(&a.anon_rel).unwrap().expect("запись по anon");
        assert_eq!(by_anon.id, id);
        // Повторная вставка того же orig_rel — конфликт уникальности, а не
        // второй ряд.
        assert!(reg.insert(&a).is_err());
        assert_eq!(reg.count().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_keeps_id_and_does_not_erase_hashes() {
        let dir = tmpdir("upsert");
        let reg = ArtifactRegistry::open(&dir.join("P0001.db")).unwrap();
        let mut a = sample("ТЗ.docx");
        a.status = Status::AgentChanged;
        let first = reg.upsert(&a).unwrap();

        // Второй upsert знает только статус: хэши должны сохраниться.
        let mut partial = Artifact::new(&a.orig_rel, &a.anon_rel, Kind::Image, Status::Synced);
        partial.anon_sha256_seen = Some("seen-2".to_string());
        let second = reg.upsert(&partial).unwrap();
        assert_eq!(first, second, "id записи не меняется");

        let stored = reg.by_orig(&a.orig_rel).unwrap().unwrap();
        assert_eq!(stored.status, Status::Synced);
        assert_eq!(stored.orig_sha256.as_deref(), Some("base-1"));
        assert_eq!(stored.anon_sha256_issued.as_deref(), Some("issued-1"));
        assert_eq!(stored.anon_sha256_seen.as_deref(), Some("seen-2"));
        assert!(stored.agent_changed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn issued_and_seen_detect_agent_change() {
        let dir = tmpdir("seen");
        let reg = ArtifactRegistry::open(&dir.join("P0001.db")).unwrap();
        let id = reg.insert(&sample("Отчёт.docx")).unwrap();
        let issued = reg.by_id(id).unwrap().unwrap();
        assert!(!issued.agent_changed(), "сразу после выпуска правки нет");

        reg.mark_seen(id, Some("hash-after-agent")).unwrap();
        let seen = reg.by_id(id).unwrap().unwrap();
        assert!(seen.agent_changed());
        assert_eq!(seen.anon_sha256_issued.as_deref(), Some("issued-1"));

        // Выпуск новой версии выравнивает seen = issued (петли нет, TR-7.7).
        reg.set_issued(id, "hash-new", Status::Synced).unwrap();
        let fresh = reg.by_id(id).unwrap().unwrap();
        assert!(!fresh.agent_changed());
        assert_eq!(fresh.anon_sha256_issued.as_deref(), Some("hash-new"));
        assert_eq!(fresh.anon_sha256_seen.as_deref(), Some("hash-new"));
        assert!(reg.mark_seen(999, None).is_err(), "чужой id — ошибка");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_moves_and_pending_list() {
        let dir = tmpdir("status");
        let reg = ArtifactRegistry::open(&dir.join("P0001.db")).unwrap();
        let id = reg.insert(&sample("Письмо.docx")).unwrap();
        let other = reg
            .insert(&sample("Письмо 2.docx").with_status(Status::AgentNew))
            .unwrap();

        assert!(reg.set_status(id, Status::PendingAccept).unwrap());
        assert!(!reg.set_status(999_999, Status::Error).unwrap());
        assert!(reg
            .set_status_by_orig("Письмо 2.docx", Status::TooBig)
            .unwrap());

        let pending = reg.list_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(reg.list_by_status(Status::TooBig).unwrap().len(), 1);
        assert_eq!(reg.list().unwrap().len(), 2);

        // Принятый вариант из pending уходит.
        assert!(reg.set_status(id, Status::Accepted).unwrap());
        assert!(reg.list_pending().unwrap().is_empty());

        // Переименование: пути меняются, id тот же.
        reg.rename_orig(id, "Архив\\Письмо.docx", "Архив\\[DOC_1].docx")
            .unwrap();
        let renamed = reg.by_id(id).unwrap().unwrap();
        assert_eq!(renamed.orig_rel, "Архив\\Письмо.docx");
        assert_eq!(renamed.anon_rel, "Архив\\[DOC_1].docx");
        assert!(reg.by_orig("Письмо.docx").unwrap().is_none());

        // Удаление (orphan/очистка) — по id.
        assert!(reg.delete(other).unwrap());
        assert!(!reg.delete(other).unwrap());
        assert_eq!(reg.count().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopen_keeps_rows_and_unknown_values_survive() {
        let dir = tmpdir("reopen");
        let path = dir.join("P0001.db");
        {
            let reg = ArtifactRegistry::open(&path).unwrap();
            reg.insert(&sample("Устав.docx")).unwrap();
            // Значение из будущей версии: читается без падения.
            reg.conn
                .execute(
                    "INSERT INTO artifacts (orig_rel, anon_rel, kind, status, ts) \
                     VALUES ('Новое.docx', 'Новое.docx', 'video', 'ignored', 1.0)",
                    [],
                )
                .unwrap();
        }
        let reg = ArtifactRegistry::open(&path).unwrap();
        assert_eq!(reg.count().unwrap(), 2);
        let future = reg.by_orig("Новое.docx").unwrap().unwrap();
        assert_eq!(future.kind, Kind::Unknown);
        assert_eq!(future.status, Status::Unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_writers_do_not_lose_rows() {
        let dir = tmpdir("concurrent");
        let path = dir.join("P0001.db");
        // Схему создаём до старта потоков: тест про конкурентную **запись**,
        // а не про одновременный `CREATE TABLE`.
        ArtifactRegistry::open(&path).unwrap();
        std::thread::scope(|scope| {
            for worker in 0..2 {
                let path = path.clone();
                scope.spawn(move || {
                    let reg = ArtifactRegistry::open(&path).expect("открыть реестр");
                    for i in 0..20 {
                        let rel = format!("Протоколы\\t{worker}-{i}.docx");
                        reg.upsert(&sample(&rel)).expect("upsert из потока");
                    }
                });
            }
        });
        let reg = ArtifactRegistry::open(&path).unwrap();
        assert_eq!(reg.count().unwrap(), 40, "WAL + busy_timeout держат запись");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_messages_are_actionable() {
        let err = RegistryError::NotFound(7);
        assert!(err.to_string().contains('7'));
        let err = RegistryError::Open(PathBuf::from(r"C:\x\P0001.db"), "нет прав".into());
        assert!(err.to_string().contains("P0001.db"));
        assert!(err.to_string().contains("нет прав"));
    }
}
