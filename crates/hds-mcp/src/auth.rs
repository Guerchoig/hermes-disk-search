//! Авторизация MCP (M1 T4, `PLAN_M1_ACCESS_PROJECTS.md` §3.3): `POST /auth/login`,
//! проверка `Authorization: Bearer`, admin-гейтинг управляющих инструментов,
//! фильтр проектов из токена.
//!
//! Десктоп-профиль не ломаем: `access.enabled` (дефолт `false`) выключен →
//! stdio/HTTP работают как раньше (без токена, без фильтра).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use hds_core::access::{authenticate, load_at, new_token, resolve_token, save_at, AuthCtx};
use hds_core::config::{dig, Config};

/// Включён ли доступ (`access.enabled`; дефолт `false` — как в плане §3.3).
pub fn auth_enabled(cfg: &Config) -> bool {
    dig(cfg, "access.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Параметры авторизации HTTP-сервера (для тестов — явный `access_path`).
#[derive(Debug, Clone)]
pub struct AuthEnv {
    /// Авторизация включена (`access.enabled`).
    pub enabled: bool,
    /// Путь к `data/access.yaml`.
    pub access_path: PathBuf,
    /// TTL токенов, дней (`access.token_ttl_days`).
    pub ttl_days: u64,
    /// Аудит событий доступа (M3 T2); `None` — выключен (desktop по умолчанию).
    pub audit: Option<hds_core::audit::Audit>,
}

impl AuthEnv {
    /// Выключенная авторизация (desktop / stdio / тесты старого поведения).
    pub fn disabled() -> AuthEnv {
        AuthEnv {
            enabled: false,
            access_path: PathBuf::from("data/access.yaml"),
            ttl_days: hds_core::access::DEFAULT_TOKEN_TTL_DAYS,
            audit: None,
        }
    }

    /// Из конфига (`access.*`, `audit.*`).
    pub fn from_config(cfg: &Config) -> AuthEnv {
        AuthEnv {
            enabled: auth_enabled(cfg),
            access_path: hds_core::access::access_path(),
            ttl_days: hds_core::access::token_ttl_days(cfg),
            audit: hds_core::audit::Audit::from_config(cfg),
        }
    }
}

/// Текущее время в unix-секундах.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Вырезать токен из заголовка `Authorization: Bearer <token>`.
fn bearer_plain(header: Option<&str>) -> Option<&str> {
    let h = header?;
    let (scheme, rest) = h.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let t = rest.trim();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    } else {
        None
    }
}

/// Резолв Bearer-токена → контекст; нет/не прошёл → `None` (HTTP 401).
pub fn resolve_bearer(env: &AuthEnv, auth_header: Option<&str>) -> Option<AuthCtx> {
    let plain = bearer_plain(auth_header)?;
    let access = load_at(&env.access_path).ok()?;
    resolve_token(&access, plain, now_unix())
}

/// `POST /auth/login`: логин/пароль → авто-выданный токен (в файл — хэш).
///
/// `(plain_token, expires_unix, ctx)`; неверный логин/пароль → `Err` (HTTP 401).
pub fn login(env: &AuthEnv, login: &str, password: &str) -> Result<(String, i64, AuthCtx), String> {
    let access = load_at(&env.access_path).map_err(|e| e.message())?;
    let ctx = authenticate(&access, login, password)
        .ok_or_else(|| "неверный логин или пароль".to_string())?;
    let (plain, tok) = new_token(&ctx.user, env.ttl_days, now_unix()).map_err(|e| e.message())?;
    let mut access = access;
    access.tokens.push(tok.clone());
    save_at(&env.access_path, &access).map_err(|e| e.message())?;
    Ok((plain, tok.expires, ctx))
}

/// Фильтр проектов запроса (единая точка для инструментов):
/// * нет авторизации (desktop/stdio) — без фильтра;
/// * admin — без фильтра (видит всё);
/// * обычный пользователь — `Some(его проекты)`; **пустой список = не видит
///   ничего** (поиск трактует `Some(пусто)` как «ничего не выдать»).
pub fn project_filter(auth: Option<&AuthCtx>) -> Option<Vec<String>> {
    match auth {
        None => None,
        Some(a) if a.admin => None,
        Some(a) => Some(a.projects.clone()),
    }
}

/// Admin-гейтинг управляющих инструментов (indexing): в server-режиме (авторизованный
/// запрос) требуется `admin`; без авторизации (desktop/stdio) — разрешено как раньше.
pub fn admin_required(auth: Option<&AuthCtx>) -> Result<(), String> {
    match auth {
        None => Ok(()),
        Some(a) if a.admin => Ok(()),
        Some(_) => Err("требуются права администратора".to_string()),
    }
}
