//! Идентичность и доступ (M1, `PLAN_M1_ACCESS_PROJECTS.md` §3.2/§3.3):
//! хранилище `data/access.yaml` (пользователи + авто-выданные токены),
//! пароли — **Argon2id** (PHC-строка), токены — **opaque 256 бит**, хранятся
//! хэшем **sha256**, сверки — постоянным временем (`subtle`).
//!
//! Контракт YAML (§3.2):
//! ```yaml
//! users:
//!   - { name: admin, login: admin, password_hash: "$argon2id$...", admin: true }
//!   - { name: alice, login: alice, password_hash: "$argon2id$...", projects: [Sales] }
//! tokens:                                  # хранятся ХЭШЕМ
//!   - { token_sha256: "<hex>", user: alice, expires: 1798761600 }
//! ```
//!
//! Поведение:
//! * файл может отсутствовать — это пустое хранилище (первый запуск);
//! * запись — атомарная, через [`crate::config::replace_file`];
//! * TTL токена — `access.token_ttl_days` (дефолт [`DEFAULT_TOKEN_TTL_DAYS`]);
//! * `expires` — unix-секунды; истёкший/отозванный/`disabled` → `None`;
//! * пустой `projects` у обычного пользователя = «нет проектов» (в server-режиме
//!   выдача пуста); политика «admin видит всё» — на уровне вызывающих (MCP/T4).

use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::config::{dig, replace_file, Config};
use crate::error::{CoreError, Result};

/// TTL токена по умолчанию, дней (`access.token_ttl_days`).
pub const DEFAULT_TOKEN_TTL_DAYS: u64 = 90;

/// Длина opaque-токена в байтах (256 бит) — hex-строка из 64 символов.
const TOKEN_BYTES: usize = 32;

/// Хранилище доступа (`data/access.yaml`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Access {
    /// Пользователи (логин/пароль выдаёт админ).
    #[serde(default)]
    pub users: Vec<User>,
    /// Авто-выданные токены (хранятся хэшем sha256).
    #[serde(default)]
    pub tokens: Vec<Token>,
}

/// Пользователь (`users[i]` контракта).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct User {
    /// Отображаемое имя (может совпадать с `login`).
    pub name: String,
    /// Логин (уникален в файле; идентичность в `AuthCtx`).
    pub login: String,
    /// PHC-строка Argon2id (`$argon2id$...`).
    pub password_hash: String,
    /// Признак администратора (UI и управляющие инструменты).
    #[serde(default, skip_serializing_if = "is_false")]
    pub admin: bool,
    /// Разрешённые проекты; пусто у обычного пользователя = ничего не видно.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    /// Учётка отключена (вход запрещён, токены не резолвятся).
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
}

/// Авто-выданный токен (хранится хэшем sha256, plain-значение показывается один раз).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    /// hex-строка sha256(plain-токен).
    pub token_sha256: String,
    /// Логин владельца.
    pub user: String,
    /// Истечение, unix-секунды.
    pub expires: i64,
}

/// Контекст авторизации запроса (после проверки токена).
#[derive(Debug, Clone, PartialEq)]
pub struct AuthCtx {
    /// Логин владельца токена.
    pub user: String,
    /// Признак администратора (наследуется от пользователя).
    pub admin: bool,
    /// Разрешённые проекты пользователя.
    pub projects: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Argon2id с параметрами по умолчанию крейта (m=19456 КиБ, t=2, p=1, v=0x13).
fn argon2id() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default())
}

/// Путь `data/access.yaml` в **каталоге состояния** (M2 T2): desktop —
/// `<корень проекта>/data`, server — `<ProgramData|Application Support>/data`.
/// (Для тестов — `load_at`/`save_at` с явным путём.)
pub fn access_path() -> PathBuf {
    let cfg = crate::config::load().unwrap_or(serde_yaml::Value::Null);
    crate::config::state_dir(&cfg)
        .join("data")
        .join("access.yaml")
}

/// Загрузка хранилища по умолчанию (`data/access.yaml`).
pub fn load() -> Result<Access> {
    load_at(&access_path())
}

/// Загрузка по явному пути; отсутствие файла — пустое хранилище (первый запуск).
pub fn load_at(path: &Path) -> Result<Access> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let text = text.trim_start_matches('\u{feff}');
            Ok(serde_yaml::from_str(text)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Access::default()),
        Err(e) => Err(CoreError::Io(e)),
    }
}

/// Сохранение хранилища по умолчанию (атомарно).
pub fn save(access: &Access) -> Result<()> {
    save_at(&access_path(), access)
}

