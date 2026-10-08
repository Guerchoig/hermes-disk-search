//! Порт `hds/config.py`: пути проекта, загрузка `config.yaml`, `dig`,
//! `db_abs_path`, атомарная замена файла с ретраями.
//!
//! `PROJECT_ROOT` в Python — каталог репозитория (`dirname(dirname(__file__))`).
//! В Rust крейт лежит в `crates/hds-core`, поэтому корень — `CARGO_MANIFEST_DIR/../..`
//! (тот же приём, что в бинарях `hds-llama`, функция `repo_root`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_yaml::Value;

use crate::error::{CoreError, Result};

/// Имя приложения (маркер `/health` и MCP), как `APP_NAME` в Python.
pub const APP_NAME: &str = "disk-search";

/// Целевой контекст embedding-модели (bge-m3) в llama-роли: `EMB_CONTEXT = 8192`.
pub const EMB_CONTEXT: i64 = 8192;

/// Разобранный `config.yaml` (в Python — `dict`; здесь `serde_yaml::Value`).
pub type Config = Value;

/// Корень проекта (`config.yaml`, `sidecar/`, `models/`...).
///
/// Порядок (важно для **поставки**): `HDS_ROOT` → рядом с исполняемым файлом →
/// build-time `CARGO_MANIFEST_DIR/../..` (dev-фолбэк).
///
/// Раньше корень был **только** build-time (путь машины сборки): распакованная на
/// другой машине сборка брала несуществующий `config.yaml`. Теперь корень
/// определяется по расположению `hds.exe`/`llm_host.exe`:
/// `<root>\bin\x.exe` → `<root>`; `target\{debug,release}\x.exe` (в т.ч. `deps\`) →
/// корень репозитория; иначе — каталог самого exe.
pub fn project_root() -> PathBuf {
    if let Some(v) = std::env::var_os("HDS_ROOT") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(r) = root_from_exe(&exe) {
            return lexical_normalize(&r);
        }
    }
    lexical_normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

/// Корень проекта по пути исполняемого файла (чистая функция — есть тест).
fn root_from_exe(exe: &Path) -> Option<PathBuf> {
    let mut dir = exe.parent()?.to_path_buf();
    // `target\<profile>\deps\x.exe` → `target\<profile>`
    if dir.file_name().map(|n| n == "deps").unwrap_or(false) {
        dir = dir.parent()?.to_path_buf();
    }
    // `<root>\bin\x.exe` → `<root>`
    if dir.file_name().map(|n| n == "bin").unwrap_or(false) {
        return dir.parent().map(|p| p.to_path_buf());
    }
    // `target\{debug,release}\x.exe` → корень репозитория
    let profile = matches!(
        dir.file_name().and_then(|n| n.to_str()),
        Some("debug") | Some("release")
    );
    if profile
        && dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n == "target")
            .unwrap_or(false)
    {
        return dir.parent()?.parent().map(|p| p.to_path_buf());
    }
    Some(dir)
}

/// Лексическая нормализация пути (`a/b/../c` → `a/c`) без обращений к ФС.
fn lexical_normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// `HDS_CONFIG` (если задан) или `<проект>/config.yaml` — как `config.config_path()`.
///
/// M2 T2: если конфига в корне проекта нет, а в **системном каталоге состояния**
/// есть (server-поставка) — берём его. `HDS_CONFIG` по-прежнему сильнее всего.
pub fn config_path() -> PathBuf {
    match std::env::var_os("HDS_CONFIG") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let root_cfg = project_root().join("config.yaml");
            if root_cfg.is_file() {
                return root_cfg;
            }
            if let Some(sys) = system_state_dir() {
                let sys_cfg = sys.join("config.yaml");
                if sys_cfg.is_file() {
                    return sys_cfg;
                }
            }
            root_cfg
        }
    }
}

/// Загрузка конфига по умолчанию (`config_path()`).
pub fn load() -> Result<Config> {
    load_from(&config_path())
}

/// Загрузка `config.yaml` по конкретному пути (UTF-8/BOM — `utf-8-sig` в Python).
pub fn load_from(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path)?;
    let text = text.trim_start_matches('\u{feff}');
    let v: Value = serde_yaml::from_str(text)?;
    Ok(v)
}

/// `dig(cfg, "index.roots")`: значение по пути через точку (`None` — нет ключа).
///
/// Не-словари на пути (как `isinstance(cur, dict)` в Python) дают `None`.
pub fn dig<'a>(cfg: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut cur = cfg;
    for part in dotted.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

