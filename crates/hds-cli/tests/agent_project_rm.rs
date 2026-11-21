//! A6.5: `hds agent project rm` — удаление проекта из анонимной зоны.
//!
//! Проверяется «снаружи», как это увидит пользователь:
//! * `rm P --yes` → 0, каталог зоны, реестр (`db`/`-wal`/`-shm`), запись
//!   `projects.json` и токен в `secrets.json` удалены; **папка проекта цела**;
//! * повторный `rm` → 2 (не 0 и не 500);
//! * живой `sync.lock` → 1 и ничего не удалено; `--force` → 0;
//! * `--purge-meta` → 0, метки удаляются через mock-воркер
//!   (`crates/hds-anonymize/tests/mock_worker.py`);
//! * без `--yes` и без терминала → 2.
//!
//! `index.stop` (общий сигнал, AG-27) команда не создаёт и не удаляет.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hds_cli::cmd::agent::cmd_agent_cfg;
use hds_core::config::{load_from, Config};

/// Тесты трогают env (`HDS_ROOT`, `HDS_CONFIG`, пути воркеров).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

const P: &str = "P0001";

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Корень репозитория: `crates/hds-cli` → два уровня вверх.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Переменные воркеров на время теста (снимаются в `Drop`).
struct EnvGuard(Vec<&'static str>);

impl EnvGuard {
    fn set(pairs: &[(&'static str, PathBuf)]) -> EnvGuard {
        for (key, value) in pairs {
            std::env::set_var(key, value);
        }
        EnvGuard(pairs.iter().map(|(key, _)| *key).collect())
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in &self.0 {
            std::env::remove_var(key);
        }
    }
}

/// Mock-воркер анонимизации; `None` — нет интерпретатора (тест мягко пропускается).
fn mock_env() -> Option<EnvGuard> {
    let root = repo_root();
    let python = hds_anonymize::discover_python(&root)?;
    let mock = root
        .join("crates")
        .join("hds-anonymize")
        .join("tests")
        .join("mock_worker.py");
    Some(EnvGuard::set(&[
        ("HDS_ANONYMIZE_PYTHON", python),
        ("HDS_ANONYMIZE_WORKER", mock),
    ]))
}

/// Стенд: временный каталог (он же `HDS_ROOT`), зона, проект и все следы A6.5.
struct Stand {
    dir: common::TempDir,
    cfg: Config,
    cfg_path: PathBuf,
    project_dir: PathBuf,
}

impl Stand {
    fn new(tag: &str) -> Stand {
        std::env::remove_var("HDS_AGENT_SECRETS");
        std::env::remove_var("HDS_CONFIG");
        let dir = common::TempDir::new(&format!("agentrm-{tag}"));
        let project_dir = dir.join("Проект");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(project_dir.join("документ.docx"), "оригинал").unwrap();
        let zone = dir.join("zone");
        fs::create_dir_all(&zone).unwrap();
        let cfg_path = dir.write_config(&format!(
            "agent:\n  enabled: true\n  root: '{}'\n\
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
            cfg_path,
            project_dir,
        }
    }

    fn agent(&self, args: &[&str]) -> i32 {
        let full: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        cmd_agent_cfg(&self.cfg, &full)
    }

    fn register(&self) -> i32 {
        self.agent(&[
            "project",
            "add",
            P,
            "--name",
            "Пилот",
            "--path",
            &self.project_dir.to_string_lossy(),
        ])
    }

    fn rm(&self, flags: &[&str]) -> i32 {
        let mut args = vec!["project".to_string(), "rm".to_string(), P.to_string()];
        args.extend(flags.iter().map(|f| f.to_string()));
        cmd_agent_cfg(&self.cfg, &args)
    }

    /// `project rm` через настоящий бинарь: stdout нужен целиком.
    ///
    /// stdin — `null`: тест не должен ждать ввода подтверждения (`--yes`).
    fn rm_bin(&self, flags: &[&str]) -> (i32, String) {
        let mut args = vec![
            "agent".to_string(),
            "project".to_string(),
            "rm".to_string(),
            P.to_string(),
        ];
        args.extend(flags.iter().map(|f| f.to_string()));
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_hds"))
            .args(&args)
            .env("HDS_CONFIG", &self.cfg_path)
            .env("HDS_ROOT", &self.dir.path)
            .env_remove("HDS_AGENT_SECRETS")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("hds");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).to_string(),
        )
    }

    fn zone(&self) -> PathBuf {
        self.dir.join("zone")
    }

    fn registry_db(&self) -> PathBuf {
        self.dir
            .join("data")
            .join("agent")
            .join("registry")
            .join(format!("{P}.db"))
    }

    fn projects_json(&self) -> PathBuf {
        self.dir.join("data").join("agent").join("projects.json")
    }

    fn secrets_json(&self) -> PathBuf {
        self.dir.join("data").join("agent").join("secrets.json")
    }

    /// Следы проекта: каталог зоны, реестр (SQLite WAL), токен.
    fn prepare(&self) {
        for rel in [
            "workspace/a.md",
            "agent-config/keys.json",
            "agent-state/sync.log",
        ] {
            let path = self.zone().join(P).join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "[PERSON_1]\n").unwrap();
        }
        fs::create_dir_all(self.registry_db().parent().unwrap()).unwrap();
        fs::write(self.registry_db(), "db").unwrap();
        fs::write(
            PathBuf::from(format!("{}-wal", self.registry_db().display())),
            "wal",
        )
        .unwrap();
        fs::write(
            PathBuf::from(format!("{}-shm", self.registry_db().display())),
            "shm",
        )
        .unwrap();
        assert_eq!(self.agent(&["token", P]), 0, "токен проекта");
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        std::env::remove_var("HDS_CONFIG");
        std::env::remove_var("HDS_ROOT");
    }
}

fn secrets_has_project(path: &Path, id: &str) -> bool {
    hds_agent::secrets::Secrets::load(path)
        .map(|secrets| secrets.has_project(id))
        .unwrap_or(false)
}

fn registry_has_project(path: &Path, id: &str) -> bool {
    hds_agent::projects::load_at(path)
        .map(|registry| registry.get(id).is_some())
        .unwrap_or(false)
}

#[test]
fn rm_removes_zone_registry_secret_and_record_keeps_project() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("full");
    assert_eq!(stand.register(), 0);
    stand.prepare();
    // Общий сигнал остановки не должен пострадать (AG-27).
    std::fs::write(stand.dir.join("index.stop"), b"").unwrap();

    let code = stand.rm(&["--yes"]);
    assert_eq!(code, 0);

    assert!(!stand.zone().join(P).exists(), "каталог зоны удалён");
    assert!(!stand.registry_db().exists());
    assert!(!PathBuf::from(format!("{}-wal", stand.registry_db().display())).exists());
    assert!(!PathBuf::from(format!("{}-shm", stand.registry_db().display())).exists());
    assert!(
        !registry_has_project(&stand.projects_json(), P),
        "запись projects.json удалена"
    );
    assert!(
        !secrets_has_project(&stand.secrets_json(), P),
        "запись проекта в secrets.json удалена"
    );

    // Папка проекта неприкосновенна; общий `index.stop` на месте.
    assert!(stand.project_dir.join("документ.docx").is_file());
    assert!(
        stand.dir.join("index.stop").exists(),
        "index.stop не трогается"
    );

    // Повторный rm — код 2 (не 0 и не 500).
    assert_eq!(stand.rm(&["--yes"]), 2);
}

