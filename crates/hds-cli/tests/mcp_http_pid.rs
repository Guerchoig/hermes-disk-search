//! M2-F5 (T0 M4): `hds mcp-http stop` не должен убивать чужой процесс с
//! переиспользованным PID. Регресс-защита: если проверка личности сломается,
//! этот тест попытается убить САМ тестовый процесс и упадёт.

use hds_cli::cmd::mcp_http::cmd_mcp_http;
use std::path::PathBuf;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmpdir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("hds-mcphttp-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// PID-файл указывает на живой, но ЧУЖОЙ процесс (наш собственный — имя образа
/// не `hds.exe`): `stop` обязан не убивать его и убрать файл.
#[test]
fn stop_does_not_kill_foreign_pid() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("stop");
    let cfg = dir.join("config.yaml");
    std::fs::write(&cfg, "mcp_http:\n  port: 8797\n").unwrap();
    std::env::set_var("HDS_ROOT", &dir);
    std::env::set_var("HDS_CONFIG", &cfg);

    let pf = dir.join("data").join("mcp_http.pid");
    std::fs::create_dir_all(pf.parent().unwrap()).unwrap();
    // Живой PID (наш тестовый процесс), но заявленное имя — чужое.
    std::fs::write(&pf, format!("{}\nnot-hds.exe\n", std::process::id())).unwrap();

    let rc = cmd_mcp_http("stop", None, None, None);
    assert_eq!(rc, 0);
    assert!(!pf.exists(), "PID-файл удалён даже при чужом процессе");
    // Дошли сюда — значит чужой процесс (мы сами) не убит.
    assert!(
        std::process::id() > 0,
        "тестовый процесс жив: чужой PID не тронут"
    );

    std::env::remove_var("HDS_ROOT");
    std::env::remove_var("HDS_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Нет PID-файла → `stop` сообщает «не запущен» и не падает.
#[test]
fn stop_without_pid_file_is_ok() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmpdir("nostop");
    let cfg = dir.join("config.yaml");
    std::fs::write(&cfg, "mcp_http:\n  port: 8796\n").unwrap();
    std::env::set_var("HDS_ROOT", &dir);
    std::env::set_var("HDS_CONFIG", &cfg);

    let rc = cmd_mcp_http("stop", None, None, None);
    assert_eq!(rc, 0);

    std::env::remove_var("HDS_ROOT");
    std::env::remove_var("HDS_CONFIG");
    let _ = std::fs::remove_dir_all(&dir);
}