/// `db_abs_path(cfg)`: `db_path` (по умолчанию `index.db`) относительно
/// **каталога состояния** (`state_dir`; M2 T2): desktop — корень проекта
/// (прежнее поведение), server — системный каталог.
pub fn db_abs_path(cfg: &Config) -> PathBuf {
    let raw = dig(cfg, "db_path")
        .and_then(|v| v.as_str())
        .unwrap_or("index.db");
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        state_dir(cfg).join(p)
    }
}

/// Имя системного каталога состояния (`PLAN_M2_SERVER_PROFILE.md` §3.2).
pub const STATE_DIR_NAME: &str = "hermes-disk-search";

/// Системный каталог состояния для server-профиля:
/// Windows — `%ProgramData%\hermes-disk-search`, macOS —
/// `/Library/Application Support/hermes-disk-search`. На прочих платформах —
/// `None` (desktop-поведение).
pub fn system_state_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("ProgramData")
            .filter(|v| !v.is_empty())
            .map(|v| PathBuf::from(v).join(STATE_DIR_NAME))
    }
    #[cfg(target_os = "macos")]
    {
        Some(
            PathBuf::from("/Library")
                .join("Application Support")
                .join(STATE_DIR_NAME),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        None
    }
}

/// Каталог состояния (M2 T2): где живут `index.db`, `data/` (`access.yaml`,
/// PID, логи).
///
/// Порядок: `HDS_ROOT` переопределяет всё (изоляция тестов/перенос установки) →
/// `server` — системный каталог ([`system_state_dir`]; если та недоступна —
/// корень проекта) → `desktop` — корень проекта (прежнее поведение).
pub fn state_dir(cfg: &Config) -> PathBuf {
    if let Some(v) = std::env::var_os("HDS_ROOT") {
        if !v.is_empty() {
            return lexical_normalize(&PathBuf::from(v));
        }
    }
    if profile(cfg).is_server() {
        if let Some(d) = system_state_dir() {
            return d;
        }
    }
    project_root()
}

/// Инициализация каталога состояния при первом запуске (M2 T2): сам каталог и
/// `data/` (`access.yaml`, PID, логи). Создание идемпотентно.
pub fn ensure_state_dir(cfg: &Config) -> std::io::Result<PathBuf> {
    let d = state_dir(cfg);
    std::fs::create_dir_all(&d)?;
    std::fs::create_dir_all(d.join("data"))?;
    Ok(d)
}

/// Профиль установки (`profile` в config.yaml, план M2 §3.1).
///
/// `desktop` — одиночная машина (дефолт): состояние рядом с программой,
/// автозапуск — задачи Планировщика `AtLogOn`. `server` — серверный режим:
/// системный каталог состояния, службы без интерактивного входа, headless-UI.
/// Сам по себе профиль **не** включает авторизацию — гейт определяется
/// только `access.enabled` (`PLAN_SERVER_MULTIUSER.md`, решения 07.10.2026).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Desktop,
    Server,
}

impl Profile {
    /// Имя профиля (`desktop` | `server`) — для вывода в `hds check`/UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Desktop => "desktop",
            Profile::Server => "server",
        }
    }

    /// Серверный профиль?
    pub fn is_server(self) -> bool {
        matches!(self, Profile::Server)
    }
}

/// `profile(cfg)`: значение ключа `profile`; неизвестное/отсутствующее — `desktop`.
pub fn profile(cfg: &Config) -> Profile {
    match dig(cfg, "profile").and_then(|v| v.as_str()) {
        Some("server") => Profile::Server,
        _ => Profile::Desktop,
    }
}

/// Дефолт `db.busy_timeout_ms` (ожидание блокировки SQLite) по профилю (M4 §3.4).
///
/// `server` — 5000 мс (читатель не должен висеть на писателе дольше SLA-бюджета);
/// `desktop` — 30000 мс (прежнее зашитое значение `connect`, поведение M3).
pub fn db_busy_timeout_default(p: Profile) -> i64 {
    if p.is_server() {
        5000
    } else {
        30_000
    }
}

/// `db.busy_timeout_ms`: ожидание блокировки SQLite, мс (M4 T1).
pub fn db_busy_timeout_ms(cfg: &Config) -> i64 {
    dig(cfg, "db.busy_timeout_ms")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| db_busy_timeout_default(profile(cfg)))
        .max(0)
}

/// Дефолт `db.read_only_conns` по профилю (M4 §3.4).
///
/// `server` — true: читатели (MCP/UI/CLI-поиск) открывают БД read-only, писатель —
/// только конвейер индексации/watcher. `desktop` — false (прежнее поведение).
pub fn db_read_only_conns_default(p: Profile) -> bool {
    p.is_server()
}