/// Атомарное сохранение по явному пути: временный файл в том же каталоге +
/// [`replace_file`] (ретраи на Windows-блокировках).
pub fn save_at(path: &Path, access: &Access) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_yaml::to_string(access)?;
    let tmp = sibling_tmp(path);
    std::fs::write(&tmp, text)?;
    replace_file(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Временный файл рядом с целевым (тот же диск — rename атомарен).
fn sibling_tmp(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "access.yaml".into());
    path.with_file_name(format!(".{name}.tmp"))
}

/// Хэш пароля — Argon2id (PHC-строка со случайной солью).
pub fn hash_password(password: &str) -> Result<String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    argon2id()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| CoreError::Other(format!("argon2: {e}")))
}

/// Проверка пароля по PHC-строке (Argon2id сверяет постоянным временем внутри).
/// Некорректный/чужой формат хэша → `false` (не паникуем).
pub fn verify_password(hash: &str, password: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    match PasswordHash::new(hash) {
        Ok(parsed) => argon2id()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// TTL токена из конфига: `access.token_ttl_days` (дефолт 90).
pub fn token_ttl_days(cfg: &Config) -> u64 {
    dig(cfg, "access.token_ttl_days")
        .and_then(|v| v.as_u64())
        .filter(|d| *d > 0)
        .unwrap_or(DEFAULT_TOKEN_TTL_DAYS)
}

/// hex(sha256(plain)) — как хранится `token_sha256`.
pub fn token_sha256_hex(plain: &str) -> String {
    hex::encode(Sha256::digest(plain.as_bytes()))
}

/// Выдать токен: 32 случайных байта (`getrandom`), plain — hex-строка.
///
/// `(plain, Token)`: plain показывается пользователю один раз, в файл идёт
/// только хэш. `now_unix` передаётся явно — тестируемость.
pub fn new_token(user: &str, ttl_days: u64, now_unix: i64) -> Result<(String, Token)> {
    let mut buf = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut buf).map_err(|e| CoreError::Other(format!("getrandom: {e}")))?;
    let plain = hex::encode(buf);
    let token = Token {
        token_sha256: token_sha256_hex(&plain),
        user: user.to_string(),
        expires: now_unix + (ttl_days as i64) * 86_400,
    };
    Ok((plain, token))
}

/// Сверка digest'а с хранящимся hex постоянным временем (длина не секрет).
fn digest_eq_stored(plain_digest: &[u8], stored_hex: &str) -> bool {
    match hex::decode(stored_hex) {
        Ok(stored) if stored.len() == plain_digest.len() => {
            stored.as_slice().ct_eq(plain_digest).into()
        }
        _ => false,
    }
}

/// Резолв токена: sha256 + постоянное время сравнения + проверка `expires`
/// и `disabled`. Истёкший/отозванный/отключённый пользователь → `None`.
pub fn resolve_token(access: &Access, plain: &str, now_unix: i64) -> Option<AuthCtx> {
    let digest = Sha256::digest(plain.as_bytes());
    let owner = access
        .tokens
        .iter()
        .find(|t| t.expires > now_unix && digest_eq_stored(&digest, &t.token_sha256))
        .map(|t| t.user.clone())?;
    user_ctx(access, &owner)
}

/// Контекст пользователя по логину (учётка должна существовать и быть активной).
fn user_ctx(access: &Access, login: &str) -> Option<AuthCtx> {
    access
        .users
        .iter()
        .find(|u| u.login == login && !u.disabled)
        .map(|u| AuthCtx {
            user: u.login.clone(),
            admin: u.admin,
            projects: u.projects.clone(),
        })
}

/// Пользователь по логину.
pub fn user_by_login<'a>(access: &'a Access, login: &str) -> Option<&'a User> {
    access.users.iter().find(|u| u.login == login)
}

/// Аутентификация логин/пароль (`POST /auth/login`): бракует `disabled`,
/// проверяет пароль. Выдача токена — отдельный вызов [`new_token`].
pub fn authenticate(access: &Access, login: &str, password: &str) -> Option<AuthCtx> {
    let user = user_by_login(access, login)?;
    if user.disabled || !verify_password(&user.password_hash, password) {
        return None;
    }
    Some(AuthCtx {
        user: user.login.clone(),
        admin: user.admin,
        projects: user.projects.clone(),
    })
}

/// Отозвать токен по хэшу; `true` — если найден и удалён.
pub fn revoke_token(access: &mut Access, token_sha256: &str) -> bool {
    let before = access.tokens.len();
    access.tokens.retain(|t| t.token_sha256 != token_sha256);
    access.tokens.len() != before
}

/// Отозвать все токены пользователя; возвращает число отозванных.
pub fn revoke_tokens_of_user(access: &mut Access, login: &str) -> usize {
    let before = access.tokens.len();
    access.tokens.retain(|t| t.user != login);
    before - access.tokens.len()
}

