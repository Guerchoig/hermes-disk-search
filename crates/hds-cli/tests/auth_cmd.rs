//! Тесты команд доступа (M1 T5): user add/list/grant/revoke/passwd/del,
//! token list/revoke. Хранилище — временный файл (без HDS_ROOT).

use hds_cli::cmd::auth::{cmd_token, cmd_user};
use hds_core::access::{authenticate, load_at, new_token, save_at, Access};
use std::path::PathBuf;

fn tmp_access(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("hds-cli-auth-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("access.yaml")
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn user_add_list_grant_revoke() {
    let p = tmp_access("agr");
    // add
    assert_eq!(
        cmd_user(
            "add",
            &args(&[
                "--login",
                "alice",
                "--password",
                "pw1",
                "--projects",
                "Sales,Docs"
            ]),
            Some(p.clone())
        ),
        0
    );
    // дубль логина — ошибка
    assert_ne!(
        cmd_user(
            "add",
            &args(&["--login", "alice", "--password", "x"]),
            Some(p.clone())
        ),
        0
    );
    // пароль хэширован (не лежит открытым текстом)
    let acc: Access = load_at(&p).unwrap();
    assert!(acc.users[0].password_hash.starts_with("$argon2id$"));
    // authenticate работает
    assert!(authenticate(&acc, "alice", "pw1").is_some());
    // grant добавляет проект
    assert_eq!(
        cmd_user(
            "grant",
            &args(&["--login", "alice", "--projects", "Ops"]),
            Some(p.clone())
        ),
        0
    );
    let acc: Access = load_at(&p).unwrap();
    assert_eq!(acc.users[0].projects, vec!["Sales", "Docs", "Ops"]);
    // revoke убирает
    assert_eq!(
        cmd_user(
            "revoke",
            &args(&["--login", "alice", "--projects", "Docs"]),
            Some(p.clone())
        ),
        0
    );
    let acc: Access = load_at(&p).unwrap();
    assert_eq!(acc.users[0].projects, vec!["Sales", "Ops"]);
    // list — код 0
    assert_eq!(cmd_user("list", &args(&[]), Some(p.clone())), 0);
}

#[test]
fn user_passwd_invalidates_and_rotates() {
    let p = tmp_access("passwd");
    cmd_user(
        "add",
        &args(&["--login", "bob", "--password", "old"]),
        Some(p.clone()),
    );
    // token bob'а (напрямую через access API — как после /auth/login)
    let mut acc: Access = load_at(&p).unwrap();
    let (plain, tok) = new_token("bob", 90, 1_700_000_000).unwrap();
    acc.tokens.push(tok);
    save_at(&p, &acc).unwrap();
    // passwd
    assert_eq!(
        cmd_user(
            "passwd",
            &args(&["--login", "bob", "--password", "new"]),
            Some(p.clone())
        ),
        0
    );
    let acc: Access = load_at(&p).unwrap();
    // старый пароль больше не работает, новый — работает
    assert!(authenticate(&acc, "bob", "old").is_none());
    assert!(authenticate(&acc, "bob", "new").is_some());
    // токены отозваны
    assert!(acc.tokens.is_empty(), "смена пароля отзывает токены");
    let _ = plain;
}

#[test]
fn user_del_removes_tokens() {
    let p = tmp_access("del");
    cmd_user(
        "add",
        &args(&["--login", "carl", "--password", "pw"]),
        Some(p.clone()),
    );
    let mut acc: Access = load_at(&p).unwrap();
    let (_, tok) = new_token("carl", 90, 1_700_000_000).unwrap();
    acc.tokens.push(tok);
    save_at(&p, &acc).unwrap();
    assert_eq!(
        cmd_user("del", &args(&["--login", "carl"]), Some(p.clone())),
        0
    );
    let acc: Access = load_at(&p).unwrap();
    assert!(acc.users.is_empty());
    assert!(
        acc.tokens.is_empty(),
        "токены удалённого пользователя отозваны"
    );
    // del несуществующего — ошибка
    assert_ne!(
        cmd_user("del", &args(&["--login", "carl"]), Some(p.clone())),
        0
    );
}

#[test]
fn token_list_and_revoke_by_prefix() {
    let p = tmp_access("tok");
    cmd_user(
        "add",
        &args(&["--login", "dana", "--password", "pw"]),
        Some(p.clone()),
    );
    let mut acc: Access = load_at(&p).unwrap();
    let (plain1, tok1) = new_token("dana", 90, 1_700_000_000).unwrap();
    acc.tokens.push(tok1);
    save_at(&p, &acc).unwrap();
    // list — код 0
    assert_eq!(
        cmd_token("list", &args(&["--user", "dana"]), Some(p.clone())),
        0
    );
    // revoke по префиксу хэша (как из token list)
    let sha = hds_core::access::token_sha256_hex(&plain1);
    let prefix = &sha[..12];
    assert_eq!(
        cmd_token("revoke", &args(&["--sha", prefix]), Some(p.clone())),
        0
    );
    let acc: Access = load_at(&p).unwrap();
    assert!(acc.tokens.is_empty());
    // повторный revoke — ошибка
    assert_ne!(
        cmd_token("revoke", &args(&["--sha", prefix]), Some(p.clone())),
        0
    );
}
