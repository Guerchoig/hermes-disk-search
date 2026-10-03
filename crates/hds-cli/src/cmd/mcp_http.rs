//! `hds mcp-http` — менеджер streamable-http MCP-сервера (порт `hds/mcp_http.py`):
//! ОДИН инстанс на машину. `check | start | stop | status | restart | run`.
//!
//! Опознание «наш» инстанс — через `GET /health` (`{"app":"disk-search"}`); чужой
//! сервис на порту не переиспользуем (STATE_FOREIGN). PID — `data/mcp_http.pid`.

use std::path::PathBuf;
use std::time::Duration;

use hds_core::config::{dig, load, project_root, Config};
use hds_core::http;
use serde_json::{json, Value};

const STATE_MCP: &str = "mcp";
const STATE_FOREIGN: &str = "foreign";
const STATE_DOWN: &str = "down";

/// `(host, port, path, start_timeout)` из секции `mcp_http`.
fn settings(cfg: &Config) -> (String, u16, String, u64) {
    let host = dig(cfg, "mcp_http.host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1")
        .to_string();
    let port = dig(cfg, "mcp_http.port")
        .and_then(|v| v.as_u64())
        .unwrap_or(8787) as u16;
    let mut path = dig(cfg, "mcp_http.path")
        .and_then(|v| v.as_str())
        .unwrap_or("/mcp")
        .trim()
        .to_string();
    if path.is_empty() {
        path = "/mcp".to_string();
    }
    if !path.starts_with('/') {
        path = format!("/{path}");
    }
    let timeout = dig(cfg, "mcp_http.start_timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(30);
    (host, port, path, timeout)
}

fn url(host: &str, port: u16, path: &str) -> String {
    format!("http://{host}:{port}{path}")
}

/// Проба `/health`: `(state, info)`.
fn probe(host: &str, port: u16) -> (String, Value) {
    match http::request(
        host,
        port,
        "GET",
        "/health",
        &[("Accept", "application/json")],
        None,
        Duration::from_secs(3),
    ) {
        Ok(resp) if (200..300).contains(&resp.status) => match resp.json() {
            Ok(v) if v.get("app").and_then(|a| a.as_str()) == Some(hds_core::config::APP_NAME) => {
                (STATE_MCP.to_string(), v)
            }
            Ok(v) => (STATE_FOREIGN.to_string(), v),
            Err(_) => (STATE_FOREIGN.to_string(), json!({})),
        },
        Ok(resp) => (STATE_FOREIGN.to_string(), json!({ "status": resp.status })),
        Err(_) => (STATE_DOWN.to_string(), json!({})),
    }
}

fn pid_file() -> PathBuf {
    project_root().join("data").join("mcp_http.pid")
}

fn read_pid() -> Option<u32> {
    std::fs::read_to_string(pid_file())
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn write_pid(pid: u32) {
    let p = pid_file();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, pid.to_string());
}

fn remove_pid_file() {
    let _ = std::fs::remove_file(pid_file());
}

/// Жив ли процесс (Windows — `tasklist`, иначе `kill -0`).
fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .creation_flags(0x0800_0000)
            .output();
        out.map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
}

/// Запуск detached-инстанса (`<current_exe> mcp --http …`).
fn spawn_instance(host: &str, port: u16, path: &str) -> Result<u32, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "mcp",
        "--http",
        "--host",
        host,
        "--port",
        &port.to_string(),
        "--path",
        path,
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
        // Снять наследование std-хендлов на время spawn: иначе detached-сервер
        // унаследует write-конец пайпа вызывающего, и ЛЮБОЙ захват вывода
        // (`$x = hds mcp-http restart`, `| Out-String`, `$(...)`, Start-Job) не
        // завершится, пока жив сервер (зависание). После spawn флаг возвращаем.
        unsafe { set_std_inherit(false) };
        let res = cmd
            .spawn()
            .map(|c| c.id())
            .map_err(|e| format!("spawn: {e}"));
        unsafe { set_std_inherit(true) };
        res
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        let child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
        Ok(child.id())
    }
}

/// Включить/снять `HANDLE_FLAG_INHERIT` на стандартных хендлах процесса
/// (`GetStdHandle` + `SetHandleInformation`; свои extern-объявления — как в других
/// модулях проекта, без новых крейтов).
#[cfg(windows)]
unsafe fn set_std_inherit(on: bool) {
    use std::os::raw::c_void;
    extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> *mut c_void;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    for n in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let h = GetStdHandle(n);
        if !h.is_null() && (h as isize) != -1 {
            let flags = if on { HANDLE_FLAG_INHERIT } else { 0 };
            let _ = SetHandleInformation(h, HANDLE_FLAG_INHERIT, flags);
        }
    }
}

/// Дождаться `STATE_MCP` до `timeout` секунд.
fn wait_up(host: &str, port: u16, timeout: u64) -> bool {
    for _ in 0..(timeout * 4) {
        if probe(host, port).0 == STATE_MCP {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// `cmd_mcp_http(sub, --host, --port, --path)`.
pub fn cmd_mcp_http(
    sub: &str,
    host_o: Option<String>,
    port_o: Option<u16>,
    path_o: Option<String>,
) -> i32 {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let (mut host, mut port, mut path, timeout) = settings(&cfg);
    if let Some(h) = host_o {
        host = h;
    }
    if let Some(p) = port_o {
        port = p;
    }
    if let Some(p) = path_o {
        path = p;
    }
    let u = url(&host, port, &path);
    match sub {
        "check" => {
            let (state, _) = probe(&host, port);
            println!("{u}: state={state}");
            if state == STATE_MCP {
                0
            } else {
                1
            }
        }
        "status" => {
            let (state, info) = probe(&host, port);
            let pid = read_pid().filter(|p| pid_alive(*p));
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "state": state, "url": u, "pid": pid,
                    "version": info.get("version").cloned().unwrap_or(Value::Null),
                }))
                .unwrap_or_default()
            );
            0
        }
        "stop" => {
            if let Some(pid) = read_pid() {
                kill_pid(pid);
                remove_pid_file();
                println!("остановлен (pid {pid})");
            } else {
                println!("не запущен (PID-файла нет)");
            }
            0
        }
        "run" => match hds_mcp::run_http(&host, port, &path) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("{e}");
                1
            }
        },
        "start" | "restart" => {
            if sub == "restart" {
                if let Some(pid) = read_pid() {
                    kill_pid(pid);
                    remove_pid_file();
                }
                for _ in 0..40 {
                    if probe(&host, port).0 == STATE_DOWN {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            } else {
                let (state, _) = probe(&host, port);
                if state == STATE_MCP {
                    println!("уже работает: {u} (переиспользую)");
                    return 0;
                }
                if state == STATE_FOREIGN {
                    eprintln!("порт занят чужим сервисом: {u}");
                    return 1;
                }
            }
            match spawn_instance(&host, port, &path) {
                Ok(pid) => {
                    write_pid(pid);
                    if wait_up(&host, port, timeout) {
                        let (_, info) = probe(&host, port);
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({
                                "state": STATE_MCP, "url": u, "pid": pid,
                                "version": info.get("version").cloned().unwrap_or(Value::Null),
                            }))
                            .unwrap_or_default()
                        );
                        0
                    } else {
                        eprintln!("инстанс не поднялся за {timeout} с: {u}");
                        1
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        other => {
            eprintln!(
                "mcp-http: неизвестная подкоманда '{other}' (check|start|stop|status|restart|run)"
            );
            2
        }
    }
}
