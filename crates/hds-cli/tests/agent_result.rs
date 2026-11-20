//! A4.4: `hds agent result` — «Принять»/«Отклонить»/конфликт по варианту агента.
//!
//! Проверяется «снаружи», как это увидит пользователь и вкладка «Агент»:
//! * `accept` → оригинал равен варианту, копия прежней версии в
//!   `.hds-agent\backup\<дата-время>`, образ пересоздан, `pending`/`base` пусты,
//!   повторный `sync` ничего не меняет;
//! * `reject` → образ равен оригиналу, агенту одно сообщение в
//!   `.hds-agent\messages.jsonl` (метки, без значений);
//! * `conflict` → `delegate` кладёт анонимную копию варианта в `_conflicts`,
//!   `new` берёт официальную версию;
//! * ошибки аргументов — кодом 2, без изменения файлов.
//!
//! Воркер — mock (`crates/hds-anonymize/tests/mock_worker.py`), поэтому тест не
//! зависит от NER-моделей и портативного Python.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use hds_agent::registry_db::{ArtifactRegistry, Status};
use hds_cli::cmd::agent::cmd_agent_cfg;
use hds_core::config::{load_from, Config};

/// Тесты трогают env (`HDS_ROOT`, `HDS_CONFIG`, пути воркеров) — гонка недопустима.
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

/// Стенд: временный каталог (он же `HDS_ROOT`), зона агента, папка проекта.
struct Stand {
    dir: common::TempDir,
    cfg: Config,
    project_dir: PathBuf,
}

impl Stand {
    fn new(tag: &str) -> Stand {
        std::env::remove_var("HDS_AGENT_SECRETS");
        std::env::remove_var("HDS_CONFIG");
        let dir = common::TempDir::new(&format!("agentresult-{tag}"));
        let project_dir = dir.join("Проект");
        fs::create_dir_all(&project_dir).unwrap();
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

    fn sync(&self) -> i32 {
        self.agent(&["sync", "--project", P, "--once"])
    }

    fn result(&self, args: &[&str]) -> i32 {
        let mut full: Vec<String> = vec!["result".to_string()];
        full.extend(args.iter().map(|a| a.to_string()));
        cmd_agent_cfg(&self.cfg, &full)
    }

    fn workspace(&self) -> PathBuf {
        self.dir.join("zone").join(P).join("workspace")
    }

    fn service(&self, sub: &str) -> PathBuf {
        self.project_dir.join(".hds-agent").join(sub)
    }

    fn registry_path(&self) -> PathBuf {
        self.dir
            .path
            .join("data")
            .join("agent")
            .join("registry")
            .join(format!("{P}.db"))
    }

    fn registry(&self) -> ArtifactRegistry {
        ArtifactRegistry::open(&self.registry_path()).unwrap()
    }

    /// Путь образа (имя с меткой) и маркер его первой строкой.
    fn image(&self, anon_rel: &str) -> PathBuf {
        self.workspace().join(anon_rel)
    }
}

/// Собрать вариант: правка образа «агентом» (маркер остаётся первой строкой).
fn make_variant(stand: &Stand, rel: &str, anon_rel: &str, text: &str) {
    let image = stand.image(anon_rel);
    let content = fs::read_to_string(&image).expect("образ выпущен");
    let marker = content.lines().next().unwrap_or("").to_string();
    assert!(
        marker.starts_with("<!-- hds-agent/"),
        "текстовый образ без маркера = «новый файл агента»: {content}"
    );
    fs::write(&image, format!("{marker}\n{text}\n")).unwrap();
    assert_eq!(stand.sync(), 0);
    assert!(
        stand.service("pending").join(rel).is_file(),
        "вариант должен лежать в pending"
    );
}

/// Ситуация 1 снаружи: accept заменяет оригинал, делает backup и пересоздаёт
/// образ; повторный проход ничего не меняет.
#[test]
fn accept_through_cli_replaces_original_and_keeps_backup() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_env) = mock_env() else {
        eprintln!("skip: нет интерпретатора воркера анонимизации");
        return;
    };
    let stand = Stand::new("accept");
    let rel = "Письмо Иванов Иван.md";
    let anon_rel = "Письмо [PERSON_1].md";
    fs::write(stand.project_dir.join(rel), "Контакт: Иванов Иван\n").unwrap();
    assert_eq!(stand.register(), 0);
    assert_eq!(stand.sync(), 0);
    make_variant(&stand, rel, anon_rel, "[PERSON_1] — правка агента");

    let code = stand.result(&["accept", "--project", P, "--path", rel]);
    assert_eq!(code, 0, "accept должен пройти");

    // Оригинал = вариант, маркер в документ пользователя не уехал.
    let original = fs::read_to_string(stand.project_dir.join(rel)).unwrap();
    assert!(
        original.contains("Иванов Иван — правка агента"),
        "{original}"
    );
    assert!(!original.contains("hds-agent"), "маркер утёк: {original}");

    // Backup: прежняя версия, каталог с датой-временем.
    let backup_root = stand.service("backup");
    let stamp_dir = fs::read_dir(&backup_root)
        .expect("каталог backup")
        .next()
        .expect("копия создана")
        .unwrap()
        .path();
    let stamp = stamp_dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(stamp.len(), 15, "YYYYMMDD-HHMMSS: {stamp}");
    assert!(!stamp.contains(':'), "{stamp}");
    let backup = stamp_dir.join(rel);
    assert_eq!(
        fs::read_to_string(&backup).unwrap(),
        "Контакт: Иванов Иван\n"
    );

    // Вариант и основание убраны, образ пересоздан из нового оригинала.
    assert!(!stand.service("pending").join(rel).exists());
    assert!(!stand.service("base").join(rel).exists());
    let image = fs::read_to_string(stand.image(anon_rel)).unwrap();
    assert!(image.contains("[PERSON_1]"), "{image}");
    assert!(image.contains("правка агента"), "{image}");

    let registry = stand.registry();
    assert_eq!(
        registry.by_orig(rel).unwrap().unwrap().status,
        Status::Accepted
    );
    assert!(registry.list_pending().unwrap().is_empty());
    drop(registry);

    // Повторный проход: образ не пересоздаётся (mtime тот же) — 0 изменений.
    let mtime = fs::metadata(stand.image(anon_rel))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(stand.sync(), 0);
    assert_eq!(
        fs::metadata(stand.image(anon_rel))
            .unwrap()
            .modified()
            .unwrap(),
        mtime
    );
}

