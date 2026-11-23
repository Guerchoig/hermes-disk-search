//! A6.5 — удаление проекта из анонимной зоны (`hds agent project rm`,
//! кнопка «Удалить…» во вкладке «Агент»).
//!
//! Что удаляется: каталог `<agent.root>\<P>` (`workspace`, `agent-config`,
//! `agent-state`), реестр `data\agent\registry\<P>.db` (+`-wal`/`-shm`), запись
//! `/projects/<P>` в `secrets.json` (токен, `provider`, `model`) и запись в
//! `projects.json`. **Папка проекта не трогается никогда**: она принадлежит
//! пользователю, а не зоне.
//!
//! Решение В-A6-3: при живом демоне зеркала (`sync.lock` не устарел) — отказ;
//! принудительно только `--force`. Живость демона считает вызывающий
//! (CLI/UI): `hds-index` зависит от `hds-agent`, а не наоборот, и обратный
//! вызов дал бы цикл.
//!
//! Метки в `anonymize.db` обычный `rm` **не** трогает — проект можно завести
//! снова; `--purge-meta` дополнительно вызывает `vault_drop_project` через
//! внедряемое замыкание (в CLI/UI это `hds_anonymize::Client::drop_project`).
//!
//! Порядок удаления: каталог зоны → реестр → секреты → запись реестра →
//! метки. Частичный отказ возвращается с уже собранным [`RemoveReport`],
//! чтобы CLI и вкладка сказали, что удалено, а что осталось.
//!
//! Секретов и значений из базы меток здесь нет: отчёт и ошибки содержат
//! только пути и `bool` (I8).

use std::path::{Path, PathBuf};

use hds_core::config::Config;
use hds_core::roots::is_agent_project;

use crate::{projects, secrets, settings};

/// Что разрешено/запрещено этому вызову (решение В-A6-3, A6.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoveOptions {
    /// Удалять, даже если демон зеркала жив (сначала снимается `sync.lock`).
    pub force: bool,
    /// Факт «демон зеркала жив» — считает вызывающий по `sync.lock`
    /// (`hds_index::watch::lock_is_stale`); ядро само PID не проверяет.
    pub daemon_alive: bool,
    /// Вызвать `purge` (удаление меток проекта из `anonymize.db`).
    pub purge_labels: bool,
}

/// Внедряемый вызов `vault_drop_project` (A6.5): ядро не поднимает Python-воркер
/// само — это делает CLI/UI, а unit-тесты передают замыкание.
pub type PurgeHook<'a> = dyn FnMut(&str) -> Result<(), String> + 'a;

/// Отчёт об удалении. Только пути и признаки — без значений (I8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoveReport {
    pub project: String,
    /// Удалённый каталог `<agent.root>\<P>` (или ссылка на него).
    pub zone: Option<PathBuf>,
    /// Удалённые файлы реестра: `.db`, `.db-wal`, `.db-shm`.
    pub registry: Vec<PathBuf>,
    /// Запись `/projects/<P>` была в `secrets.json` и удалена.
    pub token: bool,
    /// Запись проекта была в `projects.json` и удалена.
    pub record: bool,
    /// `Some` — попытка удаления меток была: `true` — удалены, `false` — нет.
    pub labels: Option<bool>,
    /// Что удалить не удалось (пути, без значений).
    pub left: Vec<PathBuf>,
}

/// Ошибка удаления проекта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveError {
    /// Проекта нет в `projects.json` (повторный `rm`; CLI → 2, UI → 404).
    NotRegistered(String),
    /// Живой `sync.lock` без `--force` (CLI/UI → 409).
    DaemonAlive(PathBuf),
    /// Файл занят другим процессом (`os error 32` на Windows).
    Busy {
        path: PathBuf,
        report: Box<RemoveReport>,
    },
    /// Ошибка ввода-вывода; `report` — что успело удалиться.
    Io {
        path: PathBuf,
        message: String,
        report: Box<RemoveReport>,
    },
    /// Метки удалить не удалось: всё остальное уже удалено (`report.labels = false`).
    Worker {
        report: Box<RemoveReport>,
        message: String,
    },
}

