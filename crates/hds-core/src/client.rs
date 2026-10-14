//! Тонкий клиент (M9): **версия продукта** и **клиентский пакет внутри поставки**.
//!
//! Зачем отдельный модуль: до 0.19 у тонкого клиента не было ни версии, ни канала
//! поставки — пакет собирался вручную из репозитория, а проверить «клиент тот же
//! версии, что сервер» было нечем (требование W7 `PLAN_SERVER_MULTIUSER.md`).
//! Теперь:
//! * упаковщики (`installers/build_rust_release*.{ps1,sh}`, `build_client_release*`)
//!   кладут рядом с приложением `version.txt` — это и есть версия продукта;
//! * серверный пакет содержит клиентский zip в `<корень>/client/`, а MCP-сервер
//!   отдаёт его по `<mcp-path>/client-package.zip` (см. `hds-mcp::http`);
//! * `/health` публикует `version`, поэтому мастера (`setup-client.ps1`,
//!   `install_client.command`) сравнивают версию клиента и сервера и предупреждают
//!   при расхождении.

use std::path::{Path, PathBuf};

use crate::config::project_root;

/// Файл версии продукта в корне поставки (пишут упаковщики релиза).
pub const VERSION_FILE: &str = "version.txt";

/// Каталог с клиентскими пакетами внутри серверной поставки.
pub const PACKAGE_DIR: &str = "client";

/// Префикс имени клиентского пакета (`hds-client-<ver>-<platform>.zip`).
pub const PACKAGE_PREFIX: &str = "hds-client-";

