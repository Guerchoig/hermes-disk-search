//! A4.1: `hds agent sync --once` и реестр артефактов `data\agent\registry\<P>.db`.
//!
//! Что проверяется:
//! * на пустой папке проекта команда создаёт реестр и **ничего не ломает**
//!   (папка проекта не меняется, `index.db` не нужен);
//! * повторный прогон идемпотентен: реестр не пересоздаётся, записи живы;
//! * путь реестра — зона пользователя (`<state_dir>\data\agent\registry`),
//!   а не `<agent.root>`;
//! * отказы: неверный идентификатор проекта, незарегистрированный проект,
//!   выключенная анонимная зона — понятный текст и код возврата.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hds_agent::registry_db::{Artifact, ArtifactRegistry, Kind, Status};
use hds_cli::cmd::agent::cmd_agent_cfg;
use hds_core::config::{load_from, Config};

/// Тесты трогают env (`HDS_ROOT`, `HDS_CONFIG`) — гонка недопустима.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Стенд: временный `<tmp>` (он же `HDS_ROOT`), зона агента и папка проекта.
struct Stand {
    dir: common::TempDir,
    cfg: Config,
    project_dir: PathBuf,
}

impl Stand {
    fn new(tag: &str, enabled: bool) -> Stand {
        std::env::remove_var("HDS_AGENT_SECRETS");
        std::env::remove_var("HDS_CONFIG");
        let dir = common::TempDir::new(&format!("agentsync-{tag}"));
        // Папка проекта намеренно **пустая**: A4.1 проверяет реестр, а не обход.
        let project_dir = dir.join("Проект");
        fs::create_dir_all(&project_dir).unwrap();
        let zone = dir.join("zone");
        fs::create_dir_all(&zone).unwrap();
        let cfg_path = dir.write_config(&format!(
            "agent:\n  enabled: {enabled}\n  root: '{}'\n\
             db_path: '{}'\n\
             embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n\
             index:\n  clip: false\n",
            slash(&zone),
            slash(&dir.join("index.db")),
        ));
        std::env::set_var("HDS_CONFIG", &cfg_path);
        std::env::set_var("HDS_ROOT", &dir.path);
        let cfg = load_from(&cfg_path).expect("config.yaml");
        Stand {
            dir,
            cfg,
            project_dir,
        }
    }

    fn register(&self, id: &str) -> i32 {
        cmd_agent_cfg(
            &self.cfg,
            &[
                "project".to_string(),
                "add".to_string(),
                id.to_string(),
                "--name".to_string(),
                "Пилот".to_string(),
                "--path".to_string(),
                self.project_dir.to_string_lossy().to_string(),
            ],
        )
    }

    fn sync(&self, args: &[&str]) -> i32 {
        let mut full: Vec<String> = vec!["sync".to_string()];
        full.extend(args.iter().map(|a| a.to_string()));
        cmd_agent_cfg(&self.cfg, &full)
    }

    /// Путь реестра по правилу TR-7.2 (зона пользователя, не `<agent.root>`).
    fn registry_path(&self, id: &str) -> PathBuf {
        self.dir
            .path
            .join("data")
            .join("agent")
            .join("registry")
            .join(format!("{id}.db"))
    }
}

#[test]
fn sync_creates_registry_on_empty_folder() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("create", true);
    assert_eq!(stand.register("P0001"), 0, "проект регистрируется");
    let registry = stand.registry_path("P0001");
    assert!(!registry.exists(), "до первого прогона реестра нет");

    let code = stand.sync(&["--project", "P0001", "--once"]);
    assert_eq!(code, 0, "sync на пустой папке — успех");
    assert!(registry.is_file(), "реестр создан: {}", registry.display());

    let reg = ArtifactRegistry::open(&registry).unwrap();
    assert_eq!(reg.count().unwrap(), 0);
    assert!(reg.list_pending().unwrap().is_empty());

    // Папка проекта не изменилась: обход появится в A4.2.
    let entries: Vec<_> = fs::read_dir(&stand.project_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert!(entries.is_empty(), "в папке проекта ничего не появилось");
}

#[test]
fn sync_is_idempotent_and_keeps_rows() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("repeat", true);
    assert_eq!(stand.register("P0001"), 0);
    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 0);
    let registry = stand.registry_path("P0001");

    // Запись, сделанная «демоном», переживает повторный прогон.
    {
        let reg = ArtifactRegistry::open(&registry).unwrap();
        let mut artifact = Artifact::new(
            "Протоколы\\Встреча.docx",
            "Протоколы\\Встреча с [PERSON_3].docx",
            Kind::Image,
            Status::Synced,
        );
        artifact.orig_sha256 = Some("base".to_string());
        reg.insert(&artifact).unwrap();
    }
    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 0);

    let reg = ArtifactRegistry::open(&registry).unwrap();
    assert_eq!(reg.count().unwrap(), 1, "повторный прогон не чистит реестр");
    assert!(reg.by_orig("Протоколы\\Встреча.docx").unwrap().is_some());
}

#[test]
fn sync_rejects_bad_and_unregistered_project() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("reject", true);
    assert_eq!(stand.register("P0001"), 0);

    // Не идентификатор проекта (D21) — отказ до построения пути.
    assert_eq!(stand.sync(&["--project", "P01", "--once"]), 2);
    // Незарегистрированный проект — отказ.
    assert_eq!(stand.sync(&["--project", "P0002", "--once"]), 2);
    // Без --project — подсказка и код 2.
    assert_eq!(stand.sync(&["--once"]), 2);
    assert!(!stand.registry_path("P01").exists());
    assert!(!stand.registry_path("P0002").exists());
}

#[test]
fn sync_refuses_when_agent_disabled() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("disabled", false);
    assert_eq!(stand.register("P0001"), 0);
    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 1);
    assert!(!stand.registry_path("P0001").exists());
}