impl RemoveError {
    /// Что успело удалиться (у `NotRegistered`/`DaemonAlive` ничего не трогали).
    pub fn report(&self) -> Option<&RemoveReport> {
        match self {
            RemoveError::NotRegistered(_) | RemoveError::DaemonAlive(_) => None,
            RemoveError::Busy { report, .. }
            | RemoveError::Io { report, .. }
            | RemoveError::Worker { report, .. } => Some(report.as_ref()),
        }
    }
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoveError::NotRegistered(id) => {
                write!(f, "проект {id} не зарегистрирован — удалять нечего")
            }
            RemoveError::DaemonAlive(lock) => write!(
                f,
                "демон зеркала работает ({}) — остановите его: `hds stop` \
                 (или снимите задачу `installers\\install_agent_sync_task.ps1 -Remove`) \
                 и повторите; принудительно — `--force`",
                lock.display()
            ),
            RemoveError::Busy { path, .. } => write!(
                f,
                "файл занят другим процессом: {} — остановите демон зеркала и повторите",
                path.display()
            ),
            RemoveError::Io { path, message, .. } => write!(f, "{}: {message}", path.display()),
            RemoveError::Worker { message, .. } => {
                write!(f, "метки проекта не удалены: {message}")
            }
        }
    }
}

impl std::error::Error for RemoveError {}

/// Удалить проект: каталог зоны, реестр, секреты, запись реестра, метки.
///
/// `purge` — внедряемый вызов `vault_drop_project`: ядро не создаёт Python-воркер
/// само и остаётся тестируемым (в unit-тестах — замыкание-счётчик).
pub fn remove_project(
    cfg: &Config,
    id: &str,
    options: RemoveOptions,
    purge: Option<&mut PurgeHook<'_>>,
) -> Result<RemoveReport, RemoveError> {
    remove_project_checked(cfg, id, options, purge, None)
}

/// То же, но с повторной проверкой живости демона прямо перед удалением (AG6-08:
/// `daemon_alive` — снимок, снятый вызывающим, и окно между проверкой и
/// `remove_dir_all` демон мог бы использовать для старта).
pub fn remove_project_checked(
    cfg: &Config,
    id: &str,
    options: RemoveOptions,
    purge: Option<&mut PurgeHook<'_>>,
    alive_now: Option<&dyn Fn() -> bool>,
) -> Result<RemoveReport, RemoveError> {
    // D21: идентификатор — только `P` + 4 цифры. Всё остальное не могло быть
    // зарегистрировано, и путь по нему строить нельзя.
    if !is_agent_project(id) {
        return Err(RemoveError::NotRegistered(id.to_string()));
    }
    if projects::get(cfg, id).is_none() {
        return Err(RemoveError::NotRegistered(id.to_string()));
    }
    if options.daemon_alive && !options.force {
        let lock = settings::state_dir_for(cfg, id)
            .map(|d| d.join("sync.lock"))
            .unwrap_or_else(|| PathBuf::from("sync.lock"));
        return Err(RemoveError::DaemonAlive(lock));
    }
    // AG6-08: `daemon_alive` — снимок, снятый вызывающим; перед удалением
    // сверяемся ещё раз (окно между проверкой и `remove_dir_all`), иначе
    // демон, стартовавший в нём, остался бы без зоны.
    if !options.force {
        if let Some(alive) = alive_now {
            if alive() {
                let lock = settings::state_dir_for(cfg, id)
                    .map(|d| d.join("sync.lock"))
                    .unwrap_or_else(|| PathBuf::from("sync.lock"));
                return Err(RemoveError::DaemonAlive(lock));
            }
        }
    }

    let mut report = RemoveReport {
        project: id.to_string(),
        ..RemoveReport::default()
    };
    // 1. Каталог зоны — первым: если он занят, ещё ничего не снято.
    if let Some(root) = settings::zone_root(cfg) {
        remove_zone(&root, id, options.force, &mut report)?;
    }
    // 2. Реестр артефактов (SQLite WAL: .db, -wal, -shm).
    remove_registry(cfg, id, &mut report)?;
    // 3. Запись проекта в общем secrets.json (токен, provider, model).
    remove_secrets(cfg, id, &mut report)?;
    // 4. Запись в projects.json — последней из «файловых» шагов.
    remove_record(cfg, id, &mut report)?;
    // 5. Метки проекта — только по явному --purge-meta.
    if options.purge_labels {
        if let Some(purge) = purge {
            match purge(id) {
                Ok(()) => report.labels = Some(true),
                Err(message) => {
                    report.labels = Some(false);
                    return Err(RemoveError::Worker {
                        report: Box::new(report),
                        message,
                    });
                }
            }
        }
    }
    Ok(report)
}

