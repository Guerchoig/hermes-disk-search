//! Команды управления доступом (M1 T5, `PLAN_M1_ACCESS_PROJECTS.md` §4):
//! `hds user add|list|del|passwd|grant|revoke`, `hds token list|revoke`,
//! `hds login [--url ...]` (клиентский помощник: login/password → токен).
//!
//! Хранилище — `data/access.yaml` (через `hds-core::access`). В тестах
//! `cmd_user`/`cmd_token` получают явный путь к файлу (изоляция от HDS_ROOT).

use std::path::PathBuf;

use hds_core::access::{
    hash_password, load_at, revoke_token, revoke_tokens_of_user, save_at, user_by_login, Access,
    User,
};

/// Путь к хранилищу по умолчанию (или явный — из тестов).
fn store(access_path: Option<&PathBuf>) -> PathBuf {
    access_path
        .cloned()
        .unwrap_or_else(hds_core::access::access_path)
}

fn csv(v: Option<String>) -> Vec<String> {
    v.map(|s| {
        s.split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(name) {
            if let Some(v) = rest.strip_prefix('=') {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn flag_bool(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn usage(msg: &str) -> i32 {
    eprintln!("{msg}");
    2
}

/// `hds user <подкоманда> [...]`; `access_path` — явный путь (тесты).
pub fn cmd_user(sub: &str, args: &[String], access_path: Option<PathBuf>) -> i32 {
    let path = store(access_path.as_ref());
    match sub {
        "add" => user_add(&path, args),
        "list" => user_list(&path),
        "del" => match flag(args, "--login") {
            Some(l) => user_del(&path, &l),
            None => usage("user del --login <логин>"),
        },
        "passwd" => match (flag(args, "--login"), flag(args, "--password")) {
            (Some(l), Some(p)) => user_passwd(&path, &l, &p),
            _ => usage("user passwd --login <логин> --password <пароль>"),
        },
        "grant" => match (flag(args, "--login"), flag(args, "--projects")) {
            (Some(l), Some(p)) => user_grant(&path, &l, &csv(Some(p)), true),
            _ => usage("user grant --login <логин> --projects \"A,B\""),
        },
        "revoke" => match (flag(args, "--login"), flag(args, "--projects")) {
            (Some(l), Some(p)) => user_grant(&path, &l, &csv(Some(p)), false),
            _ => usage("user revoke --login <логин> --projects \"A,B\""),
        },
        other => usage(&format!(
            "user: неизвестная подкоманда '{other}' (add|list|del|passwd|grant|revoke)"
        )),
    }
}

fn load(path: &std::path::Path) -> Result<Access, String> {
    load_at(path).map_err(|e| e.message())
}

fn save(path: &std::path::Path, acc: &Access) -> Result<(), String> {
    save_at(path, acc).map_err(|e| e.message())
}

fn user_add(path: &std::path::Path, args: &[String]) -> i32 {
    let login = match flag(args, "--login") {
        Some(l) if !l.trim().is_empty() => l,
        _ => return usage("user add --login <логин> --password <пароль> [--name N] [--admin] [--projects \"A,B\"]"),
    };
    let password = match flag(args, "--password") {
        Some(p) => p,
        None => return usage("user add: нужен --password"),
    };
    let mut acc = match load(path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("доступ: {e}");
            return 1;
        }
    };
    if user_by_login(&acc, &login).is_some() {
        eprintln!("user add: логин '{login}' уже существует");
        return 1;
    }
    let hash = match hash_password(&password) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("хэш пароля: {e}");
            return 1;
        }
    };
    let projects = csv(flag(args, "--projects"));
    acc.users.push(User {
        name: flag(args, "--name").unwrap_or_else(|| login.clone()),
        login: login.clone(),
        password_hash: hash,
        admin: flag_bool(args, "--admin"),
        projects: projects.clone(),
        disabled: false,
    });
    if let Err(e) = save(path, &acc) {
        eprintln!("запись {}: {}", path.display(), e);
        return 1;
    }
    println!(
        "Создан пользователь {login} (admin: {}), проекты: {}",
        flag_bool(args, "--admin"),
        if projects.is_empty() {
            "—".to_string()
        } else {
            projects.join(", ")
        }
    );
    0
}

fn user_list(path: &std::path::Path) -> i32 {
    let acc = match load(path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("доступ: {e}");
            return 1;
        }
    };
    for u in &acc.users {
        println!(
            "{}\t{}\tadmin={}\tdisabled={}\tпроекты: {}",
            u.login,
            u.name,
            u.admin,
            u.disabled,
            if u.projects.is_empty() {
                "—".to_string()
            } else {
                u.projects.join(", ")
            }
        );
    }
    0
}

fn user_del(path: &std::path::Path, login: &str) -> i32 {
    let mut acc = match load(path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("доступ: {e}");
            return 1;
        }
    };
    if user_by_login(&acc, login).is_none() {
        eprintln!("user del: логин '{login}' не найден");
        return 1;
    }
    acc.users.retain(|u| u.login != login);
    let revoked = revoke_tokens_of_user(&mut acc, login);
    if let Err(e) = save(path, &acc) {
        eprintln!("запись: {e}");
        return 1;
    }
    println!("Удалён пользователь {login}; отозвано токенов: {revoked}");
    0
}

fn user_passwd(path: &std::path::Path, login: &str, password: &str) -> i32 {
    let mut acc = match load(path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("доступ: {e}");
            return 1;
        }
    };
    let hash = match hash_password(password) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("хэш пароля: {e}");
            return 1;
        }
    };
    match acc.users.iter_mut().find(|u| u.login == login) {
        Some(u) => u.password_hash = hash,
        None => {
            eprintln!("user passwd: логин '{login}' не найден");
            return 1;
        }
    }
    // Все токены пользователя отзываются при смене пароля (защита).
    let revoked = revoke_tokens_of_user(&mut acc, login);
    if let Err(e) = save(path, &acc) {
        eprintln!("запись: {e}");
        return 1;
    }
    println!("Пароль обновлён ({login}); отозвано токенов: {revoked}");
    0
}