#[test]
fn live_daemon_lock_refuses_then_force_removes() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("daemon");
    assert_eq!(stand.register(), 0);
    stand.prepare();
    let lock = stand.zone().join(P).join("agent-state").join("sync.lock");
    std::fs::write(&lock, std::process::id().to_string()).unwrap();

    let code = stand.rm(&["--yes"]);
    assert_eq!(code, 1, "живой демон — отказ без --force");
    assert!(stand.zone().join(P).exists(), "ничего не удалено");
    assert!(registry_has_project(&stand.projects_json(), P));

    let code = stand.rm(&["--yes", "--force"]);
    assert_eq!(code, 0, "--force удаляет при живом lock");
    assert!(!stand.zone().join(P).exists());
    assert!(!registry_has_project(&stand.projects_json(), P));
    assert!(stand.project_dir.join("документ.docx").is_file());
}

#[test]
fn purge_meta_drops_labels_through_worker() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_mock) = mock_env() else {
        eprintln!("python для mock-воркера не найден — тест пропущен");
        return;
    };
    let stand = Stand::new("purge");
    assert_eq!(stand.register(), 0);
    stand.prepare();

    let (code, stdout) = stand.rm_bin(&["--yes", "--purge-meta"]);
    assert_eq!(code, 0, "stdout: {stdout}");
    assert!(
        stdout.contains("метки: удалены (--purge-meta)"),
        "воркер должен подтвердить удаление меток: {stdout}"
    );
    assert!(!stand.zone().join(P).exists());
    assert!(!registry_has_project(&stand.projects_json(), P));
}

#[test]
fn rm_without_yes_and_without_terminal_is_code_two() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("noyes");
    assert_eq!(stand.register(), 0);
    stand.prepare();
    // stdin у бинаря — null (нет терминала): подтверждение не читается.
    let (code, _) = stand.rm_bin(&[]);
    assert_eq!(code, 2, "нет терминала для подтверждения");
    assert!(stand.zone().join(P).exists(), "ничего не удалено");
    assert!(registry_has_project(&stand.projects_json(), P));
}

#[test]
fn rm_rejects_bad_project_id() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("badid");
    let code = cmd_agent_cfg(
        &stand.cfg,
        &[
            "project".to_string(),
            "rm".to_string(),
            "../P0001".to_string(),
            "--yes".to_string(),
        ],
    );
    assert_eq!(code, 2, "идентификатор не по D21 — код 2");
}