/// Каталог `<root>\<P>`: рекурсивно, если это каталог; ссылка — только сама ссылка.
fn remove_zone(
    root: &Path,
    id: &str,
    force: bool,
    report: &mut RemoveReport,
) -> Result<(), RemoveError> {
    let zone = root.join(id);
    let meta = match std::fs::symlink_metadata(&zone) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_error(&zone, e, report)),
    };
    // Ссылка (симлинк/junction): удаляем ссылку, а не цель — иначе рекурсивное
    // удаление ушло бы за корень зоны.
    if meta.file_type().is_symlink() {
        match remove_link(&zone) {
            Ok(()) => {
                report.zone = Some(zone);
                return Ok(());
            }
            Err(e) => return Err(io_error(&zone, e, report)),
        }
    }
    let root_real = match std::fs::canonicalize(root) {
        Ok(real) => real,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_error(root, e, report)),
    };
    let zone_real = match std::fs::canonicalize(&zone) {
        Ok(real) => real,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_error(&zone, e, report)),
    };
    if !zone_is_direct_child(&root_real, &zone_real, id) {
        let message = format!(
            "каталог {} не является прямым подкаталогом {} — удаление отменено",
            zone.display(),
            root.display()
        );
        return Err(RemoveError::Io {
            path: zone,
            message,
            report: Box::new(report.clone()),
        });
    }
    if force {
        // Живой демон держит sync.lock; при --force снимаем его. Общий
        // `index.stop` не трогаем (AG-27).
        let lock = zone.join("agent-state").join("sync.lock");
        let _ = std::fs::remove_file(&lock);
    }
    let outcome = if meta.file_type().is_dir() {
        std::fs::remove_dir_all(&zone)
    } else {
        std::fs::remove_file(&zone)
    };
    match outcome {
        Ok(()) => {
            report.zone = Some(zone);
            Ok(())
        }
        Err(e) => {
            report.left.push(zone.clone());
            Err(io_error(&zone, e, report))
        }
    }
}

/// Реестр `<P>.db` и его WAL-файлы.
fn remove_registry(cfg: &Config, id: &str, report: &mut RemoveReport) -> Result<(), RemoveError> {
    let Some(db) = settings::registry_db(cfg, id) else {
        return Ok(());
    };
    let mut paths = vec![db.clone()];
    for suffix in ["-wal", "-shm"] {
        paths.push(PathBuf::from(format!("{}{suffix}", db.display())));
    }
    for path in paths {
        match std::fs::remove_file(&path) {
            Ok(()) => report.registry.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                report.left.push(path.clone());
                return Err(io_error(&path, e, report));
            }
        }
    }
    Ok(())
}

