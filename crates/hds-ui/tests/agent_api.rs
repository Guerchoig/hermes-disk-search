//! A5.1: маршруты вкладки «Агент» — проекты, состояние зеркала, файлы реестра.
//!
//! `HDS_CONFIG`/`HDS_ROOT` — глобальные переменные процесса, поэтому тесты идут
//! под одним мьютексом, каждый со своим временным каталогом.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use hds_ui::{route, ReqHeaders};
use serde_json::Value;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmpdir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("hds-uiapi-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Стенд: временный каталог (он же `HDS_ROOT`), зона агента и `config.yaml`.
struct Stand {
    dir: PathBuf,
}

impl Stand {
    fn new(tag: &str, profile: Option<&str>) -> Stand {
        let dir = tmpdir(tag);
        let zone = dir.join("zone");
        fs::create_dir_all(&zone).unwrap();
        let mut body = format!(
            "agent:\n  enabled: true\n  root: '{}'\n\
             db_path: '{}'\n\
             embedding:\n  dim: 4\n  base_url: 'http://127.0.0.1:9/v1'\n\
             index:\n  clip: false\n",
            slash(&zone),
            slash(&dir.join("index.db")),
        );
        if let Some(profile) = profile {
            body.push_str(&format!("profile: {profile}\n"));
        }
        let cfg = dir.join("config.yaml");
        fs::write(&cfg, body).unwrap();
        std::env::set_var("HDS_CONFIG", &cfg);
        std::env::set_var("HDS_ROOT", &dir);
        Stand { dir }
    }

    fn zone(&self) -> PathBuf {
        self.dir.join("zone")
    }

    fn project_dir(&self, name: &str) -> PathBuf {
        let p = self.dir.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        std::env::remove_var("HDS_CONFIG");
        std::env::remove_var("HDS_ROOT");
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn get(path: &str, query: &str) -> (u16, Value) {
    let (status, _, body) = route("GET", path, query, &ReqHeaders::default(), "");
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

fn post(path: &str, body: &Value) -> (u16, Value) {
    let h = ReqHeaders {
        origin: Some("http://127.0.0.1:8765".into()),
        content_type: Some("application/json".into()),
        x_hds_ui: Some("1".into()),
        ..ReqHeaders::default()
    };
    let (status, _, text) = route("POST", path, "", &h, &body.to_string());
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[test]
fn agent_projects_and_state_validation() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("empty", None);

    let (status, v) = get("/api/agent/projects", "");
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["enabled"], true);
    assert_eq!(v["root"], serde_json::json!(slash(&stand.zone())));
    assert!(v["projects"].as_array().unwrap().is_empty());

    // Без параметра project — 400; неизвестный проект — 404.
    assert_eq!(get("/api/agent/state", "").0, 400);
    assert_eq!(get("/api/agent/state", "project=P0009").0, 404);
    assert_eq!(get("/api/agent/files", "").0, 400);
    assert_eq!(get("/api/agent/files", "project=P0009").0, 404);
}

#[test]
fn agent_project_add_then_state_and_files() {
    use hds_agent::registry_db::{Artifact, ArtifactRegistry, Kind, Status};

    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("add", None);
    let project = stand.project_dir("Проект");

    let (status, v) = post(
        "/api/agent/projects",
        &serde_json::json!({ "name": "Пилот", "path": slash(&project) }),
    );
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["project"]["id"], "P0001");
    assert_eq!(v["project"]["files"], 0);

    let (status, v) = get("/api/agent/projects", "");
    assert_eq!(status, 200);
    let items = v["projects"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{v}");
    assert_eq!(items[0]["name"], "Пилот");

    // Состояние: демона нет, журнала нет, файлов нет.
    let (status, v) = get("/api/agent/state", "project=P0001");
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["daemon"], false);
    assert_eq!(v["files"], 0);
    assert!(v["journal"].as_array().unwrap().is_empty());

    // Реестр появился (демон отработал) — файлы видны в таблице.
    let db = stand
        .dir
        .join("data")
        .join("agent")
        .join("registry")
        .join("P0001.db");
    let registry = ArtifactRegistry::open(&db).unwrap();
    registry
        .insert(&Artifact::new(
            "Протоколы\\Встреча.docx",
            "Протоколы\\Встреча с [PERSON_3].docx",
            Kind::Image,
            Status::Synced,
        ))
        .unwrap();
    drop(registry);
    let (status, v) = get("/api/agent/files", "project=P0001");
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["count"], 1);
    assert_eq!(v["files"][0]["orig"], "Протоколы\\Встреча.docx");
    assert_eq!(v["files"][0]["status"], "synced");

    // Повторная регистрация того же проекта — 409, плохой путь — 400.
    let (status, _) = post(
        "/api/agent/projects",
        &serde_json::json!({ "id": "P0001", "path": slash(&project) }),
    );
    assert_eq!(status, 409);
    let (status, _) = post(
        "/api/agent/projects",
        &serde_json::json!({ "path": "относительный\\путь" }),
    );
    assert_eq!(status, 400);
    let (status, _) = post(
        "/api/agent/projects",
        &serde_json::json!({ "path": slash(&stand.zone()) }),
    );
    assert_eq!(status, 400, "папка внутри agent.root — отказ");
}

#[test]
fn agent_post_requires_csrf() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _stand = Stand::new("csrf", None);
    let (status, _, _) = route(
        "POST",
        "/api/agent/projects",
        "",
        &ReqHeaders::default(),
        "{}",
    );
    assert_eq!(status, 403, "POST без X-HDS-UI/Origin отклонён");
}

