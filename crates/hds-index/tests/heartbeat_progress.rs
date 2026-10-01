//! Тесты `heartbeat`/`progress` — R30 (`PLAN_W2_LLM_HOST.md` §5/B4): различать
//! живой прогон, паузную и зависшую сессию по времени последнего прогресса,
//! а также состав полей `heartbeat_data` (их читают UI и MCP).

use std::path::PathBuf;

use hds_index::heartbeat::{index_running, session_state, HeartbeatFile, SessionState};
use hds_index::progress::ProgressReporter;
use serde_json::json;

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("hds-hb-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn session_state_distinguishes_live_paused_stale() {
    let dir = tmpdir("state");
    let hb = HeartbeatFile::new(&dir);

    // нет файла
    assert_eq!(session_state(&hb, 30.0), SessionState::None);
    assert!(!index_running(&hb, 30.0));

    // свежий без паузы → Live
    hb.write(&json!({"seen": 5, "paused": false}));
    assert_eq!(session_state(&hb, 30.0), SessionState::Live);
    assert!(index_running(&hb, 30.0));
    assert!(session_state(&hb, 30.0).blocks_new_run());

    // свежий с паузой → Paused (новый прогон НЕ блокируется — R30)
    hb.write(&json!({"seen": 5, "paused": true}));
    assert_eq!(session_state(&hb, 30.0), SessionState::Paused);
    assert!(!index_running(&hb, 30.0), "паузная сессия не блокирует новый прогон");

    // старый `ts` (внутри файла) → Stale, даже если mtime свежий
    // (пишем файл напрямую: `write()` всегда ставит свежий `ts`, как Python)
    std::fs::write(hb.path(), r#"{"seen": 5, "paused": false, "ts": 1.0}"#).unwrap();
    assert_eq!(session_state(&hb, 30.0), SessionState::Stale);
    assert!(!index_running(&hb, 30.0), "зависшая сессия не блокирует новый прогон");

    hb.remove();
    assert_eq!(session_state(&hb, 30.0), SessionState::None);
}

#[test]
fn heartbeat_data_has_ui_mcp_fields() {
    let rep = ProgressReporter::new(0);
    rep.seen();
    rep.seen();
    rep.set_total(10);
    rep.set_last_path("D:/x/a.txt");
    rep.processed("indexed(3 чанков)", Some("text"), 0.5, 3);
    rep.set_current("D:/x/b.pdf", "PDF → текст/OCR");
    rep.set_progress(42.0);
    let d = rep.heartbeat_data();

    for key in [
        "seen",
        "processed",
        "errors",
        "chunks",
        "paused",
        "total",
        "elapsed",
        "rate_min",
        "rate_window",
        "events",
        "path",
        "phase",
        "progress",
    ] {
        assert!(d.get(key).is_some(), "нет поля {key} в heartbeat_data: {d}");
    }
    assert_eq!(d["seen"], json!(2));
    assert_eq!(d["processed"], json!(1));
    assert_eq!(d["chunks"], json!(3));
    assert_eq!(d["path"], json!("D:/x/b.pdf"));
    assert_eq!(d["progress"], json!(42.0));
    // events: последнее обработанное с лемматизированным статусом-ключом
    assert_eq!(d["events"][0]["status"], json!("indexed"));
    assert_eq!(d["events"][0]["path"], json!("D:/x/a.txt"));
    assert_eq!(d["events"][0]["chunks"], json!(3));
}

#[test]
fn eta_absent_without_total() {
    let rep = ProgressReporter::new(0);
    rep.seen();
    assert!(rep.eta_sec().is_none());
    let d = rep.heartbeat_data();
    assert!(d.get("eta_sec").is_none(), "eta_sec убирается при отсутствии ETA");
    assert!(d.get("remaining").is_none());

    rep.set_total(5);
    let d = rep.heartbeat_data();
    assert!(d.get("eta_sec").is_some());
    assert_eq!(d["remaining"], json!(4));
}

#[test]
fn counters_accumulate_like_python() {
    let rep = ProgressReporter::new(0);
    rep.processed("unchanged", Some("text"), 0.1, 0);
    rep.processed("skipped_big", Some("text"), 0.1, 0);
    rep.processed("error: boom", Some("docx"), 0.1, 0);
    let d = rep.heartbeat_data();
    assert_eq!(d["processed"], json!(2)); // unchanged + skipped_big
    assert_eq!(d["errors"], json!(1)); // error
}