/// Ситуация 4 снаружи: reject возвращает образ к оригиналу и оставляет агенту
/// одно сообщение (метки, без значений); повторный проход его не дублирует.
#[test]
fn reject_through_cli_restores_image_and_notifies_agent() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_env) = mock_env() else {
        eprintln!("skip: нет интерпретатора воркера анонимизации");
        return;
    };
    let stand = Stand::new("reject");
    let rel = "Письмо Иванов Иван.md";
    let anon_rel = "Письмо [PERSON_1].md";
    fs::write(stand.project_dir.join(rel), "Контакт: Иванов Иван\n").unwrap();
    assert_eq!(stand.register(), 0);
    assert_eq!(stand.sync(), 0);
    make_variant(&stand, rel, anon_rel, "[PERSON_1] — правка агента");

    assert_eq!(stand.result(&["reject", "--project", P, "--path", rel]), 0);

    // Оригинал не менялся, образ — снова официальная версия.
    assert_eq!(
        fs::read_to_string(stand.project_dir.join(rel)).unwrap(),
        "Контакт: Иванов Иван\n"
    );
    let image = fs::read_to_string(stand.image(anon_rel)).unwrap();
    assert!(image.contains("[PERSON_1]"), "{image}");
    assert!(
        !image.contains("правка агента"),
        "образ восстановлен: {image}"
    );
    assert!(!stand.service("pending").join(rel).exists());
    assert!(!stand.service("base").join(rel).exists());
    assert!(
        !stand.service("backup").exists(),
        "оригинал не заменялся — backup не нужен"
    );

    // Сообщение агенту: одна строка, метка вместо значения.
    let text =
        fs::read_to_string(stand.service("messages.jsonl")).expect("очередь сообщений агенту");
    assert_eq!(text.lines().count(), 1, "{text}");
    let line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(line["kind"], "document_rejected");
    assert_eq!(line["rel"], anon_rel);
    assert!(
        line["text"]
            .as_str()
            .unwrap()
            .contains("образ восстановлен"),
        "{text}"
    );
    assert!(!text.contains("Иванов Иван"), "значения не уходят: {text}");

    // Статус — synced, вариант не собирается, сообщений не прибавляется.
    assert_eq!(
        stand.registry().by_orig(rel).unwrap().unwrap().status,
        Status::Synced
    );
    assert_eq!(stand.sync(), 0);
    let after = fs::read_to_string(stand.service("messages.jsonl")).unwrap();
    assert_eq!(after.lines().count(), 1, "{after}");
}