/// Токены пользователя (для `hds token list` / UI).
pub fn tokens_of_user(access: &Access, login: &str) -> Vec<Token> {
    access
        .tokens
        .iter()
        .filter(|t| t.user == login)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Временный каталог под тест (чистится на входе) — как в других крейтах.
    fn tmpdir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("hds-access-{tag}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmp");
        dir
    }

    fn sample_access() -> Access {
        Access {
            users: vec![
                User {
                    name: "Admin".into(),
                    login: "admin".into(),
                    password_hash: hash_password("admin-pw").unwrap(),
                    admin: true,
                    projects: Vec::new(),
                    disabled: false,
                },
                User {
                    name: "Alice".into(),
                    login: "alice".into(),
                    password_hash: hash_password("alice-pw").unwrap(),
                    admin: false,
                    projects: vec!["Sales".into(), "Docs".into()],
                    disabled: false,
                },
                User {
                    name: "Bob".into(),
                    login: "bob".into(),
                    password_hash: hash_password("bob-pw").unwrap(),
                    admin: false,
                    projects: vec!["Sales".into()],
                    disabled: true,
                },
            ],
            tokens: Vec::new(),
        }
    }

    #[test]
    fn yaml_roundtrip_and_skip_defaults() {
        let dir = tmpdir("roundtrip");
        let path = dir.join("access.yaml");
        let acc = sample_access();
        save_at(&path, &acc).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        // Опциональные поля по умолчанию не сериализуются (компактный файл).
        assert!(text.contains("disabled: true"));
        assert!(text.contains("admin: true"));
        assert!(text.contains("argon2id"));

        let back: Access = serde_yaml::from_str(&text).unwrap();
        assert_eq!(back, acc);
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tmpdir("missing");
        let acc = load_at(&dir.join("no-such-file.yaml")).unwrap();
        assert_eq!(acc, Access::default());
    }

    #[test]
    fn password_verify_ok_fail_and_malformed() {
        let h1 = hash_password("secret").unwrap();
        assert!(verify_password(&h1, "secret"));
        assert!(!verify_password(&h1, "Secret"));
        assert!(!verify_password(&h1, ""));
        // Два вызова — разные соли.
        let h2 = hash_password("secret").unwrap();
        assert_ne!(h1, h2);
        // Чужой/битый формат не паникует.
        assert!(!verify_password("not-a-phc", "secret"));
        assert!(!verify_password("$argon2id$broken", "secret"));
    }
    #[test]
    fn authenticate_login_password() {
        let acc = sample_access();
        // Успех, admin-флаг и проекты.
        let ctx = authenticate(&acc, "alice", "alice-pw").unwrap();
        assert_eq!(ctx.user, "alice");
        assert!(!ctx.admin);
        assert_eq!(ctx.projects, vec!["Sales", "Docs"]);
        let ctx = authenticate(&acc, "admin", "admin-pw").unwrap();
        assert!(ctx.admin);
        // Неверный пароль, неизвестный логин.
        assert!(authenticate(&acc, "alice", "wrong").is_none());
        assert!(authenticate(&acc, "nobody", "x").is_none());
        // disabled — вход запрещён.
        assert!(authenticate(&acc, "bob", "bob-pw").is_none());
    }

    #[test]
    fn token_issue_resolve_expire_revoke() {
        let mut acc = sample_access();
        let now = 1_700_000_000i64;
        let (plain, tok) = new_token("alice", 90, now).unwrap();
        assert_eq!(plain.len(), 64);
        assert_eq!(tok.user, "alice");
        assert_eq!(tok.expires, now + 90 * 86_400);
        assert_ne!(tok.token_sha256, plain); // хранится хэш
        acc.tokens.push(tok.clone());

        let ctx = resolve_token(&acc, &plain, now).unwrap();
        assert_eq!(ctx.user, "alice");
        assert_eq!(ctx.projects, vec!["Sales", "Docs"]);
        // Подделка → None.
        let mut forged = plain.clone();
        let first = forged.chars().next().unwrap();
        let flip = if first == 'a' { 'b' } else { 'a' };
        forged.replace_range(0..1, &flip.to_string());
        assert!(resolve_token(&acc, &forged, now).is_none());
        // Истёкший → None; на границе — ещё действует.
        assert!(resolve_token(&acc, &plain, tok.expires + 1).is_none());
        assert!(resolve_token(&acc, &plain, tok.expires - 1).is_some());
        // Отзыв → None.
        assert!(revoke_token(&mut acc, &tok.token_sha256));
        assert!(!revoke_token(&mut acc, &tok.token_sha256));
        assert!(resolve_token(&acc, &plain, now).is_none());
    }
}