/// Запись `/projects/<P>` в `secrets.json` — точечно, без потери чужих блоков.
fn remove_secrets(cfg: &Config, id: &str, report: &mut RemoveReport) -> Result<(), RemoveError> {
    let path = settings::secrets_file(cfg);
    match secrets::remove_project_at(&path, id) {
        Ok(present) => {
            report.token = present;
            Ok(())
        }
        Err(e) => {
            report.left.push(path.clone());
            Err(RemoveError::Io {
                path,
                message: e.to_string(),
                report: Box::new(report.clone()),
            })
        }
    }
}

/// Запись проекта в `projects.json`.
fn remove_record(cfg: &Config, id: &str, report: &mut RemoveReport) -> Result<(), RemoveError> {
    match projects::remove(cfg, id) {
        Ok(true) => {
            report.record = true;
            Ok(())
        }
        // Между проверкой и удалением запись исчезла — это уже «не зарегистрирован».
        Ok(false) => Err(RemoveError::NotRegistered(id.to_string())),
        Err(message) => {
            let path = settings::projects_file(cfg);
            report.left.push(path.clone());
            Err(RemoveError::Io {
                path,
                message,
                report: Box::new(report.clone()),
            })
        }
    }
}

/// `os error 32` — Windows `ERROR_SHARING_VIOLATION`: файл занят (живой демон).
fn io_error(path: &Path, e: std::io::Error, report: &RemoveReport) -> RemoveError {
    if cfg!(windows) && e.raw_os_error() == Some(32) {
        RemoveError::Busy {
            path: path.to_path_buf(),
            report: Box::new(report.clone()),
        }
    } else {
        RemoveError::Io {
            path: path.to_path_buf(),
            message: e.to_string(),
            report: Box::new(report.clone()),
        }
    }
}

/// Разрешённый каталог проекта — ровно `<root_real>\<P>` (AG-11).
///
/// Сравнение без регистра: путь зоны мог быть записан другим регистром, а на
/// Windows это тот же каталог. `P` + 4 цифры сравниваются ASCII-регистронезависимо.
fn zone_is_direct_child(root_real: &Path, zone_real: &Path, id: &str) -> bool {
    let name_ok = zone_real
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(id));
    let parent_ok = zone_real
        .parent()
        .is_some_and(|parent| same_path(parent, root_real));
    name_ok && parent_ok && !same_path(zone_real, root_real)
}

/// Совпадение путей без учёта регистра и хвостовых разделителей.
fn same_path(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    }
    norm(a) == norm(b)
}

/// Удалить саму ссылку, не трогая цель.
#[cfg(windows)]
fn remove_link(path: &Path) -> std::io::Result<()> {
    // Симлинк/junction на каталог снимается `remove_dir`, на файл — `remove_file`.
    match std::fs::remove_dir(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => std::fs::remove_file(path),
        other => other,
    }
}

