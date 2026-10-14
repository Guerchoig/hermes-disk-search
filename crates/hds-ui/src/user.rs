//! Пользовательский (не админский) UI (R4, `PLAN_TRANSCRIBE_SERVICE.md`): кто я
//! и установка тонкого клиента. Доступен `POST /api/auth/login` → `hds login`.
//!
//! Никаких админских данных (конфиг, службы, доступ) здесь нет: страницу
//! `/ui/user/` и эти API пускает **user-гейт** (`ui_user_gate`) — любой валидный
//! токен. Логистика клиента переиспользует `hds_core::client` (те же маршруты,
//! что раздаёт MCP-сервер: `/mcp/client-install.ps1|sh`, `client-package.zip`).

use hds_core::access::AuthCtx;
use hds_core::client;
use hds_core::config::{dig, load, project_root, Config};
use serde_json::{json, Value};

/// Ключ `mcp_http.path` (endpoint MCP) — дефолт `/mcp` (как у `hds mcp-http`).
pub const DEFAULT_MCP_PATH: &str = "/mcp";
/// Порт MCP-сервера по умолчанию (`mcp_http.port`).
pub const DEFAULT_MCP_PORT: u16 = 8787;

/// `GET /api/user/me` — шапка страницы: кто вошёл и его проекты.
///
/// `None` — гейт выключен (десктоп: `access.enabled: false`) — доступ локальный.
pub fn me_json(auth: Option<&AuthCtx>) -> Value {
    match auth {
        Some(c) => json!({
            "auth": true, "user": c.user, "admin": c.admin, "projects": c.projects
        }),
        None => json!({
            "auth": false, "user": Value::Null, "admin": true, "projects": Value::Null,
            "hint": "локальный доступ: access.enabled: false (десктоп — вход свободный)"
        }),
    }
}

/// `mcp_http.path` из конфига (без завершающего `/`).
fn mcp_path(cfg: &Config) -> String {
    dig(cfg, "mcp_http.path")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MCP_PATH)
        .trim_end_matches('/')
        .to_string()
}

/// `GET /api/user/client` — «Установка клиента» (R4.4).
///
/// Отдаём **пути** (`/mcp/...`) — они работают от того же origin, что и страница
/// (за прокси наружу опубликованы `/mcp`, `/ui`, `/health`), и **адрес сервера**
/// из конфига — для прямого подключения. Формат ссылок — общий с MCP-сервером
/// (`hds_core::client`), чтобы мастеры и UI не расходились.
pub fn client_json() -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let path = mcp_path(&cfg);
    let host = dig(&cfg, "mcp_http.host")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = dig(&cfg, "mcp_http.port")
        .and_then(|v| v.as_u64())
        .and_then(|v| u16::try_from(v).ok())
        .unwrap_or(DEFAULT_MCP_PORT);
    let have: Vec<String> = client::packages_in(&project_root())
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .map(|s| s.to_string())
        .collect();
    json!({
        "version": client::product_version(),
        "platform": client::current_platform(),
        "mcp_path": path,
        "server": {
            "host": host,
            "port": port,
            // Адрес «как настроено» (на сервере слушает 127.0.0.1, наружу — прокси).
            "address": format!("http://{host}:{port}{path}"),
        },
        "routes": {
            "install_ps1": client::install_script_route(&path, "client-install.ps1"),
            "install_sh": client::install_script_route(&path, "client-install.sh"),
            // Пустой base → путь от того же origin (за прокси = внешний адрес сервера).
            "package_windows": client::package_url_platform("", &path, "windows-x64"),
            "package_macos": client::package_url_platform("", &path, "macos-arm64"),
            "health": "/health",
        },
        "packages_available": have,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Шапка страницы: вошедший не-админ видит свои проекты, локальный режим — нет.
    #[test]
    fn me_json_reflects_auth() {
        let a = AuthCtx {
            user: "anna".into(),
            admin: false,
            projects: vec!["Sales".into()],
        };
        let v = me_json(Some(&a));
        assert_eq!(v["auth"], true);
        assert_eq!(v["user"], "anna");
        assert_eq!(v["admin"], false);
        assert_eq!(v["projects"][0], "Sales");

        let local = me_json(None);
        assert_eq!(local["auth"], false);
        assert!(local["user"].is_null());
    }

    /// Пути клиента совпадают с маршрутами MCP-сервера (`hds_core::client`) —
    /// UI и мастеры не расходятся.
    #[test]
    fn client_routes_match_core_helpers() {
        let p = "/mcp";
        assert_eq!(
            client::install_script_route(p, "client-install.ps1"),
            "/mcp/client-install.ps1"
        );
        assert_eq!(
            client::package_url_platform("", p, "windows-x64"),
            "/mcp/client-package.zip?platform=windows-x64"
        );
        assert_eq!(client::package_route(p), "/mcp/client-package.zip");
    }
}
