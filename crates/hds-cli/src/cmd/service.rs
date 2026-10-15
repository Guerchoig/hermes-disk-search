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
//! * роли: `watch` | `mcp-http` | `ui` | `llm-host` | `transcribe` (llm-host — спавн и
//!   ожидание `llm_host.exe run`, чтобы SCM контролировал наш процесс; transcribe —
//!   демон автотранскрибации `hds transcribe-watch`);
//! * `install|uninstall|start|stop|status [role]` — управление: общий код
//!   `hds-core::service` (M7 T1; Windows — `sc.exe` с автостартом/авторестартом,
//!   macOS — плейст LaunchDaemon + `launchctl`); печать и exit-код без изменений.
//!
//! Десктопный автозапуск (Планировщик `AtLogOn`, `install_autostart.ps1`) не
//! трогается: службы — только server-профиль (ставит `installers/install_server.ps1`).

use std::path::PathBuf;

use hds_core::config::{ensure_state_dir, load};

/// Роли/имена служб и общий код управления — в `hds-core::service` (M7 T1,
/// `PLAN_M7_UI_CONTROL.md` §3.1): тот же код использует UI (у которого нет
/// зависимости на `hds-cli` — иначе цикл). Реэкспорт сохранён, чтобы
/// `hds-cli::cmd::service::{ROLES, service_name, …}` работали как раньше.
pub use hds_core::service::{daemon_label, daemon_plist, parse_role, service_name, ROLES};

/// `hds service <sub> [role]`; `sub` — run|install|uninstall|start|stop|status|root.
pub fn cmd_service(sub: &str, args: &[String]) -> i32 {
    let role = args.first().map(|s| s.trim());
    match sub {
        "run" => match role.and_then(parse_role) {
            Some(r) => service_run(r),
            None => usage(&format!("service run: нужна роль ({})", ROLES.join("|"))),
        },
        // `root` — не управление службой, а «где стоит поставка»: корень берётся из
        // binPath самой службы, чтобы обёртки обновления (`update.cmd`/`.command`)
        // не требовали пути руками (RELEASING.md §6).
        "root" => print_installed_root(),
        "install" | "uninstall" | "start" | "stop" | "status" => match role.and_then(parse_role) {
            Some(r) => service_control(sub, r),
            None => usage(&format!("service {sub}: нужна роль ({})", ROLES.join("|"))),
        },
        other => usage(&format!(
            "service: неизвестная подкоманда '{other}' \
             (run|install|uninstall|start|stop|status|root)"
        )),
    }
}

/// `hds service root` — корень установки зарегистрированных ролей (стандартный
/// выход, путь на stdout; `1` — служб нет: dev-запуск/ручные бинарники).
fn print_installed_root() -> i32 {
    match hds_core::service::installed_root() {
        Some(root) => {
            println!("{}", root.display());
            0
        }
        None => {
            eprintln!("service root: ни одна роль не зарегистрирована — корень неизвестен");
            eprintln!(
                "             (службы не стоят или поставка распакована вручную: укажите путь сами)"
            );
            1
        }
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
        "transcribe" => crate::cmd::transcribe::cmd_transcribe_watch(),
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
    //! SCM-обвязка: главная функция службы (`windows-service`). Управление
    //! (`sc.exe` install/uninstall/start/stop/status) — в `hds-core::service`.
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
        let role_for_handler = role.clone();
        let handler = move |control: ServiceControl| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                // Роли не умеют останавливаться кооперативно — завершаем
                // процесс (ребёнок llm-host сначала — taskkill /T); `transcribe`
                // останавливается кооперативно через `transcribe.stop` (R3.7).
                super::on_service_stop(&role_for_handler);
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
}

#[cfg(windows)]
fn current_service_role() -> String {
    role_from_args(&std::env::args().collect::<Vec<_>>())
}

/// Роль из argv (`service run <role>`) — чистая функция (F10.3, M2-ревью:
/// эвристика по `argv` теперь покрыта тестом).
#[cfg(windows)]
fn role_from_args(args: &[String]) -> String {
    match args.windows(2).rev().find(|w| w[0] == "run") {
        Some(w) => w[1].clone(),
        None => "ui".to_string(),
    }
}

#[cfg(windows)]
fn on_service_stop(role: &str) {
    use std::os::windows::process::CommandExt;
    use std::sync::atomic::Ordering;
    // R3.7 (PLAN_TRANSCRIBE_SERVICE): демон `transcribe` умеет кооперативную
    // остановку — ставим `transcribe.stop`, даём ему выйти (снять lock/index.pause)
    // и НЕ завершаем процесс принудительно сразу.
    if role == "transcribe" {
        let stop = hds_core::config::project_root().join("transcribe.stop");
        let _ = std::fs::write(&stop, b"");
        return;
    }
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

/// Не-Windows (macOS LaunchDaemon, linux): SCM-диспетчера нет — процессом
/// владеет launchd/systemd, а роль выполняется напрямую.
///
/// РЕГРЕССИЯ (13.10.2026, релизная приёмка): функция была только под
/// `#[cfg(windows)]`, а кросс-платформенный `service_run` вызывал её безусловно —
/// сборка для Linux/macOS падала с `E0425: cannot find function`. Ветка
/// `server-multiuser` до этого не прогонялась в CI (`rust.yml` следил только за
/// `main`/`w2-llm-host`).
#[cfg(not(windows))]
fn service_run_platform(role: &str) -> i32 {
    run_role(role)
}

/// Управление службой (`install|uninstall|start|stop|status`) — общий код
/// `hds-core::service` (M7 T1). Печатаем stdout/stderr и возвращаем exit-код
/// ровно как раньше (без смены поведения). `hds.exe` для `binPath` — свой
/// `current_exe()` (CLI вызывается как `hds …`).
fn service_control(sub: &str, role: &str) -> i32 {
    let exe = std::env::current_exe().unwrap_or_default();
    let r = hds_core::service::control(sub, role, &exe);
    print!("{}", r.stdout);
    eprint!("{}", r.stderr);
    r.code
}

#[cfg(test)]
mod tests {
    /// F10.3 (M2-ревью): роль из argv распознаётся (эвристика `service run <role>`).
    #[cfg(windows)]
    #[test]
    fn role_from_args_reads_service_role() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            super::role_from_args(&a(&["hds.exe", "service", "run", "mcp-http"])),
            "mcp-http"
        );
        assert_eq!(
            super::role_from_args(&a(&["hds.exe", "service", "run", "llm-host"])),
            "llm-host"
        );
        // Нет `run <role>` — дефолт `ui`.
        assert_eq!(
            super::role_from_args(&a(&["hds.exe", "service", "status"])),
            "ui"
        );
    }
}
