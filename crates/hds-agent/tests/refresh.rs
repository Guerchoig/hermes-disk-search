//! TR-6.5: снимок vault пересобирается по `vault_rev` перед каждым запросом.
//!
//! Проверка на mock-воркере анонимизации (`crates/hds-anonymize/tests/mock_worker.py`):
//! метка, выданная уже после старта сеанса, обязана попасть в деанонимизатор —
//! иначе пользователь увидит в ответе сырой `[PERSON_2]` (AG-2 ревью A0–A3).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use hds_agent::deanon::DeAnonymizer;
use hds_agent::filter::Filter;
use hds_agent::gateway::{GatewayHost, NoAskUser, RefreshState};
use hds_agent::settings::UnknownPolicy;
use hds_agent::worker::HostHandler;
use hds_anonymize::{discover_python, Client, ClientConfig};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("корень репозитория")
        .to_path_buf()
}

#[test]
fn snapshot_is_refreshed_by_vault_rev() {
    let root = repo_root();
    let Some(python) = discover_python(&root) else {
        eprintln!("refresh: нет интерпретатора воркера анонимизации — тест пропущен");
        return;
    };
    let script = root
        .join("crates")
        .join("hds-anonymize")
        .join("tests")
        .join("mock_worker.py");
    let mut cfg = ClientConfig::new(&python, &root);
    cfg.script = Some(script);
    let client = Arc::new(Mutex::new(Client::spawn(cfg).expect("mock-воркер")));

    let snapshot = {
        let mut client = client.lock().unwrap();
        client.snapshot("P0001").expect("снимок")
    };
    assert_eq!(snapshot.rev, 7);
    let filter = Arc::new(Filter::new(
        "P0001",
        Vec::new(),
        UnknownPolicy::Label,
        Duration::from_secs(300),
        None,
    ));
    filter.set_snapshot(&snapshot, &[], &[], &[]);
    let deanon = Arc::new(Mutex::new(DeAnonymizer::new(
        &snapshot,
        &[],
        Path::new("C:\\ws"),
        Path::new("C:\\proj"),
    )));
    assert_eq!(
        deanon.lock().unwrap().text("[PERSON_2]").text,
        "[PERSON_2]",
        "до выдачи метки значения нет"
    );

    let state = Arc::new(RefreshState {
        system: Vec::new(),
        projects: Vec::new(),
        secrets: Vec::new(),
        workspace: PathBuf::from("C:\\ws"),
        project_root: PathBuf::from("C:\\proj"),
        deanon: Arc::clone(&deanon),
    });
    let host = GatewayHost::new(
        "P0001",
        "C:\\ws",
        Arc::clone(&filter),
        Arc::clone(&client),
        Arc::new(NoAskUser),
        None,
    )
    .with_refresh(state);

    // Старый снимок: известное значение заменяется меткой.
    let out = host
        .handle(
            "egress_check",
            &json!({ "kind": "model", "body": "Иванов Иван" }),
        )
        .expect("egress_check");
    assert_eq!(out["body"], json!("[PERSON_1]"));

    // Воркер выдал новую метку (как `anonymize_text` новой реплики).
    let label = client
        .lock()
        .unwrap()
        .label_for_value("P0001", "Петров Пётр", "PERSON")
        .expect("label_for_value")
        .label;
    assert_eq!(label.as_deref(), Some("[PERSON_2]"));

    // Следующий запрос видит новый `vault_rev` и пересобирает снимок.
    let out = host
        .handle(
            "egress_check",
            &json!({ "kind": "model", "body": "Петров Пётр" }),
        )
        .expect("egress_check после выдачи метки");
    assert_eq!(out["body"], json!("[PERSON_2]"));
    assert_eq!(filter.rev(), Some(8), "снимок пересобран");
    assert_eq!(
        deanon.lock().unwrap().text("[PERSON_2]").text,
        "Петров Пётр",
        "новая метка деанонимизируется без перезапуска сеанса"
    );
}
