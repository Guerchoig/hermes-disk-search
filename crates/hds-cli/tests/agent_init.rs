//! `hds agent init` / `hds agent token` (TR-4.2, TR-4.3): сброс индекса и токен
//! проекта. Индексация в тесте не запускается (`--no-index`) — проверяется
//! разрушительная часть: стоп слежения, удаление `index.db` (+`-wal`/`-shm`) и
//! выхода анонимизации, создание пустой базы, очистка сигналов.

use std::path::{Path, PathBuf};

use hds_core::config::load_from;

/// Тесты трогают env (`HDS_CONFIG`, `HDS_ROOT`, `HDS_AGENT_SECRETS`).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "hds-agentinit-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(p: &Path, text: &str) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, text).unwrap();
}

/// Стенд: зона с одним проектом, старая база, выход анонимизации, конфиг.
fn stand(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = tmpdir(tag);
    let zone = dir.join("zone");
    touch(&zone.join("P0001/workspace/a.md"), "Документ [PERSON_1].");
    touch(&zone.join("P0001/agent-config/keys.json"), "{}");
    touch(&zone.join("P0001/agent-state/session.txt"), "history");
    let db = dir.join("index.db");
    touch(&db, "old-db");
    touch(&dir.join("index.db-wal"), "wal");
    touch(&dir.join("index.db-shm"), "shm");
    let anonymized = dir.join("data").join("anonymized");
    touch(&anonymized.join("old.docx"), "legacy");
    let cfg = format!(
        "agent:\n  enabled: true\n  root: '{}'\n\
         db_path: '{}'\n\
         embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n\
         index:\n  clip: false\n\
         anonymize:\n  output_dir: '{}'\n",
        zone.to_string_lossy().replace('\\', "/"),
        db.to_string_lossy().replace('\\', "/"),
        anonymized.to_string_lossy().replace('\\', "/")
    );
    let cfg_path = dir.join("config.yaml");
    touch(&cfg_path, &cfg);
    std::env::set_var("HDS_CONFIG", &cfg_path);
    std::env::set_var("HDS_ROOT", &dir);
    (dir, cfg_path, db)
}

