//! `hds service` — службы Windows (SCM) и LaunchDaemon-хостинг macOS (M2 T4,
//! `PLAN_M2_SERVER_PROFILE.md` §3.4).
//!
//! Единая точка входа сервиса: **`hds service run <role>`** — процесс, который
//! ставит служба (SCM `binPath` или LaunchDaemon `ProgramArguments`):
//! * Windows: при старте под SCM — `service_dispatcher` (`windows-service`):
//!   обработчик Stop завершает процесс (роли — блокирующие циклы без канала
//!   остановки; для `llm-host` сперва убивается ребёнок `llm_host.exe`);
//!   при запуске вручную диспетчер SCM не срабатывает — роль выполняется
//!   напрямую (тот же путь, что и на macOS);
//! * роли: `watch` | `mcp-http` | `ui` | `llm-host` (последний — спавн и
//!   ожидание `llm_host.exe run`, чтобы SCM контролировал наш процесс);
//! * `install|uninstall|start|stop|status [role]` — управление: Windows —
//!   `sc.exe` (автостарт + авторестарт при падении), macOS — плейст
//!   LaunchDaemon + `launchctl`.
//!
//! Десктопный автозапуск (Планировщик `AtLogOn`, `install_autostart.ps1`) не
//! трогается: службы — только server-профиль (ставит `installers/install_server.ps1`).

use std::path::PathBuf;

use hds_core::config::{ensure_state_dir, load};

/// Поддерживаемые роли службы.
pub const ROLES: [&str; 4] = ["watch", "mcp-http", "ui", "llm-host"];

/// Имя службы Windows для роли (`hermes-disk-search-<role>`).
pub fn service_name(role: &str) -> String {
    format!("hermes-disk-search-{role}")
}

/// Метка LaunchDaemon macOS для роли.
pub fn daemon_label(role: &str) -> String {
    format!("com.hermes-disk-search.{role}")
}

/// Разбор роли (валидация для CLI).
pub fn parse_role(s: &str) -> Option<&'static str> {
    ROLES.iter().find(|r| **r == s).copied()
}

/// `hds service <sub> [role]`; `sub` — run|install|uninstall|start|stop|status.
pub fn cmd_service(sub: &str, args: &[String]) -> i32 {
    let role = args.first().map(|s| s.trim());
    match sub {
        "run" => match role.and_then(parse_role) {
            Some(r) => service_run(r),
            None => usage(&format!("service run: нужна роль ({})", ROLES.join("|"))),
        },
        "install" | "uninstall" | "start" | "stop" | "status" => match role.and_then(parse_role) {
            Some(r) => service_control(sub, r),
            None => usage(&format!("service {sub}: нужна роль ({})", ROLES.join("|"))),
        },
        other => usage(&format!(
            "service: неизвестная подкоманда '{other}' (run|install|uninstall|start|stop|status)"
        )),
    }
}

fn usage(msg: &str) -> i32 {
    eprintln!("{msg}");
    2
}

/// Выполнить роль (блокирующе). Общий код для SCM-потока, LaunchDaemon и
/// прямого запуска.
fn run_role_blocking(role: &str) -> i32 {
    match role {
        "watch" => crate::cmd::watch::cmd_watch(None),
        "mcp-http" => crate::cmd::mcp_http::cmd_mcp_http("run", None, None, None),
        "ui" => crate::cmd::ui::cmd_ui(None, None),
        "llm-host" => run_llm_host(),
        _ => {
            eprintln!("service: неизвестная роль '{role}'");
            2
        }
    }
}

/// `service run`: инициализация каталога состояния (M2 T2) и запуск роли.
///
/// Windows: под SCM роль обслуживает диспетчер (`windows-service`); не под SCM
/// (вручную, LaunchDaemon, linux) — прямой запуск.
pub fn service_run(role: &str) -> i32 {
    match load() {
        Ok(cfg) => {
            if let Err(e) = ensure_state_dir(&cfg) {
                eprintln!("service: каталог состояния: {e}");
            }
        }
        Err(e) => eprintln!(
            "service: конфиг не читается ({}) — роль всё равно стартует",
            e.message()
        ),
    }
    service_run_platform(role)
}