/// Версия продукта из `<root>/version.txt` (чистая функция — есть тест).
///
/// `None` — файла нет/пусто (dev-сборка из репозитория).
pub fn product_version_at(root: &Path) -> Option<String> {
    let v = std::fs::read_to_string(root.join(VERSION_FILE)).ok()?;
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Версия запущенного продукта: `version.txt` в корне поставки, иначе версия
/// крейта (dev-сборка: `target\{debug,release}` → корень репозитория, где файла нет).
pub fn product_version() -> String {
    product_version_at(&project_root()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

/// Платформенный «хвост» имени пакета: Windows x64.
pub const PLATFORM_WINDOWS: &str = "windows-x64";

/// Платформенный «хвост» имени пакета: macOS Apple Silicon.
pub const PLATFORM_MACOS: &str = "macos-arm64";

/// Платформа текущего процесса (дефолт, если клиент не назвал свою).
pub fn current_platform() -> String {
    match std::env::consts::OS {
        "windows" => PLATFORM_WINDOWS.to_string(),
        "macos" => PLATFORM_MACOS.to_string(),
        os => format!("{os}-{}", std::env::consts::ARCH),
    }
}

/// Нормализовать подсказку `?platform=` (`windows`/`win`/`windows-x64`/`mac`/`macos`/…).
pub fn normalize_platform(hint: &str) -> Option<String> {
    let h = hint.trim().to_ascii_lowercase().replace('_', "-");
    if h.is_empty() {
        return None;
    }
    if h.contains("windows") || h == "win" || h.starts_with("win-") {
        return Some(PLATFORM_WINDOWS.to_string());
    }
    if h.contains("mac") || h.contains("darwin") || h.contains("osx") || h.contains("os-x") {
        return Some(PLATFORM_MACOS.to_string());
    }
    Some(h)
}

/// Платформа по `User-Agent` (`curl`/`wget`/браузер её тоже присылают).
pub fn platform_from_user_agent(ua: &str) -> Option<String> {
    let u = ua.to_ascii_lowercase();
    if u.contains("windows") {
        return Some(PLATFORM_WINDOWS.to_string());
    }
    if u.contains("macintosh") || u.contains("mac os") || u.contains("darwin") {
        return Some(PLATFORM_MACOS.to_string());
    }
    None
}

/// Платформа из имени пакета (`hds-client-0.19.0-windows-x64.zip` → `windows-x64`).
///
/// Разбор идёт **с конца**: версия может содержать дефис (`0.20.0-rc1`), поэтому
/// «взять после первого дефиса» неверно (РЕГРЕССИЯ 13.10.2026 — ловилась живым
/// E2E: на запрос `?platform=macos-arm64` отдавался windows-пакет).
pub fn platform_of(package: &Path) -> Option<String> {
    let name = package.file_name()?.to_str()?.to_ascii_lowercase();
    let rest = name.strip_suffix(".zip")?.strip_prefix(PACKAGE_PREFIX)?;
    for known in [PLATFORM_WINDOWS, PLATFORM_MACOS] {
        if rest.ends_with(&format!("-{known}")) {
            return Some((*known).to_string());
        }
    }
    // Неизвестная платформа: последние два «слова» имени (`…-<os>-<arch>`).
    let parts: Vec<&str> = rest.split('-').collect();
    (parts.len() >= 2).then(|| format!("{}-{}", parts[parts.len() - 2], parts[parts.len() - 1]))
}

/// Все клиентские пакеты в `<root>/client` (отсортированы по имени; в имени версия,
/// поэтому последний — самый свежий).
pub fn packages_in(root: &Path) -> Vec<PathBuf> {
    let dir = root.join(PACKAGE_DIR);
    let mut cands: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .filter(|p| {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                    name.starts_with(PACKAGE_PREFIX) && name.to_ascii_lowercase().ends_with(".zip")
                })
                .collect()
        })
        .unwrap_or_default();
    cands.sort();
    cands
}

/// Пакет для платформы `want` (чистая функция — есть тест).
///
/// Берём самый «старший» подходящий (в имени версия). Если подходящего нет — самый
/// старший вообще: на сервере может лежать только один пакет (напр. dev-сборка или
/// поставка под одну платформу), и отдавать пустоту вместо него неверно.
pub fn pick_package(root: &Path, want: &str) -> Option<PathBuf> {
    let all = packages_in(root);
    let want = want.to_ascii_lowercase();
    all.iter()
        .rev()
        .find(|p| platform_of(p).as_deref() == Some(want.as_str()))
        .cloned()
        .or_else(|| all.last().cloned())
}

/// Клиентский пакет текущей поставки под платформу сервера
/// (`None` — dev-сборка/пакет не вложен).
pub fn package() -> Option<PathBuf> {
    pick_package(&project_root(), &current_platform())
}

/// Клиентский пакет под платформу сервера в конкретном каталоге (чистая функция).
pub fn package_in(root: &Path) -> Option<PathBuf> {
    pick_package(root, &current_platform())
}

/// Файл из каталога клиентских файлов поставки (`<root>/client/<name>`), если он
/// есть: скрипты-бутстрапы (`client-install.ps1|sh`) отдаются сервером публично.
pub fn package_file_at(root: &Path, name: &str) -> Option<PathBuf> {
    // имя — без путей: защита от `../`
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    let p = root.join(PACKAGE_DIR).join(name);
    p.is_file().then_some(p)
}

/// То же для текущей поставки.
pub fn package_file(name: &str) -> Option<PathBuf> {
    package_file_at(&project_root(), name)
}

/// Адрес скачивания клиентского пакета с сервера — для подсказок в мастерах и
/// документации: `<base><mcp-path>/client-package.zip`.
pub fn package_url(base: &str, mcp_path: &str) -> String {
    let base = base.trim_end_matches('/');
    let mp = mcp_path.trim_end_matches('/');
    format!("{base}{mp}/client-package.zip")
}

/// Адрес скачивания под конкретную платформу (`?platform=` — сервер выберет пакет:
/// в поставке лежат оба, `windows-x64` и `macos-arm64`).
pub fn package_url_platform(base: &str, mcp_path: &str, platform: &str) -> String {
    format!("{}?platform={platform}", package_url(base, mcp_path))
}

/// Путь скачивания клиентского пакета в маршрутах MCP-сервера.
pub fn package_route(mcp_path: &str) -> String {
    format!("{}/client-package.zip", mcp_path.trim_end_matches('/'))
}

/// Путь скачивания скрипта-бутстрапа (публичный, без токена): `client-install.ps1|sh`.
pub fn install_script_route(mcp_path: &str, file: &str) -> String {
    format!("{}/{file}", mcp_path.trim_end_matches('/'))
}

/// Имена файлов скриптов-бутстрапов в `<root>/client/`.
pub const INSTALL_SCRIPTS: [&str; 2] = ["client-install.ps1", "client-install.sh"];

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("hds-client-{tag}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Версия читается из `version.txt` (с переводом строки/пробелами — тримится),
    /// пустой файл и отсутствие файла дают `None`.
    #[test]
    fn version_file_is_trimmed_and_optional() {
        let root = tmp_root("ver");
        assert_eq!(product_version_at(&root), None, "файла нет — версии нет");

        std::fs::write(root.join(VERSION_FILE), b"").unwrap();
        assert_eq!(product_version_at(&root), None, "пустой файл — версии нет");

        std::fs::write(root.join(VERSION_FILE), b"0.19.0\r\n").unwrap();
        assert_eq!(product_version_at(&root).as_deref(), Some("0.19.0"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// В поставке пакет ищется в `<root>/client`, берётся самый старший по имени;
    /// посторонние файлы и другие каталоги не мешают.
    #[test]
    fn package_is_found_in_client_dir() {
        let root = tmp_root("pkg");
        assert_eq!(package_in(&root), None, "каталога нет — пакета нет");

        let dir = root.join(PACKAGE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("README.txt"), b"x").unwrap();
        // рядом лежат скрипты-бутстрапы — они не пакеты и не мешают
        std::fs::write(dir.join("client-install.ps1"), b"# script").unwrap();
        std::fs::write(dir.join("hds-client-0.18.0-windows-x64.zip"), b"old").unwrap();
        std::fs::write(dir.join("hds-client-0.19.0-windows-x64.zip"), b"new").unwrap();
        assert_eq!(
            pick_package(&root, PLATFORM_WINDOWS),
            Some(dir.join("hds-client-0.19.0-windows-x64.zip")),
            "берём самый старший по имени"
        );

        // Пакет не в client/ (например подсунутый рядом) не подхватывается.
        std::fs::write(root.join("hds-client-0.20.0-windows-x64.zip"), b"x").unwrap();
        assert_eq!(
            pick_package(&root, PLATFORM_WINDOWS),
            Some(dir.join("hds-client-0.19.0-windows-x64.zip"))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Пакет выбирается по платформе; если подходящего нет — отдаётся единственный
    /// имеющийся (поставка под одну платформу / dev-сборка), а не пустота.
    #[test]
    fn pick_package_prefers_platform_with_fallback() {
        let root = tmp_root("pick");
        let dir = root.join(PACKAGE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hds-client-0.19.0-windows-x64.zip"), b"win").unwrap();
        std::fs::write(dir.join("hds-client-0.19.0-macos-arm64.zip"), b"mac").unwrap();

        assert_eq!(
            pick_package(&root, PLATFORM_MACOS),
            Some(dir.join("hds-client-0.19.0-macos-arm64.zip"))
        );
        // `pick_package` сравнивает полный «хвост» платформы: подсказку вида
        // `macos` нормализует вызывающий (`normalize_platform`), иначе — фолбэк.
        assert_eq!(
            pick_package(&root, "macos"),
            Some(dir.join("hds-client-0.19.0-windows-x64.zip")),
            "неизвестная платформа → фолбэк на самый старший пакет"
        );
        assert_eq!(
            pick_package(&root, &normalize_platform("mac").unwrap()),
            Some(dir.join("hds-client-0.19.0-macos-arm64.zip")),
            "после нормализации — нужный пакет"
        );
        assert_eq!(
            pick_package(&root, "linux-x64"),
            Some(dir.join("hds-client-0.19.0-windows-x64.zip")),
            "нет linux-пакета — отдаём самый старший из имеющихся"
        );
        assert_eq!(
            platform_of(&dir.join("hds-client-0.19.0-macos-arm64.zip")).as_deref(),
            Some("macos-arm64")
        );

        // Версия с дефисом (пререлиз) не ломает разбор платформы: раньше имя
        // разбиралось от первого дефиса, и `?platform=macos-arm64` отдавал
        // windows-пакет (РЕГРЕССИЯ 13.10.2026, живой E2E).
        let rc = dir.join("hds-client-0.20.0-rc1-macos-arm64.zip");
        std::fs::write(&rc, b"rc").unwrap();
        assert_eq!(platform_of(&rc).as_deref(), Some("macos-arm64"));
        assert_eq!(
            pick_package(&root, PLATFORM_MACOS),
            Some(rc),
            "самый свежий mac-пакет (в т.ч. с дефисом в версии)"
        );
        assert_eq!(
            pick_package(&root, PLATFORM_WINDOWS),
            Some(dir.join("hds-client-0.19.0-windows-x64.zip"))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Подсказки платформы и `User-Agent` нормализуются к нашим двум платформам.
    #[test]
    fn platform_hints_and_user_agent() {
        for hint in ["windows", "Windows-x64", "win", "WIN_64"] {
            assert_eq!(
                normalize_platform(hint).as_deref(),
                Some(PLATFORM_WINDOWS),
                "подсказка {hint}"
            );
        }
        for hint in ["macos", "mac", "darwin", "MacOS-arm64", "Mac OS X"] {
            assert_eq!(
                normalize_platform(hint).as_deref(),
                Some(PLATFORM_MACOS),
                "подсказка {hint}"
            );
        }
        assert_eq!(normalize_platform("  "), None, "пустая подсказка");
        assert_eq!(
            normalize_platform("linux-x64").as_deref(),
            Some("linux-x64")
        );

        // curl -A 'Mozilla/5.0 (Macintosh; ...)' / Windows-браузер
        assert_eq!(
            platform_from_user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)").as_deref(),
            Some(PLATFORM_MACOS)
        );
        assert_eq!(
            platform_from_user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)").as_deref(),
            Some(PLATFORM_WINDOWS)
        );
        assert_eq!(platform_from_user_agent("curl/8.5.0"), None);
    }

    /// Подсказка-адрес и маршрут совпадают по смыслу (одна и та же точка).
    #[test]
    fn package_url_and_route_agree() {
        assert_eq!(
            package_url("https://host/", "/mcp"),
            "https://host/mcp/client-package.zip"
        );
        assert_eq!(
            package_url_platform("https://host/", "/mcp", PLATFORM_MACOS),
            "https://host/mcp/client-package.zip?platform=macos-arm64"
        );
        assert_eq!(package_route("/mcp"), "/mcp/client-package.zip");
        assert_eq!(package_route("/mcp/"), "/mcp/client-package.zip");
        assert_eq!(
            install_script_route("/mcp", "client-install.ps1"),
            "/mcp/client-install.ps1"
        );
    }
}