/// Удалить саму ссылку, не трогая цель.
#[cfg(not(windows))]
fn remove_link(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::Secrets;
    use serde_yaml::from_str;

    /// Тесты трогают `HDS_ROOT` — гонка с другими модулями недопустима.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const P: &str = "P0001";
    const OTHER: &str = "P0002";

    struct Stand {
        dir: PathBuf,
        cfg: Config,
    }

    impl Stand {
        fn new(tag: &str) -> Stand {
            let dir =
                std::env::temp_dir().join(format!("hds-agent-remove-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let zone = dir.join("zone");
            std::fs::create_dir_all(&zone).unwrap();
            let cfg = from_str(&format!(
                "agent:\n  enabled: true\n  root: '{}'\n",
                zone.to_string_lossy().replace('\\', "/")
            ))
            .unwrap();
            std::env::set_var("HDS_ROOT", &dir);
            std::env::remove_var("HDS_AGENT_SECRETS");
            Stand { dir, cfg }
        }

        fn zone(&self) -> PathBuf {
            self.dir.join("zone")
        }

        fn state(&self) -> PathBuf {
            self.dir.join("data").join("agent")
        }

        fn registry_db(&self) -> PathBuf {
            self.state().join("registry").join(format!("{P}.db"))
        }

        fn secrets_json(&self) -> PathBuf {
            self.state().join("secrets.json")
        }

        fn write(&self, path: &Path, text: &str) {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(path, text).unwrap();
        }

        /// Зарегистрированный проект с каталогом зоны, реестром, токеном и папкой проекта.
        fn prepare(&self) -> PathBuf {
            let project_dir = self.dir.join("Проект");
            self.write(&project_dir.join("документ.docx"), "оригинал");
            self.write(&self.zone().join(P).join("workspace/a.md"), "[PERSON_1]");
            self.write(&self.zone().join(P).join("agent-config/keys.json"), "{}");
            self.write(&self.zone().join(P).join("agent-state/sync.log"), "start\n");
            self.write(&self.registry_db(), "db");
            self.write(
                &PathBuf::from(format!("{}-wal", self.registry_db().display())),
                "wal",
            );
            self.write(
                &PathBuf::from(format!("{}-shm", self.registry_db().display())),
                "shm",
            );
            self.write(
                &self.state().join("projects.json"),
                &format!(
                    r#"{{"projects":[
                       {{"id":"{P}","name":"Пилот","path":"{}","provider":"openrouter","model":"m1","created":1}},
                       {{"id":"{OTHER}","name":"Второй","path":"{}","provider":"openrouter","model":"m2","created":2}}
                    ]}}"#,
                    project_dir.to_string_lossy().replace('\\', "\\\\"),
                    self.dir.join("Другой").to_string_lossy().replace('\\', "\\\\")
                ),
            );
            self.write(
                &self.secrets_json(),
                &format!(
                    r#"{{
                      "projects": {{
                        "{P}": {{"token":"tok-P0001-abcdef","token_sha256":"aaa","provider":"openrouter","model":"m1"}},
                        "{OTHER}": {{"token":"tok-P0002-abcdef","token_sha256":"bbb","provider":"openrouter","model":"m2"}}
                      }},
                      "providers": {{"openrouter":{{"api_key":"sk-or-abcdefgh"}}}},
                      "mcp": {{"tavily":{{"api_key":"tvly-abcdefgh"}}}}
                    }}"#
                ),
            );
            project_dir
        }
    }

    impl Drop for Stand {
        fn drop(&mut self) {
            std::env::remove_var("HDS_ROOT");
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn options() -> RemoveOptions {
        RemoveOptions::default()
    }

    #[test]
    fn removes_zone_registry_secrets_and_record_but_keeps_project() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let stand = Stand::new("full");
        let project_dir = stand.prepare();

        let report = remove_project(&stand.cfg, P, options(), None).expect("удалено");

        assert_eq!(report.project, P);
        assert_eq!(report.zone.as_deref(), Some(stand.zone().join(P).as_path()));
        assert_eq!(report.registry.len(), 3, "db + wal + shm: {report:?}");
        assert!(report.token, "запись в secrets.json была");
        assert!(report.record, "запись в projects.json была");
        assert_eq!(report.labels, None, "без --purge-meta метки не трогались");
        assert!(report.left.is_empty(), "{report:?}");

        // Следов проекта в зоне и реестре нет.
        assert!(!stand.zone().join(P).exists(), "каталог зоны удалён");
        assert!(!stand.registry_db().exists());
        assert!(!PathBuf::from(format!("{}-wal", stand.registry_db().display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", stand.registry_db().display())).exists());

        // Запись из projects.json ушла, соседняя осталась.
        let registry = projects::load_at(&stand.state().join("projects.json")).unwrap();
        assert!(registry.get(P).is_none());
        assert!(registry.get(OTHER).is_some(), "другой проект цел");

        // Из secrets.json ушёл только /projects/P0001 (AG-22).
        let secrets = Secrets::load(&stand.secrets_json()).unwrap();
        assert!(!secrets.has_project(P));
        assert!(secrets.has_project(OTHER));
        assert_eq!(
            secrets.mcp_key("tavily").as_deref(),
            Some("tvly-abcdefgh"),
            "mcp целы"
        );
        assert!(secrets.provider("openrouter").is_ok(), "providers целы");

        // Папка проекта неприкосновенна.
        assert!(project_dir.join("документ.docx").is_file());
        // В отчёте нет ни токенов, ни ключей (I8).
        let text = format!("{report:?}");
        assert!(!text.contains("tok-P0001"), "{text}");
        assert!(!text.contains("sk-or-"), "{text}");
    }

    #[test]
    fn second_remove_is_not_registered() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let stand = Stand::new("repeat");
        stand.prepare();
        remove_project(&stand.cfg, P, options(), None).unwrap();
        let err = remove_project(&stand.cfg, P, options(), None).expect_err("повтор — отказ");
        assert!(matches!(err, RemoveError::NotRegistered(ref id) if id == P));
        // Идентификатор не по D21 — тоже «не зарегистрирован», путей не строим.
        let err =
            remove_project(&stand.cfg, "../P0001", options(), None).expect_err("плохой id — отказ");
        assert!(matches!(err, RemoveError::NotRegistered(_)));
    }

    #[test]
    fn live_daemon_refuses_without_force() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let stand = Stand::new("daemon");
        stand.prepare();
        let lock = stand.zone().join(P).join("agent-state").join("sync.lock");
        std::fs::write(&lock, std::process::id().to_string()).unwrap();

        let err = remove_project(
            &stand.cfg,
            P,
            RemoveOptions {
                daemon_alive: true,
                ..options()
            },
            None,
        )
        .expect_err("живой демон — отказ");
        match err {
            RemoveError::DaemonAlive(path) => assert_eq!(path, lock),
            other => panic!("ожидался DaemonAlive, получено {other:?}"),
        }
        assert!(stand.zone().join(P).exists(), "ничего не удалено");
        assert!(stand.registry_db().exists());
        assert!(projects::get(&stand.cfg, P).is_some());

        // --force проходит: lock снимается, каталог удаляется.
        let report = remove_project(
            &stand.cfg,
            P,
            RemoveOptions {
                force: true,
                daemon_alive: true,
                purge_labels: false,
            },
            None,
        )
        .expect("--force удаляет");
        assert!(report.zone.is_some());
        assert!(!stand.zone().join(P).exists());
    }

    /// AG6-08: lock проверяется ещё раз перед удалением — демон, стартовавший
    /// после снимка `daemon_alive`, не остаётся без зоны.
    #[test]
    fn recheck_refuses_removal_when_daemon_started_late() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let stand = Stand::new("recheck");
        stand.prepare();
        let alive = || true;
        let err = remove_project_checked(&stand.cfg, P, options(), None, Some(&alive))
            .expect_err("живой демон найден повторной проверкой");
        assert!(matches!(err, RemoveError::DaemonAlive(_)), "{err:?}");
        assert!(stand.zone().join(P).exists(), "зона цела");
        assert!(stand.registry_db().exists(), "реестр цел");
        assert!(projects::get(&stand.cfg, P).is_some(), "запись цела");
    }

    #[test]
    fn purge_runs_only_with_flag_and_reports_failure() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let stand = Stand::new("purge");
        stand.prepare();
        let mut calls = 0usize;
        let mut counter = |_: &str| {
            calls += 1;
            Ok(())
        };
        let report = remove_project(&stand.cfg, P, options(), Some(&mut counter)).unwrap();
        assert_eq!(calls, 0, "без purge_labels замыкание не зовётся");
        assert_eq!(report.labels, None);

        // Второй проект — с флагом.
        stand.prepare();
        let mut seen: Vec<String> = Vec::new();
        let mut purge = |id: &str| {
            seen.push(id.to_string());
            Ok(())
        };
        let report = remove_project(
            &stand.cfg,
            P,
            RemoveOptions {
                purge_labels: true,
                ..options()
            },
            Some(&mut purge),
        )
        .unwrap();
        assert_eq!(seen, vec![P.to_string()]);
        assert_eq!(report.labels, Some(true));

        // Ошибка воркера: остальное уже удалено, в отчёте видно, что метки остались.
        stand.prepare();
        let mut fail = |_: &str| Err("воркер недоступен".to_string());
        let err = remove_project(
            &stand.cfg,
            P,
            RemoveOptions {
                purge_labels: true,
                ..options()
            },
            Some(&mut fail),
        )
        .expect_err("метки не удалились");
        match err {
            RemoveError::Worker { report, message } => {
                assert_eq!(report.labels, Some(false));
                assert!(
                    report.record && report.token,
                    "остальное удалено: {report:?}"
                );
                assert!(message.contains("воркер"));
            }
            other => panic!("ожидался Worker, получено {other:?}"),
        }
        assert!(!stand.zone().join(P).exists());
        assert!(projects::get(&stand.cfg, P).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn busy_file_stops_before_registry_and_record() {
        use std::os::windows::fs::OpenOptionsExt;

        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let stand = Stand::new("busy");
        stand.prepare();
        // Открыт без FILE_SHARE_DELETE: remove_dir_all получит ERROR_SHARING_VIOLATION.
        let held = stand.zone().join(P).join("workspace").join("held.txt");
        std::fs::write(&held, "занято").unwrap();
        let _guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&held)
            .unwrap();

        let err = remove_project(&stand.cfg, P, options(), None).expect_err("файл занят");
        match err {
            RemoveError::Busy { report, .. } => {
                assert!(report.zone.is_none());
                assert!(!report.left.is_empty(), "что не удалилось — в left");
                assert!(
                    !report.token && !report.record,
                    "до secrets/реестра не дошли"
                );
            }
            other => panic!("ожидался Busy, получено {other:?}"),
        }
        // Состояние вне зоны прежнее: реестр, токен и запись на месте.
        assert!(stand.registry_db().exists());
        assert!(Secrets::load(&stand.secrets_json()).unwrap().has_project(P));
        assert!(projects::get(&stand.cfg, P).is_some());
    }

    #[test]
    fn zone_guard_rejects_outside_paths() {
        let root = Path::new(r"C:\anon\zone");
        assert!(zone_is_direct_child(
            root,
            Path::new(r"C:\anon\zone\P0001"),
            P
        ));
        // Другой регистр — тот же каталог на Windows.
        assert!(zone_is_direct_child(
            root,
            Path::new(r"C:\anon\zone\p0001"),
            P
        ));
        // Не прямой потомок, не тот id, сам корень.
        assert!(!zone_is_direct_child(
            root,
            Path::new(r"C:\anon\zone\sub\P0001"),
            P
        ));
        assert!(!zone_is_direct_child(
            root,
            Path::new(r"C:\anon\zone\P0002"),
            P
        ));
        assert!(!zone_is_direct_child(root, root, P));
        assert!(!zone_is_direct_child(
            root,
            Path::new(r"C:\anon\other\P0001"),
            P
        ));
    }

    #[cfg(windows)]
    #[test]
    fn symlinked_project_dir_removes_link_only() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let stand = Stand::new("symlink");
        stand.prepare();
        // Цель ссылки — каталог вне зоны.
        let target = stand.dir.join("outside");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("важное.txt"), "не трогать").unwrap();
        let link = stand.zone().join(P);
        std::fs::remove_dir_all(&link).unwrap();
        // Создание симлинка требует прав (Developer Mode); без них проверяем
        // предохранитель пути напрямую (`zone_guard_rejects_outside_paths`).
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            return;
        }
        let report = remove_project(&stand.cfg, P, options(), None).expect("ссылка снята");
        assert!(report.zone.is_some());
        assert!(!link.exists(), "ссылка удалена");
        assert!(
            target.join("важное.txt").is_file(),
            "цель ссылки не тронута"
        );
    }
}