/// Прямой запуск роли — общий для всех платформ.
fn run_role(role: &str) -> i32 {
    let code = run_role_blocking(role);
    if code != 0 {
        eprintln!("service: роль '{role}' завершилась с кодом {code}");
    }
    code
}

/// `llm-host`: бинарь-резидент рядом с `hds.exe` (`bin\llm_host.exe` или рядом),
/// режим `run` — ждём его завершения (SCM/LaunchDaemon контролирует наш процесс).
fn llm_host_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join("llm_host.exe"), dir.join("llm_host")];
    if let Some(parent) = dir.parent() {
        candidates.push(parent.join("bin").join("llm_host.exe"));
        candidates.push(parent.join("bin").join("llm_host"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// PID ребёнка `llm_host.exe` (для остановки службы обработчиком Stop).
#[cfg(windows)]
static LLM_CHILD_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn run_llm_host() -> i32 {
    let exe = match llm_host_exe() {
        Some(p) => p,
        None => {
            eprintln!(
                "service: llm_host(.exe) не найден рядом с hds (bin\\llm_host.exe или рядом)"
            );
            return 1;
        }
    };
    eprintln!("service: llm-host — запускаю {}", exe.display());
    let mut child = match std::process::Command::new(&exe).arg("run").spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("service: не запустить {}: {e}", exe.display());
            return 1;
        }
    };
    #[cfg(windows)]
    {
        use std::sync::atomic::Ordering;
        LLM_CHILD_PID.store(child.id(), Ordering::SeqCst);
    }
    match child.wait() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("service: ожидание llm_host: {e}");
            1
        }
    }
}

// --- Windows: SCM + sc.exe ---

#[cfg(windows)]
mod scm {
    //! SCM-обвязка: главная функция службы (`windows-service`) и управление
    //! через `sc.exe` (install/uninstall/start/stop/status).
    //!
    //! Остановка: роли — блокирующие циклы без канала остановки, поэтому
    //! обработчик Stop завершает процесс (SCM фиксирует остановку по смерти
    //! процесса); для `llm-host` сперва убивается ребёнок (`taskkill /T`).

    use std::time::Duration;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    windows_service::define_windows_service!(ffi_service_main, service_main);

    /// Запуск SCM-диспетчера (ffi_service_main виден только внутри модуля).
    pub fn start_dispatcher(name: &str) -> Result<(), windows_service::Error> {
        windows_service::service_dispatcher::start(name, ffi_service_main)
    }

    /// Главная функция службы (вызывается SCM; роль берём из своего argv).
    fn service_main(_args: Vec<std::ffi::OsString>) {
        let role = super::current_service_role();
        let name = super::service_name(&role);
        let handler = |control: ServiceControl| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                // Роли не умеют останавливаться кооперативно — завершаем
                // процесс (ребёнок llm-host сначала — taskkill /T).
                super::on_service_stop();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status_handle = match service_control_handler::register(&name, handler) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("service: SCM register: {e}");
                return;
            }
        };
        let set = |state: ServiceState, accept: ServiceControlAccept| {
            let _ = status_handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(30),
                process_id: None,
            });
        };
        set(ServiceState::Running, ServiceControlAccept::STOP);

        // Роль — в отдельном потоке; SCM-поток ждёт её падения (обычно роль
        // живёт вечно, а остановку делает обработчик Stop через exit).
        let role_owned = role.to_string();
        let role_thread = std::thread::spawn(move || super::run_role(&role_owned));
        let _ = role_thread.join();

        set(ServiceState::Stopped, ServiceControlAccept::empty());
    }

    /// Аргументы `sc.exe` для подкоманды управления службой.
    pub fn sc_args(sub: &str, role: &str) -> Vec<String> {
        let name = super::service_name(role);
        match sub {
            "install" => vec![
                "create".into(),
                name,
                format!(
                    "binPath= \"\\\"{}\\\" service run {role}\"",
                    std::env::current_exe()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                ),
                "start= auto".into(),
                format!("DisplayName= \"Hermes Disk Search: {role}\""),
                "obj= LocalSystem".into(),
            ],
            "uninstall" => vec!["delete".into(), name],
            "start" => vec!["start".into(), name],
            "stop" => vec!["stop".into(), name],
            "status" => vec!["query".into(), name],
            _ => vec![],
        }
    }

    /// Авторестарт службы при падении (5 с, 5 с, 30 с; сброс счётчика за сутки).
    pub const FAILURE_ARGS: [&str; 2] = [
        "reset= 86400",
        "actions= restart/5000/restart/5000/restart/30000",
    ];
}