/// Ситуация 2 снаружи: конфликт — `delegate` кладёт анонимную копию варианта в
/// `_conflicts` и пишет агенту «перенеси правки»; `new` берёт официальную версию.
#[test]
fn conflict_through_cli_delegates_and_takes_new_version() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(_env) = mock_env() else {
        eprintln!("skip: нет интерпретатора воркера анонимизации");
        return;
    };
    let stand = Stand::new("conflict");
    let rel = "Протокол Иванов Иван.md";
    let anon_rel = "Протокол [PERSON_1].md";
    fs::write(stand.project_dir.join(rel), "Контакт: Иванов Иван\n").unwrap();
    assert_eq!(stand.register(), 0);
    assert_eq!(stand.sync(), 0);
    make_variant(&stand, rel, anon_rel, "[PERSON_1] — правка агента");

    // Пользователь правит оригинал, пока агент работал → конфликт.
    fs::write(
        stand.project_dir.join(rel),
        "Контакт: Иванов Иван — новая версия\n",
    )
    .unwrap();
    assert_eq!(stand.sync(), 0);
    assert_eq!(
        stand.registry().by_orig(rel).unwrap().unwrap().status,
        Status::Conflict
    );
    // «Принять» теперь нельзя — только выбор при конфликте.
    assert_eq!(stand.result(&["accept", "--project", P, "--path", rel]), 1);

    let code = stand.result(&[
        "conflict",
        "--project",
        P,
        "--path",
        rel,
        "--choice",
        "delegate",
    ]);
    assert_eq!(code, 0, "delegate должен пройти");

    // Анонимная копия варианта — в анонимной зоне, рядом со образом.
    let copies: Vec<String> = fs::read_dir(stand.workspace().join("_conflicts"))
        .expect("каталог _conflicts")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(copies[0], "Протокол [PERSON_1] (вариант агента).md");
    let copy = fs::read_to_string(stand.workspace().join("_conflicts").join(&copies[0])).unwrap();
    assert!(copy.contains("[PERSON_1] — правка агента"), "{copy}");
    assert!(!copy.contains("Иванов Иван"), "копия анонимна: {copy}");

    // Образ — из новой официальной версии; вариант и основание убраны.
    let image = fs::read_to_string(stand.image(anon_rel)).unwrap();
    assert!(image.contains("новая версия"), "{image}");
    assert!(!image.contains("правка агента"), "{image}");
    assert!(!stand.service("pending").join(rel).exists());
    assert!(!stand.service("base").join(rel).exists());

    let text = fs::read_to_string(stand.service("messages.jsonl")).unwrap();
    assert!(text.contains("conflict_delegated"), "{text}");
    assert!(text.contains("_conflicts"), "{text}");
    assert!(!text.contains("Иванов Иван"), "значения не уходят: {text}");

    // Второй документ: «взять новую версию» — вариант отбрасывается.
    let rel2 = "Смета Иванов Иван.md";
    let anon2 = "Смета [PERSON_1].md";
    fs::write(stand.project_dir.join(rel2), "Сумма: 51 903,14\n").unwrap();
    assert_eq!(stand.sync(), 0);
    make_variant(&stand, rel2, anon2, "[PERSON_1] — правка агента");
    fs::write(
        stand.project_dir.join(rel2),
        "Сумма: 51 903,14 — уточнено\n",
    )
    .unwrap();
    assert_eq!(stand.sync(), 0);
    assert_eq!(
        stand.result(&[
            "conflict",
            "--project",
            P,
            "--path",
            rel2,
            "--choice",
            "new",
        ]),
        0
    );
    let image2 = fs::read_to_string(stand.image(anon2)).unwrap();
    assert!(image2.contains("уточнено"), "{image2}");
    assert!(!image2.contains("правка агента"), "{image2}");
    assert_eq!(
        fs::read_to_string(stand.project_dir.join(rel2)).unwrap(),
        "Сумма: 51 903,14 — уточнено\n",
        "оригинал не перезаписан"
    );
    assert_eq!(stand.sync(), 0);
}

/// Аргументы: без `--path`, без `--choice` и с неизвестным словом — код 2 и
/// никаких изменений в папке проекта.
#[test]
fn result_rejects_bad_arguments() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("args");
    fs::write(stand.project_dir.join("Письмо.md"), "текст\n").unwrap();
    assert_eq!(stand.register(), 0);

    assert_eq!(stand.result(&["accept", "--project", P]), 2, "нужен --path");
    assert_eq!(
        stand.result(&["conflict", "--project", P, "--path", "Письмо.md"]),
        2,
        "нужен --choice"
    );
    assert_eq!(
        stand.result(&[
            "conflict",
            "--project",
            P,
            "--path",
            "Письмо.md",
            "--choice",
            "оба",
        ]),
        2
    );
    assert_eq!(
        stand.result(&["принять", "--project", P, "--path", "Письмо.md"]),
        2
    );
    assert_eq!(
        stand.result(&["accept", "--project", "P01", "--path", "Письмо.md"]),
        2,
        "P01 — не идентификатор проекта (D21)"
    );
    assert_eq!(
        stand.result(&["result", "--project", P, "--path", "Письмо.md"]),
        2,
        "неизвестное решение"
    );
    // Ничего не создано: ни реестра, ни служебных каталогов.
    assert!(!stand.service("backup").exists());
    assert!(!stand.service("messages.jsonl").exists());
    assert_eq!(
        fs::read_to_string(stand.project_dir.join("Письмо.md")).unwrap(),
        "текст\n"
    );
}