fn cleanup(dir: &Path) {
    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
    std::env::remove_var("HDS_AGENT_SECRETS");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn init_resets_index_and_removes_legacy_output() {
    let _env = ENV_LOCK.lock().unwrap();
    let (dir, cfg_path, db) = stand("init");
    let cfg = load_from(&cfg_path).unwrap();

    let code = hds_cli::cmd::agent::cmd_agent_cfg(
        &cfg,
        &[
            "init".to_string(),
            "--yes".to_string(),
            "--no-index".to_string(),
        ],
    );
    assert_eq!(code, 0);

    // База создана заново и пустая (нет ни одного файла проекта).
    assert!(db.exists(), "пустая база должна быть создана");
    let conn = hds_core::db::connect(&db, 4).unwrap();
    let files: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(files, 0);
    drop(conn);

    // Прежние `-wal`/`-shm` снесены (новых нет: соединение закрыто).
    assert!(!dir.join("index.db-wal").exists());
    assert!(!dir.join("index.db-shm").exists());
    // Выход прежней карточки анонимизации удалён.
    assert!(!dir.join("data/anonymized").exists());
    // Сигналы не оставлены.
    assert!(!dir.join("index.stop").exists());

    cleanup(&dir);
}

#[test]
fn init_refuses_output_dir_outside_allowed_roots_unless_forced() {
    let _env = ENV_LOCK.lock().unwrap();
    let (dir, cfg_path, _db) = stand("foreign");
    // Владелец указал в anonymize.output_dir общий каталог вне data/ и вне зоны.
    let foreign = dir.join("foreign-out");
    touch(&foreign.join("shared.docx"), "не наш каталог");
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    let text = text.replace(
        &format!(
            "anonymize:\n  output_dir: '{}'",
            dir.join("data/anonymized")
                .to_string_lossy()
                .replace('\\', "/")
        ),
        &format!(
            "anonymize:\n  output_dir: '{}'",
            foreign.to_string_lossy().replace('\\', "/")
        ),
    );
    std::fs::write(&cfg_path, text).unwrap();
    let cfg = load_from(&cfg_path).unwrap();

    // Без --force — отказ (В-10), каталог цел.
    let code = hds_cli::cmd::agent::cmd_agent_cfg(
        &cfg,
        &[
            "init".to_string(),
            "--yes".to_string(),
            "--no-index".to_string(),
        ],
    );
    assert_eq!(code, 1, "чужой каталог без --force удалять нельзя");
    assert!(
        foreign.join("shared.docx").exists(),
        "каталог должен остаться"
    );

    // С --force — удаляется (владелец подтвердил, что это выход карточки).
    let code = hds_cli::cmd::agent::cmd_agent_cfg(
        &cfg,
        &[
            "init".to_string(),
            "--yes".to_string(),
            "--no-index".to_string(),
            "--force".to_string(),
        ],
    );
    assert_eq!(code, 0);
    assert!(
        !foreign.exists(),
        "--force должен удалить указанный каталог"
    );

    cleanup(&dir);
}

#[test]
fn init_requires_agent_mode_and_project() {
    let _env = ENV_LOCK.lock().unwrap();
    // Режим агента выключен → отказ (код 1), ничего не удаляется.
    let dir = tmpdir("off");
    let cfg_path = dir.join("config.yaml");
    touch(
        &cfg_path,
        "agent:\n  enabled: false\n  root: 'D:/hds-anon'\ndb_path: 'index.db'\n",
    );
    let cfg = load_from(&cfg_path).unwrap();
    let code = hds_cli::cmd::agent::cmd_agent_cfg(&cfg, &["init".to_string(), "--yes".to_string()]);
    assert_eq!(code, 1, "без agent.enabled init не работает");
    let _ = std::fs::remove_dir_all(&dir);

    // Режим включён, но проектов нет → отказ с подсказкой.
    let dir = tmpdir("empty");
    let zone = dir.join("zone");
    std::fs::create_dir_all(&zone).unwrap();
    let cfg_path = dir.join("config.yaml");
    touch(
        &cfg_path,
        &format!(
            "agent:\n  enabled: true\n  root: '{}'\ndb_path: '{}'\n",
            zone.to_string_lossy().replace('\\', "/"),
            dir.join("index.db").to_string_lossy().replace('\\', "/")
        ),
    );
    let cfg = load_from(&cfg_path).unwrap();
    let code = hds_cli::cmd::agent::cmd_agent_cfg(&cfg, &["init".to_string(), "--yes".to_string()]);
    assert_eq!(code, 1, "без P*/workspace индексировать нечего");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn token_is_hashed_and_stored() {
    let _env = ENV_LOCK.lock().unwrap();
    let (dir, cfg_path, _db) = stand("token");
    let cfg = load_from(&cfg_path).unwrap();
    let secrets_path = dir.join("secrets.json");
    std::env::set_var("HDS_AGENT_SECRETS", &secrets_path);

    let code =
        hds_cli::cmd::agent::cmd_agent_cfg(&cfg, &["token".to_string(), "P0001".to_string()]);
    assert_eq!(code, 0);
    let secrets = hds_mcp::agent::load_at(&secrets_path).unwrap();
    let tok = secrets
        .projects
        .get("P0001")
        .expect("токен проекта сохранён");
    assert_eq!(tok.expires, 0, "без --ttl-days токен бессрочный");
    assert_eq!(tok.token_sha256.len(), 64, "в файл пишется хэш");
    let plain = tok
        .token
        .as_deref()
        .expect("plain-токен нужен агенту для MCP-настроек");
    assert_eq!(
        hds_core::access::token_sha256_hex(plain),
        tok.token_sha256,
        "хэш и plain-токен описывают один токен"
    );
    // Неверный id проекта — отказ.
    let code = hds_cli::cmd::agent::cmd_agent_cfg(&cfg, &["token".to_string(), "P01".to_string()]);
    assert_eq!(code, 2);

    cleanup(&dir);
}