#[cfg(windows)]
fn current_service_role() -> String {
    // Роль — аргумент команды `service run <role>` в собственном argv.
    let args: Vec<String> = std::env::args().collect();
    match args.windows(2).rev().find(|w| w[0] == "run") {
        Some(w) => w[1].clone(),
        None => "ui".to_string(),
    }
}

#[cfg(windows)]
fn on_service_stop() {
    use std::os::windows::process::CommandExt;
    use std::sync::atomic::Ordering;
    let pid = LLM_CHILD_PID.load(Ordering::SeqCst);
    if pid != 0 {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status();
    }
    std::process::exit(0);
}

#[cfg(windows)]
fn service_run_platform(role: &str) -> i32 {
    let name = service_name(role);
    // Под SCM диспетчер владеет процессом и возвращается при завершении
    // службы; не под SCM — ошибка → прямой запуск (вручную/Планировщик).
    match scm::start_dispatcher(&name) {
        Ok(()) => 0,
        Err(_) => run_role(role),
    }
}

#[cfg(windows)]
fn service_control(sub: &str, role: &str) -> i32 {
    let args = scm::sc_args(sub, role);
    let out = std::process::Command::new("sc.exe").args(&args).output();
    match out {
        Ok(o) => {
            print!("{}", String::from_utf8_lossy(&o.stdout));
            eprint!("{}", String::from_utf8_lossy(&o.stderr));
            let code = o.status.code().unwrap_or(1);
            if sub == "install" && code == 0 {
                // Авторестарт при падении (риск плана: служба держит exe).
                let _ = std::process::Command::new("sc.exe")
                    .arg("failure")
                    .arg(service_name(role))
                    .args(scm::FAILURE_ARGS.iter().flat_map(|a| a.split_whitespace()))
                    .output();
                println!(
                    "служба {} установлена (автостарт, авторестарт 5с/5с/30с)",
                    service_name(role)
                );
            }
            if sub == "status" {
                // sc query: 0 — установлена; 1060 — нет.
                return if code == 0 { 0 } else { 1 };
            }
            code
        }
        Err(e) => {
            eprintln!("service: не запустить sc.exe: {e}");
            1
        }
    }
}

// --- macOS: LaunchDaemon ---

#[cfg(target_os = "macos")]
fn service_control(sub: &str, role: &str) -> i32 {
    // Плейст /Library/LaunchDaemons/<label>.plist (root), RunAtLoad + KeepAlive;
    // управление — launchctl bootstrap/bootout/kickstart/kill/print.
    let label = daemon_label(role);
    match sub {
        "install" => service_install_macos(role, &label),
        "uninstall" => {
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", &format!("system/{label}")])
                .status();
            let _ = std::fs::remove_file(plist_path(&label));
            println!("LaunchDaemon {label} удалён (если был)");
            0
        }
        "start" => launchctl(["kickstart", "-k", &format!("system/{label}")]),
        "stop" => launchctl(["kill", "SIGTERM", &format!("system/{label}")]),
        "status" => launchctl(["print", &format!("system/{label}")]),
        _ => 2,
    }
}

#[cfg(target_os = "macos")]
fn launchctl(args: [&str; 3]) -> i32 {
    match std::process::Command::new("launchctl").args(args).status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("service: launchctl: {e}");
            1
        }
    }
}

#[cfg(target_os = "macos")]
fn plist_path(label: &str) -> std::path::PathBuf {
    std::path::PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist"))
}

