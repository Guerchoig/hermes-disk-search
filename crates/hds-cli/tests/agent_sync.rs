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

    // A4.2: запись о **существующем** оригинале переживает повторный прогон.
    // Готовим пару «оригинал ↔ образ» с совпадающими sha256 — тогда проходу
    // не нужен воркер, а изменение (и «чистка») были бы видны по счётчикам.
    let project_rel = "Протоколы\\Встреча.docx";
    let anon_rel = "Протоколы\\Встреча с [PERSON_3].docx";
    fs::create_dir_all(stand.project_dir.join("Протоколы")).unwrap();
    let original = stand.project_dir.join(project_rel);
    fs::write(&original, "версия документа").unwrap();
    let workspace = stand.dir.join("zone").join("P0001").join("workspace");
    fs::create_dir_all(workspace.join("Протоколы")).unwrap();
    let image = workspace.join(anon_rel);
    fs::write(&image, "образ документа").unwrap();
    let orig_sha = hds_agent::mirror::sha256_file(&original).unwrap();
    let image_sha = hds_agent::mirror::sha256_file(&image).unwrap();
    {
        let reg = ArtifactRegistry::open(&registry).unwrap();
        let mut artifact = Artifact::new(project_rel, anon_rel, Kind::Image, Status::Synced);
        artifact.orig_sha256 = Some(orig_sha);
        artifact.anon_sha256_issued = Some(image_sha.clone());
        artifact.anon_sha256_seen = Some(image_sha);
        reg.insert(&artifact).unwrap();
    }
    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 0);

    let reg = ArtifactRegistry::open(&registry).unwrap();
    assert_eq!(reg.count().unwrap(), 1, "повторный прогон не чистит реестр");
    assert!(reg.by_orig(project_rel).unwrap().is_some());
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

/// Корень репозитория: `crates/hds-cli` → два уровня вверх.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Вернуть переменные окружения в исходное состояние после теста.
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

/// A4.2 e2e: прямой путь на mock-воркере (NER-модели не нужны).
///
/// Проверяется «снаружи»: образ появился под анонимным именем и с метками,
/// служебные записи не выпущены, реестр знает про unsupported, а повторный
/// прогон не пересоздаёт образ (mtime не меняется).
#[test]
fn sync_forward_path_creates_labeled_images() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = repo_root();
    let Some(python) = hds_anonymize::discover_python(&root) else {
        eprintln!("skip: нет интерпретатора воркера анонимизации");
        return;
    };
    let mock = root
        .join("crates")
        .join("hds-anonymize")
        .join("tests")
        .join("mock_worker.py");
    let _env = EnvGuard::set(&[
        ("HDS_ANONYMIZE_PYTHON", python),
        ("HDS_ANONYMIZE_WORKER", mock),
    ]);

    let stand = Stand::new("forward", true);
    fs::create_dir_all(stand.project_dir.join("Протоколы")).unwrap();
    fs::write(
        stand.project_dir.join("Протоколы\\Встреча Иванов Иван.md"),
        "# Встреча\n\nКонтакт: Иванов Иван\n",
    )
    .unwrap();
    fs::write(stand.project_dir.join("данные.bin"), b"\x00\x01binary").unwrap();
    fs::write(stand.project_dir.join("~$Встреча.docx"), "lock").unwrap();
    fs::create_dir_all(stand.project_dir.join(".hds-agent")).unwrap();
    fs::write(stand.project_dir.join(".hds-agent\\state.json"), "{}").unwrap();

    assert_eq!(stand.register("P0001"), 0);
    assert_eq!(
        stand.sync(&["--project", "P0001", "--once"]),
        0,
        "прямой путь на mock-воркере"
    );

    let workspace = stand.dir.join("zone").join("P0001").join("workspace");
    let image = workspace.join("Протоколы\\Встреча [PERSON_1].md");
    let text = fs::read_to_string(&image).expect("образ с анонимным именем");
    assert!(text.contains("[PERSON_1]"), "{text}");
    assert!(!text.contains("Иванов Иван"), "значение не утекло в образ");

    let registry = ArtifactRegistry::open(&stand.registry_path("P0001")).unwrap();
    let row = registry
        .by_orig("Протоколы\\Встреча Иванов Иван.md")
        .unwrap()
        .expect("запись образа");
    assert_eq!(row.status, Status::Synced);
    assert_eq!(row.anon_rel, "Протоколы\\Встреча [PERSON_1].md");
    assert_eq!(row.kind, Kind::Image);
    assert!(row.orig_sha256.is_some());

    let bin = registry
        .by_orig("данные.bin")
        .unwrap()
        .expect("запись unsupported");
    assert_eq!(bin.status, Status::Unsupported);
    assert!(registry
        .by_orig(".hds-agent\\state.json")
        .unwrap()
        .is_none());
    assert!(registry.by_orig("~$Встреча.docx").unwrap().is_none());
    let records = registry.count().unwrap();
    drop(registry);

    // Повторный прогон: 0 изменений — образ не перезаписан (mtime тот же).
    let mtime_before = fs::metadata(&image).unwrap().modified().unwrap();
    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 0);
    let mtime_after = fs::metadata(&image).unwrap().modified().unwrap();
    assert_eq!(mtime_before, mtime_after, "образ не пересоздавался");
    let registry = ArtifactRegistry::open(&stand.registry_path("P0001")).unwrap();
    assert_eq!(registry.count().unwrap(), records, "реестр не раздулся");
}