/// `db.read_only_conns`: открывать читателям БД в read-only режиме.
pub fn db_read_only_conns(cfg: &Config) -> bool {
    dig(cfg, "db.read_only_conns")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| db_read_only_conns_default(profile(cfg)))
}

/// Дефолт `llm_server.parallel` (слоты чата) по профилю (M4 §3.1).
///
/// `server` — 5 (цель SLA: 5 одновременных `ask_my_files`); `desktop` — 1
/// (прежнее поведение). Правило то же, что в `hds-llama::config::build`:
/// явное значение в YAML сильнее дефолта.
pub fn llm_parallel_default(p: Profile) -> i64 {
    if p.is_server() {
        5
    } else {
        1
    }
}

/// `llm_server.parallel`: число слотов чата (для диагностик/UI).
pub fn llm_parallel(cfg: &Config) -> i64 {
    dig(cfg, "llm_server.parallel")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(|| llm_parallel_default(profile(cfg)))
        .max(1)
}

/// `db.journal_mode`: режим журнала, который писатель (`open_rw`) ставит PRAGMA'ой.
///
/// Дефолт `wal` (порт Python). WAL персистентен в файле базы — read-only читатели
/// наследуют его и сами PRAGMA не ставят. Бэкап — только `VACUUM INTO`/`.backup`
/// (копировать файлы WAL «на горячую» нельзя — пометка для Э10).
pub fn db_journal_mode(cfg: &Config) -> String {
    dig(cfg, "db.journal_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("wal")
        .to_string()
}

/// Атомарная замена файла с ретраями на `PermissionError` (как `config.replace_file`).
///
/// Windows: UI читает `config.yaml` каждые 2 с из соседнего потока, а `os.replace`
/// (CRT) не передаёт `FILE_SHARE_DELETE` — поэтому нужны ретраи.
pub fn replace_file(src: &Path, dst: &Path) -> Result<()> {
    replace_file_opts(src, dst, 20, Duration::from_millis(50))
}

/// `replace_file` с настраиваемыми попытками/паузой (для тестов).
pub fn replace_file_opts(src: &Path, dst: &Path, attempts: u32, delay: Duration) -> Result<()> {
    let attempts = attempts.max(1);
    for i in 0..attempts {
        match std::fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) if i + 1 < attempts && is_permission(&e) => {
                std::thread::sleep(delay);
            }
            Err(e) => return Err(CoreError::Io(e)),
        }
    }
    unreachable!("цикл возвращает результат на последней попытке")
}

