//! Матрица опасных путей (TR-3.4, TR-3.10 а).
//!
//! Тот же набор правил реализован в воркере (`sidecar/hds_agent/src/guard.ts`),
//! потому что хук `hds-guard` живёт в Node. Здесь он нужен Rust-стороне:
//! `host_call office_*` тоже принимает путь, и он обязан проверяться до
//! передачи в Python.
//!
//! Правила:
//! * пустой путь — отказ;
//! * UNC (`\\server\share`), `//server/share` — отказ;
//! * префиксы пространства устройств `\\?\`, `\\.\` — отказ;
//! * альтернативные потоки (`file.txt:hidden`) — отказ;
//! * имена устройств (`CON`, `NUL`, `COM1`, `LPT1`, `PRN`, `AUX`) — отказ;
//! * относительный путь абсолютизируется от корня workspace, `..` разрешается;
//! * junction/symlink разрешается через `realpath` (канонизация);
//! * результат обязан лежать внутри `<P>\workspace`.

use std::path::{Component, Path, PathBuf};

/// Причина отказа проверки пути.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    /// Пустой путь.
    Empty,
    /// UNC-путь (`\\server\share`).
    Unc,
    /// Префикс пространства устройств (`\\?\`, `\\.\`).
    DeviceNamespace,
    /// Альтернативный поток данных (`file.txt:hidden`).
    AlternateStream,
    /// Имя устройства (`CON`, `NUL`, `COM1`, …).
    DeviceName,
    /// Путь выходит за пределы workspace.
    Escape,
}

impl GuardError {
    /// Текст для модели (отказ виден в `reason` хука).
    pub fn reason(&self) -> String {
        match self {
            GuardError::Empty => "пустой путь".to_string(),
            GuardError::Unc => "UNC-путь запрещён".to_string(),
            GuardError::DeviceNamespace => "префикс пространства устройств запрещён".to_string(),
            GuardError::AlternateStream => "альтернативный поток данных запрещён".to_string(),
            GuardError::DeviceName => "имя устройства запрещено".to_string(),
            GuardError::Escape => "путь вне workspace проекта".to_string(),
        }
    }
}

/// Имена устройств DOS (без расширения и с любым расширением).
const DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Проверить путь по матрице опасных путей и вернуть канонический путь.
pub fn check_within(root: &Path, candidate: &str) -> Result<PathBuf, GuardError> {
    let raw = candidate.trim();
    if raw.is_empty() {
        return Err(GuardError::Empty);
    }
    let lower = raw.to_ascii_lowercase();
    if raw.starts_with("\\\\") || raw.starts_with("//") {
        if lower.starts_with("\\\\?\\") || lower.starts_with("\\\\.\\") {
            return Err(GuardError::DeviceNamespace);
        }
        return Err(GuardError::Unc);
    }
    if raw.contains('\0') {
        return Err(GuardError::Empty);
    }
    // Альтернативный поток: двоеточие не на позиции буквы диска.
    for (i, b) in raw.as_bytes().iter().enumerate() {
        if *b == b':' && i > 1 {
            return Err(GuardError::AlternateStream);
        }
    }
    let base = file_stem(raw);
    if DEVICE_NAMES.iter().any(|d| d.eq_ignore_ascii_case(&base)) {
        return Err(GuardError::DeviceName);
    }

    let root_real = canonical_or_lexical(root);
    let joined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let absolute = normalize(&joined);
    let real = canonical_or_lexical(&absolute);
    if !is_within(&real, &root_real) {
        return Err(GuardError::Escape);
    }
    Ok(real)
}

/// Имя без расширения для проверки имён устройств.
fn file_stem(raw: &str) -> String {
    raw.rsplit(['\\', '/'])
        .next()
        .unwrap_or(raw)
        .split(['.', ':'])
        .next()
        .unwrap_or(raw)
        .trim_end_matches(['.', ' '])
        .to_string()
}

/// Лексическая нормализация: `.` и `..` без обращений к ФС.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                // Выше корня не поднимаемся: лишние `..` просто отбрасываются.
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Канонизация существующего пути; для несуществующего — канонизация
/// ближайшего существующего предка + остаток.
pub fn canonical_or_lexical(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let mut ancestor = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while let Some(name) = ancestor.file_name() {
        tail.push(name.to_os_string());
        if !ancestor.pop() {
            break;
        }
        if let Ok(real) = std::fs::canonicalize(&ancestor) {
            let mut out = real;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return normalize(&out);
        }
    }
    normalize(path)
}

/// Лежит ли `child` внутри `parent` (по компонентам, без учёта регистра).
pub fn is_within(child: &Path, parent: &Path) -> bool {
    let c: Vec<String> = child
        .components()
        .map(|x| x.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let p: Vec<String> = parent
        .components()
        .map(|x| x.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    !p.is_empty() && c.len() >= p.len() && c[..p.len()] == p[..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hds-agent-paths-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("allowed.txt"), "ok").unwrap();
        dir
    }

    #[test]
    fn relative_and_absolute_inside_are_allowed() {
        let ws = workspace("inside");
        assert!(check_within(&ws, "allowed.txt").is_ok());
        assert!(check_within(&ws, "sub\\file.txt").is_ok());
        let abs = ws.join("allowed.txt");
        assert!(check_within(&ws, abs.to_str().unwrap()).is_ok());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let ws = workspace("parent");
        assert_eq!(
            check_within(&ws, "..\\..\\secret.txt"),
            Err(GuardError::Escape)
        );
        assert_eq!(check_within(&ws, "sub\\..\\..\\x"), Err(GuardError::Escape));
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn absolute_outside_is_rejected() {
        let ws = workspace("outside");
        let outside = std::env::temp_dir().join("hds-agent-outside.txt");
        assert_eq!(
            check_within(&ws, outside.to_str().unwrap()),
            Err(GuardError::Escape)
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn dangerous_forms_are_rejected() {
        let ws = workspace("danger");
        assert_eq!(check_within(&ws, ""), Err(GuardError::Empty));
        assert_eq!(
            check_within(&ws, "\\\\server\\share\\file.txt"),
            Err(GuardError::Unc)
        );
        assert_eq!(
            check_within(&ws, "\\\\?\\C:\\Windows\\win.ini"),
            Err(GuardError::DeviceNamespace)
        );
        assert_eq!(
            check_within(&ws, "\\\\.\\PhysicalDrive0"),
            Err(GuardError::DeviceNamespace)
        );
        assert_eq!(
            check_within(&ws, "allowed.txt:hidden"),
            Err(GuardError::AlternateStream)
        );
        assert_eq!(check_within(&ws, "NUL"), Err(GuardError::DeviceName));
        assert_eq!(
            check_within(&ws, "sub\\con.txt"),
            Err(GuardError::DeviceName)
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[cfg(windows)]
    #[test]
    fn junction_escape_is_rejected() {
        let ws = workspace("junction");
        let outside = ws
            .parent()
            .unwrap()
            .join(format!("hds-agent-junction-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        let link = ws.join("link-out");
        let created = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if created {
            assert_eq!(
                check_within(&ws, "link-out\\secret.txt"),
                Err(GuardError::Escape)
            );
        }
        let _ = std::fs::remove_dir_all(&link);
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&ws);
    }
}