/// A4.3 e2e на mock-воркере: новый файл агента → папка проекта, правка
/// образа → вариант в `.hds-agent\pending` со снимком основания.
///
/// Проверяется «снаружи»: маркер образа не даёт повторного выпуска, оригинал
/// при сборке варианта не меняется (I6), служебная папка `.hds-agent` не
/// зеркалится обратно.
#[test]
fn sync_reverse_emits_agent_files_and_pending_variant() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = repo_root();
    let Some(python) = hds_anonymize::discover_python(&root) else {
        eprintln!("skip: нет интерпретатора воркера анонимизации");
        return;
    };
    let mock = root
        .join("crates")
        .join("hds-anonymize")
        .join("tests")
        .join("mock_worker.py");
    let _env = EnvGuard::set(&[
        ("HDS_ANONYMIZE_PYTHON", python),
        ("HDS_ANONYMIZE_WORKER", mock),
    ]);

    let stand = Stand::new("reverse", true);
    let letter = "Письмо Иванов Иван.md";
    fs::write(stand.project_dir.join(letter), "Контакт: Иванов Иван\n").unwrap();
    assert_eq!(stand.register("P0001"), 0);
    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 0);

    let workspace = stand.dir.join("zone").join("P0001").join("workspace");
    let image = workspace.join("Письмо [PERSON_1].md");
    let original = stand.project_dir.join(letter);
    let original_sha = hds_agent::mirror::sha256_file(&original).unwrap();
    let image_text = fs::read_to_string(&image).expect("образ письма");
    assert!(image_text.contains("[PERSON_1]"), "{image_text}");
    assert!(
        image_text.starts_with("<!-- hds-agent/P0001/"),
        "маркер образа: {image_text}"
    );
    let marker_line = image_text.lines().next().unwrap().to_string();

    // Агент правит образ (маркер остаётся) и создаёт новый файл.
    fs::write(&image, format!("{marker_line}\n[PERSON_1] правка агента\n")).unwrap();
    fs::write(
        workspace.join("Отчёт [PERSON_1].md"),
        "[PERSON_1] подготовил отчёт\n",
    )
    .unwrap();

    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 0);

    // Новый файл — на своём месте в папке проекта, с реальным именем.
    let emitted = stand.project_dir.join("Отчёт Иванов Иван.md");
    let emitted_text = fs::read_to_string(&emitted).expect("выпущенный файл агента");
    assert!(emitted_text.contains("Иванов Иван"), "{emitted_text}");
    assert!(!emitted_text.contains("[PERSON_"), "{emitted_text}");
    assert!(
        !emitted_text.contains("hds-agent"),
        "маркер не уехал: {emitted_text}"
    );

    // Изменённый образ — вариант в pending, оригинал не тронут (I6).
    let pending = stand
        .project_dir
        .join(".hds-agent")
        .join("pending")
        .join(letter);
    let pending_text = fs::read_to_string(&pending).expect("вариант в pending");
    assert!(pending_text.contains("Иванов Иван"), "{pending_text}");
    assert!(pending_text.contains("правка агента"), "{pending_text}");
    assert!(
        !pending_text.contains("hds-agent"),
        "маркер не уехал: {pending_text}"
    );
    let base = stand
        .project_dir
        .join(".hds-agent")
        .join("base")
        .join(letter);
    assert!(base.is_file(), "снимок основания");
    assert_eq!(
        hds_agent::mirror::sha256_file(&original).unwrap(),
        original_sha,
        "оригинал не изменился"
    );

    let registry = ArtifactRegistry::open(&stand.registry_path("P0001")).unwrap();
    let row = registry.by_orig(letter).unwrap().expect("запись письма");
    assert_eq!(row.status, Status::PendingAccept);
    let new_row = registry
        .by_orig("Отчёт Иванов Иван.md")
        .unwrap()
        .expect("запись agent_new");
    assert_eq!(new_row.kind, Kind::AgentNew);
    let records = registry.count().unwrap();
    let project_files = fs::read_dir(&stand.project_dir).unwrap().count();
    drop(registry);

    // Петля TR-7.7: повторный проход ничего не выпускает и не плодит записей.
    assert_eq!(stand.sync(&["--project", "P0001", "--once"]), 0);
    let registry = ArtifactRegistry::open(&stand.registry_path("P0001")).unwrap();
    assert_eq!(registry.count().unwrap(), records);
    assert_eq!(
        fs::read_dir(&stand.project_dir).unwrap().count(),
        project_files
    );
}