fn is_permission(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(test)]
mod tests {
    use super::root_from_exe;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Тесты, трогающие env (`HDS_ROOT`/`HDS_CONFIG`/`ProgramData`), идут
    /// параллельно в потоках — гонка env недопустима.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Путь из частей с разделителем текущей ОС (тесты идут и на ubuntu-CI).
    fn p(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    #[test]
    fn root_from_shipped_bin_layout() {
        assert_eq!(
            root_from_exe(&p(&["inst", "bin", "hds.exe"])).unwrap(),
            p(&["inst"])
        );
    }

    #[test]
    fn root_from_cargo_run_layout() {
        assert_eq!(
            root_from_exe(&p(&["repo", "target", "debug", "hds.exe"])).unwrap(),
            p(&["repo"])
        );
        assert_eq!(
            root_from_exe(&p(&["repo", "target", "release", "llm_host.exe"])).unwrap(),
            p(&["repo"])
        );
    }

    #[test]
    fn root_from_cargo_test_layout() {
        assert_eq!(
            root_from_exe(&p(&["repo", "target", "debug", "deps", "some_test.exe"])).unwrap(),
            p(&["repo"])
        );
    }

    #[test]
    fn root_from_portable_flat_layout() {
        assert_eq!(
            root_from_exe(&p(&["portable", "hds.exe"])).unwrap(),
            p(&["portable"])
        );
    }

    /// M4 §3.1: `llm_server.parallel` — server-дефолт 5, desktop 1, явное сильнее.
    #[test]
    fn llm_parallel_defaults_by_profile() {
        use super::{llm_parallel, llm_parallel_default, Profile};
        assert_eq!(llm_parallel_default(Profile::Server), 5);
        assert_eq!(llm_parallel_default(Profile::Desktop), 1);
        let mk = |yaml: &str| -> super::Config { serde_yaml::from_str(yaml).unwrap() };
        assert_eq!(llm_parallel(&mk("profile: server")), 5);
        assert_eq!(llm_parallel(&mk("profile: desktop")), 1);
        assert_eq!(
            llm_parallel(&mk("profile: server\nllm_server:\n  parallel: 2")),
            2,
            "явное значение сильнее профильного дефолта"
        );
    }

    /// M2 T1: `profile` — desktop по умолчанию, server — по ключу; мусор → desktop.
    #[test]
    fn profile_defaults_and_parsing() {
        use super::{profile, Profile};
        use serde_yaml::Value;

        let cfg: Value = serde_yaml::from_str("profile: server").unwrap();
        assert_eq!(profile(&cfg), Profile::Server);

        let cfg: Value = serde_yaml::from_str("profile: desktop").unwrap();
        assert_eq!(profile(&cfg), Profile::Desktop);

        // Ключа нет — дефолт.
        let cfg: Value = serde_yaml::from_str("index:\n  roots: []").unwrap();
        assert_eq!(profile(&cfg), Profile::Desktop);

        // Неизвестное значение / не строка — desktop (не падаем).
        let cfg: Value = serde_yaml::from_str("profile: cluster").unwrap();
        assert_eq!(profile(&cfg), Profile::Desktop);
        let cfg: Value = serde_yaml::from_str("profile: [server]").unwrap();
        assert_eq!(profile(&cfg), Profile::Desktop);
    }

    /// M2 T2: `state_dir` — `HDS_ROOT` сильнее всего; desktop — корень проекта;
    /// server — системный каталог (если платформа его имеет).
    #[test]
    fn state_dir_resolution() {
        let _env = ENV_LOCK.lock().unwrap();
        use super::{state_dir, system_state_dir};
        use serde_yaml::Value;

        // HDS_ROOT переопределяет даже server.
        let cfg: Value = serde_yaml::from_str("profile: server").unwrap();
        std::env::set_var("HDS_ROOT", "Z:\\tmp-hds-state");
        assert_eq!(
            state_dir(&cfg),
            std::path::PathBuf::from("Z:\\tmp-hds-state")
        );
        std::env::remove_var("HDS_ROOT");

        // desktop — корень проекта.
        let cfg: Value = serde_yaml::from_str("profile: desktop").unwrap();
        assert_eq!(state_dir(&cfg), super::project_root());

        // server — системный каталог состояния (или корень проекта, если
        // платформа без системного каталога, как linux-CI).
        let cfg: Value = serde_yaml::from_str("profile: server").unwrap();
        let expected = system_state_dir().unwrap_or_else(super::project_root);
        assert_eq!(state_dir(&cfg), expected);
        if system_state_dir().is_some() {
            assert!(expected.ends_with(super::STATE_DIR_NAME));
        }
    }

    /// M2 T2: `config_path` — при отсутствии конфига в корне проекта берём
    /// конфиг из системного каталога состояния (server-поставка).
    #[cfg(windows)]
    #[test]
    fn config_path_falls_back_to_system_state_dir() {
        let _env = ENV_LOCK.lock().unwrap();
        use super::config_path;

        // Временный каталог-заглушка вместо ProgramData: внутри — системный
        // каталог состояния с конфигом (server-поставка).
        let fake_pd = std::env::temp_dir().join(format!("hds-sysdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fake_pd);
        let sys = fake_pd.join(super::STATE_DIR_NAME);
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::write(sys.join("config.yaml"), "profile: server\n").unwrap();

        // Изолированный корень проекта: пустой (чистим на старте — pid
        // на Windows переиспользуются, каталог мог остаться от прошлого прогона).
        let empty_root = std::env::temp_dir().join(format!("hds-emptyroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty_root);
        std::fs::create_dir_all(&empty_root).unwrap();

        let prev_pd = std::env::var_os("ProgramData");
        std::env::set_var("ProgramData", &fake_pd);
        std::env::remove_var("HDS_CONFIG");
        std::env::set_var("HDS_ROOT", &empty_root);

        // Конфига в корне проекта нет — берём конфиг из системного каталога.
        let p = config_path();
        assert_eq!(p, sys.join("config.yaml"), "p={}", p.display());

        // Конфиг в корне проекта существует — он сильнее системного.
        std::fs::write(empty_root.join("config.yaml"), "profile: desktop\n").unwrap();
        let p = config_path();
        assert_eq!(p, empty_root.join("config.yaml"), "p={}", p.display());

        // Восстановление окружения.
        match prev_pd {
            Some(v) => std::env::set_var("ProgramData", v),
            None => std::env::remove_var("ProgramData"),
        }
        std::env::remove_var("HDS_ROOT");
        let _ = std::fs::remove_dir_all(&fake_pd);
        let _ = std::fs::remove_dir_all(&empty_root);
    }
}