#[test]
fn agent_gate_is_forbidden_in_server_profile() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _stand = Stand::new("server", Some("server"));
    let reply = hds_ui::agent::gate().expect_err("server-профиль — 403 (D20)");
    assert_eq!(reply.0, 403);
    assert!(reply.2.contains("серверном профиле"), "{}", reply.2);
}

/// Следы A6.5 у проекта `P0001`: каталог зоны, реестр (`db`/`-wal`/`-shm`),
/// общий `secrets.json` с соседним проектом, `providers` и `mcp`.
fn seed_project_traces(stand: &Stand) {
    let zone = stand.zone().join("P0001");
    for rel in [
        "workspace/a.md",
        "agent-config/keys.json",
        "agent-state/sync.log",
    ] {
        let path = zone.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "[PERSON_1]\n").unwrap();
    }
    let db = stand
        .dir
        .join("data")
        .join("agent")
        .join("registry")
        .join("P0001.db");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::write(&db, "db").unwrap();
    fs::write(PathBuf::from(format!("{}-wal", db.display())), "wal").unwrap();
    fs::write(PathBuf::from(format!("{}-shm", db.display())), "shm").unwrap();
    let secrets = stand.dir.join("data").join("agent").join("secrets.json");
    fs::write(
        &secrets,
        r#"{
          "projects": {
            "P0001": { "token": "tok-P0001-abcdef", "token_sha256": "aaa", "provider": "openrouter", "model": "m1" },
            "P0002": { "token": "tok-P0002-abcdef", "token_sha256": "bbb", "provider": "openrouter", "model": "m2" }
          },
          "providers": { "openrouter": { "api_key": "sk-or-abcdefgh" } },
          "mcp": { "tavily": { "api_key": "tvly-abcdefgh" } }
        }"#,
    )
    .unwrap();
}

/// Зарегистрировать проект `P0001` (папка проекта — `stand.project_dir`).
fn register(stand: &Stand) -> PathBuf {
    let project = stand.project_dir("Проект");
    let (status, v) = post(
        "/api/agent/projects",
        &serde_json::json!({ "name": "Пилот", "path": slash(&project) }),
    );
    assert_eq!(status, 200, "{v}");
    project
}

