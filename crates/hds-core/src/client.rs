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

/// Клиентский пакет в каталоге `<root>/client` (чистая функция — есть тест).
///
/// Берём самый «старший» по имени (в имени версия): в поставке пакет один, но при
/// обновлении рядом может остаться прежний.
pub fn package_in(root: &Path) -> Option<PathBuf> {
    let dir = root.join(PACKAGE_DIR);
    let mut cands: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name.starts_with(PACKAGE_PREFIX) && name.to_ascii_lowercase().ends_with(".zip")
        })
        .collect();
    cands.sort();
    cands.pop()
}

/// Клиентский пакет текущей поставки (`None` — dev-сборка/пакет не вложен).
pub fn package() -> Option<PathBuf> {
    package_in(&project_root())
}

/// Адрес скачивания клиентского пакета с сервера — для подсказок в мастерах и
/// документации: `<base><mcp-path>/client-package.zip`.
pub fn package_url(base: &str, mcp_path: &str) -> String {
    let base = base.trim_end_matches('/');
    let mp = mcp_path.trim_end_matches('/');
    format!("{base}{mp}/client-package.zip")
}

/// Путь скачивания клиентского пакета в маршрутах MCP-сервера.
pub fn package_route(mcp_path: &str) -> String {
    format!("{}/client-package.zip", mcp_path.trim_end_matches('/'))
}

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
        std::fs::write(dir.join("hds-client-0.18.0-windows-x64.zip"), b"old").unwrap();
        std::fs::write(dir.join("hds-client-0.19.0-windows-x64.zip"), b"new").unwrap();
        assert_eq!(
            package_in(&root),
            Some(dir.join("hds-client-0.19.0-windows-x64.zip")),
            "берём самый старший по имени"
        );

        // Пакет не в client/ (например подсунутый рядом) не подхватывается.
        std::fs::write(root.join("hds-client-0.20.0-windows-x64.zip"), b"x").unwrap();
        assert_eq!(
            package_in(&root),
            Some(dir.join("hds-client-0.19.0-windows-x64.zip"))
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Подсказка-адрес и маршрут совпадают по смыслу (одна и та же точка).
    #[test]
    fn package_url_and_route_agree() {
        assert_eq!(
            package_url("https://host/", "/mcp"),
            "https://host/mcp/client-package.zip"
        );
        assert_eq!(package_route("/mcp"), "/mcp/client-package.zip");
        assert_eq!(package_route("/mcp/"), "/mcp/client-package.zip");
    }
}