/// Выдать (`add=true`) или отозвать (`add=false`) проекты пользователя.
fn user_grant(path: &std::path::Path, login: &str, projects: &[String], add: bool) -> i32 {
    let mut acc = match load(path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("доступ: {e}");
            return 1;
        }
    };
    let user = match acc.users.iter_mut().find(|u| u.login == login) {
        Some(u) => u,
        None => {
            eprintln!("user: логин '{login}' не найден");
            return 1;
        }
    };
    for p in projects {
        if add {
            if !user.projects.contains(p) {
                user.projects.push(p.clone());
            }
        } else {
            user.projects.retain(|x| x != p);
        }
    }
    let projects = user.projects.clone();
    if let Err(e) = save(path, &acc) {
        eprintln!("запись: {e}");
        return 1;
    }
    println!(
        "Проекты {login}: {}",
        if projects.is_empty() {
            "—".to_string()
        } else {
            projects.join(", ")
        }
    );
    0
}
/// `hds token list|revoke`; `access_path` — явный путь (тесты).
pub fn cmd_token(sub: &str, args: &[String], access_path: Option<PathBuf>) -> i32 {
    let path = store(access_path.as_ref());
    let mut acc = match load(&path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("доступ: {e}");
            return 1;
        }
    };
    match sub {
        "list" => {
            let user = flag(args, "--user");
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            for t in &acc.tokens {
                let expired = t.expires <= now;
                if let Some(u) = &user {
                    if t.user != *u {
                        continue;
                    }
                }
                println!(
                    "{}…\tuser={}\tистекает={}\t{}",
                    &t.token_sha256[..12.min(t.token_sha256.len())],
                    t.user,
                    t.expires,
                    if expired { "ПРОСРОЧЕН" } else { "" }
                );
            }
            0
        }
        "revoke" => {
            let changed;
            if let Some(u) = flag(args, "--user") {
                let n = revoke_tokens_of_user(&mut acc, &u);
                println!("Отозвано токенов {u}: {n}");
                changed = n > 0;
            } else if let Some(sha) = flag(args, "--sha") {
                // По префиксу хэша (для копипасты из `token list`).
                let hit: Vec<String> = acc
                    .tokens
                    .iter()
                    .map(|t| t.token_sha256.clone())
                    .filter(|h| h.starts_with(&sha))
                    .collect();
                if hit.is_empty() {
                    eprintln!("token revoke: токен с префиксом '{sha}' не найден");
                    return 1;
                }
                if hit.len() > 1 {
                    eprintln!("token revoke: префикс неоднозначен ({} токенов)", hit.len());
                    return 1;
                }
                let _ = revoke_token(&mut acc, &hit[0]);
                println!("Токен отозван");
                changed = true;
            } else {
                eprintln!("token revoke --sha <префикс хэша> | --user <логин>");
                return 2;
            }
            if changed {
                if let Err(e) = save(&path, &acc) {
                    eprintln!("запись: {e}");
                    return 1;
                }
            }
            0
        }
        other => usage(&format!(
            "token: неизвестная подкоманда '{other}' (list|revoke)"
        )),
    }
}

/// `hds login [--url http://127.0.0.1:8787] --login L --password P` —
/// клиентский помощник: логин/пароль → токен (JSON на stdout).
pub fn cmd_login(args: &[String]) -> i32 {
    let url = flag(args, "--url").unwrap_or_else(|| "http://127.0.0.1:8787".to_string());
    let login = flag(args, "--login").unwrap_or_default();
    let password = flag(args, "--password").unwrap_or_default();
    if login.is_empty() || password.is_empty() {
        return usage("login --login <логин> --password <пароль> [--url http://host:port]");
    }
    // Парс упрощённый: http://host:port[/...]
    let rest = url
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    let (hostport, base_path) = match rest.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{p}")),
        None => (rest.to_string(), String::new()),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(8787)),
        None => (hostport.clone(), 8787),
    };
    let body = format!(r#"{{"login":"{login}","password":"{password}"}}"#);
    let path = format!("{base_path}/auth/login");
    let reply = hds_core::http::request(
        &host,
        port,
        "POST",
        &path,
        &[],
        Some(&body),
        std::time::Duration::from_secs(10),
    );
    let resp = match reply {
        Ok(r) => r,
        Err(e) => {
            eprintln!("login: сервер {host}:{port} недоступен ({e})");
            return 1;
        }
    };
    if resp.status != 200 {
        eprintln!("login: отказ ({}): {}", resp.status, resp.body.trim());
        return 1;
    }
    println!("{}", resp.body.trim());
    0
}