#[cfg(target_os = "macos")]
fn service_install_macos(role: &str, label: &str) -> i32 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("service: не определить свой exe: {e}");
            return 1;
        }
    };
    let body = daemon_plist(&exe.display().to_string(), role, &label);
    let path = plist_path(label);
    if std::fs::write(&path, body).is_err() {
        eprintln!(
            "service: нет прав на {} — запустите с sudo:\n--- 8< ---\n{body}\n--- >8 ---",
            path.display()
        );
        return 1;
    }
    launchctl(["bootstrap", "system", &format!("system/{label}")])
}

/// Содержимое LaunchDaemon-плейста (RunAtLoad + KeepAlive — старт при загрузке
/// без интерактивного входа и перезапуск при падении).
pub fn daemon_plist(exe: &str, role: &str, label: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>service</string>
        <string>run</string>
        <string>{role}</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
</dict>
</plist>
"#
    )
}

// --- Прочие платформы (linux-CI) ---

#[cfg(all(unix, not(target_os = "macos")))]
fn service_control(sub: &str, _role: &str) -> i32 {
    eprintln!(
        "service {sub}: на этой платформе службы не поддерживаются (Windows — SCM, macOS — LaunchDaemon)"
    );
    2
}

#[cfg(test)]
mod tests {
    use super::{daemon_label, daemon_plist, parse_role, service_name, ROLES};

    /// M2 T4: роли — ровно четыре; имена служб/лейблы однозначно выводятся.
    #[test]
    fn roles_and_names() {
        assert_eq!(parse_role("ui"), Some("ui"));
        assert_eq!(parse_role("mcp-http"), Some("mcp-http"));
        assert_eq!(parse_role("watch"), Some("watch"));
        assert_eq!(parse_role("llm-host"), Some("llm-host"));
        assert_eq!(parse_role("nope"), None);
        assert_eq!(service_name("ui"), "hermes-disk-search-ui");
        assert_eq!(daemon_label("llm-host"), "com.hermes-disk-search.llm-host");
    }

    /// Аргументы sc.exe: install строит binPath с вложенными кавыками вокруг exe.
    #[cfg(windows)]
    #[test]
    fn sc_install_args_quote_binpath() {
        let args = super::scm::sc_args("install", "mcp-http");
        assert_eq!(args[0], "create");
        assert_eq!(args[1], "hermes-disk-search-mcp-http");
        let bin = &args[2];
        assert!(bin.starts_with("binPath= \"\\\""), "{bin}");
        assert!(bin.contains("\" service run mcp-http\""), "{bin}");
        assert!(args.contains(&"start= auto".to_string()));
        // M3 T0 (F1): DisplayName должен содержать реальную роль, а не литерал
        // `<role>` (дефект M2: все службы получали имя «Hermes Disk Search: <role>»).
        assert!(
            args.contains(&"DisplayName= \"Hermes Disk Search: mcp-http\"".to_string()),
            "DisplayName без подстановки роли: {args:?}"
        );
        assert!(!args.iter().any(|a| a.contains("<role>")), "{args:?}");
        for sub in ["uninstall", "start", "stop", "status"] {
            let a = super::scm::sc_args(sub, "ui");
            assert_eq!(a.last().unwrap(), "hermes-disk-search-ui");
        }
    }

    /// LaunchDaemon-плейст: Label/ProgramArguments/RunAtLoad/KeepAlive.
    #[test]
    fn plist_has_required_keys() {
        let p = daemon_plist("/opt/hds/bin/hds", "ui", "com.hermes-disk-search.ui");
        for key in [
            "<key>Label</key>",
            "<key>RunAtLoad</key><true/>",
            "<key>KeepAlive</key><true/>",
        ] {
            assert!(p.contains(key), "нет {key}");
        }
        assert!(p.contains("<string>/opt/hds/bin/hds</string>"));
        assert!(p.contains("<string>service</string>"));
        assert!(p.contains("<string>ui</string>"));
        assert!(ROLES.len() == 4);
    }
}