#[test]
fn agent_project_rm_removes_zone_registry_secret_and_record() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("rm", None);
    let project = register(&stand);
    seed_project_traces(&stand);

    let (status, v) = post(
        "/api/agent/project/rm",
        &serde_json::json!({ "project": "P0001" }),
    );
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["project"], "P0001");
    assert_eq!(v["removed"]["token"], true);
    assert_eq!(v["removed"]["record"], true);
    assert_eq!(v["removed"]["labels"], Value::Null, "метки не трогались");
    assert!(
        v["removed"]["zone"]
            .as_str()
            .unwrap_or("")
            .ends_with("P0001"),
        "{v}"
    );
    assert_eq!(v["removed"]["registry"].as_array().unwrap().len(), 3);
    assert!(v["left"].as_array().unwrap().is_empty(), "{v}");

    // Зона, реестр, запись и токен удалены; соседи и секреты-ключи целы.
    assert!(!stand.zone().join("P0001").exists());
    assert!(!stand
        .dir
        .join("data")
        .join("agent")
        .join("registry")
        .join("P0001.db")
        .exists());
    let registry =
        hds_agent::projects::load_at(&stand.dir.join("data").join("agent").join("projects.json"))
            .unwrap();
    assert!(registry.get("P0001").is_none());
    let secrets = hds_agent::secrets::Secrets::load(
        &stand.dir.join("data").join("agent").join("secrets.json"),
    )
    .unwrap();
    assert!(!secrets.has_project("P0001"));
    assert!(secrets.has_project("P0002"), "соседний проект цел");
    assert_eq!(secrets.mcp_key("tavily").as_deref(), Some("tvly-abcdefgh"));

    // Папка проекта неприкосновенна.
    assert!(project.exists());
    // Тело ответа — без токенов и ключей (I8).
    let text = v.to_string();
    assert!(!text.contains("tok-P0001"), "{text}");
    assert!(!text.contains("sk-or-"), "{text}");
}

#[test]
fn agent_project_rm_refuses_live_daemon_and_accepts_force() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("rmdaemon", None);
    register(&stand);
    seed_project_traces(&stand);
    let lock = stand
        .zone()
        .join("P0001")
        .join("agent-state")
        .join("sync.lock");
    fs::write(&lock, std::process::id().to_string()).unwrap();

    let (status, v) = post(
        "/api/agent/project/rm",
        &serde_json::json!({ "project": "P0001" }),
    );
    assert_eq!(status, 409, "{v}");
    assert!(v["error"].as_str().unwrap_or("").contains("демон зеркала"));
    assert!(stand.zone().join("P0001").exists(), "ничего не удалено");
    let registry =
        hds_agent::projects::load_at(&stand.dir.join("data").join("agent").join("projects.json"))
            .unwrap();
    assert!(registry.get("P0001").is_some());

    // force: true — удаляется, даже если lock ещё на месте.
    let (status, v) = post(
        "/api/agent/project/rm",
        &serde_json::json!({ "project": "P0001", "force": true }),
    );
    assert_eq!(status, 200, "{v}");
    assert!(!stand.zone().join("P0001").exists());
}

#[test]
fn agent_project_rm_repeat_is_404_and_bad_body_is_400() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let stand = Stand::new("rmrepeat", None);
    register(&stand);
    seed_project_traces(&stand);

    let (status, _) = post(
        "/api/agent/project/rm",
        &serde_json::json!({ "project": "P0001" }),
    );
    assert_eq!(status, 200);
    let (status, v) = post(
        "/api/agent/project/rm",
        &serde_json::json!({ "project": "P0001" }),
    );
    assert_eq!(status, 404, "повтор — 404, не 500: {v}");

    // Без project и с id не по D21 — 400.
    assert_eq!(post("/api/agent/project/rm", &serde_json::json!({})).0, 400);
    assert_eq!(
        post(
            "/api/agent/project/rm",
            &serde_json::json!({ "project": "../P0001" })
        )
        .0,
        400
    );
}

#[test]
fn agent_project_rm_requires_csrf() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _stand = Stand::new("rmcsrf", None);
    let (status, _, _) = route(
        "POST",
        "/api/agent/project/rm",
        "",
        &ReqHeaders::default(),
        r#"{"project":"P0001"}"#,
    );
    assert_eq!(status, 403, "POST без X-HDS-UI/Origin отклонён");
}

#[test]
fn agent_project_rm_is_forbidden_in_server_profile() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _stand = Stand::new("rmserver", Some("server"));
    let (status, v) = post(
        "/api/agent/project/rm",
        &serde_json::json!({ "project": "P0001" }),
    );
    assert_eq!(status, 403, "серверный профиль — 403 (D20): {v}");
}
