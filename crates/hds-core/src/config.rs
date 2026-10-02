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
pub fn config_path() -> PathBuf {
    match std::env::var_os("HDS_CONFIG") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => project_root().join("config.yaml"),
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

/// `db_abs_path(cfg)`: `db_path` (по умолчанию `index.db`) относительно корня проекта.
pub fn db_abs_path(cfg: &Config) -> PathBuf {
    let raw = dig(cfg, "db_path")
        .and_then(|v| v.as_str())
        .unwrap_or("index.db");
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        project_root().join(p)
    }
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
}
