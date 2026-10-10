//! Общий модуль управления OS-службами (M7 T1, `PLAN_M7_UI_CONTROL.md` §3.1).
//!
//! Перенесено из `hds-cli::cmd::service` — **общая (непривилегированная) часть**,
//! которую теперь используют и CLI, и UI (`hds-ui` не зависит от `hds-cli`, а
//! `hds-cli` зависит от `hds-ui` — прямой импорт дал бы цикл):
//! * роли и имена (`ROLES`, `service_name`, `daemon_label`, `parse_role`);
//! * сборка аргументов `sc.exe` (`sc_args`) и LaunchDaemon-плейста (`daemon_plist`);
//! * чтение состояния службы (`status`: SCM `sc query`/`sc qc`, launchd `launchctl print`);
//! * управление (`control`: `install|uninstall|start|stop` через `sc.exe`/`launchctl`).
//!
//! `hds service run <role>` (SCM-диспетчер `windows-service`, спавн `llm_host.exe`)
//! **остаётся в CLI** — поэтому `windows-service` в `hds-core` не тянется:
//! управление идёт через `sc.exe`, а не через SCM-протокол.
//!
//! Тонкость (M7 §3.1): `install` строит `binPath` из пути к `hds.exe`, который
//! передаёт вызывающий. Внутри общего кода `current_exe()` указал бы на exe
//! **вызывающего** (UI), а не на `hds`, — поэтому [`sc_args`] принимает путь
//! параметром.

use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;

use serde::Serialize;

/// Поддерживаемые роли службы.
pub const ROLES: [&str; 4] = ["watch", "mcp-http", "ui", "llm-host"];

/// Имя службы Windows для роли (`hermes-disk-search-<role>`).
pub fn service_name(role: &str) -> String {
    format!("hermes-disk-search-{role}")
}

/// Метка LaunchDaemon macOS для роли (`com.hermes-disk-search.<role>`).
pub fn daemon_label(role: &str) -> String {
    format!("com.hermes-disk-search.{role}")
}

/// Разбор роли (валидация).
pub fn parse_role(s: &str) -> Option<&'static str> {
    ROLES.iter().find(|r| **r == s).copied()
}

/// Состояние роли службы — контракт `PLAN_M7_UI_CONTROL.md` §3.1/§3.2.
///
/// Поля ровно те, что отдаёт `GET /api/services`: `role`, `kind`
/// (`"service"` — установлена в SCM/launchd; `"absent"` — нет), `state`
/// (`"running"|"stopped"|"absent"|"unknown"`), `autostart`, `detail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceStatus {
    /// Роль (`watch|mcp-http|ui|llm-host`).
    pub role: String,
    /// `"service"` — установлена; `"absent"` — не найдена/не поддерживается.
    pub kind: String,
    /// `"running" | "stopped" | "absent" | "unknown"`.
    pub state: String,
    /// Автостарт (Windows — `START_TYPE AUTO_START`; macOS — плейст с `RunAtLoad`).
    pub autostart: bool,
    /// Детали (сырой фрагмент вывода/ошибки) — для карточки и диагностики.
    pub detail: String,
}

impl ServiceStatus {
    /// Роль не установлена либо платформа не поддерживает службы.
    pub fn absent(role: &str, detail: impl Into<String>) -> ServiceStatus {
        ServiceStatus {
            role: role.to_string(),
            kind: "absent".into(),
            state: "absent".into(),
            autostart: false,
            detail: detail.into(),
        }
    }
}

/// Результат управляющего действия (`install|uninstall|start|stop`): код
/// возврата и раздельные потоки. CLI печатает их как раньше (`print!`/`eprint!`
/// с сохранением порядка и exit-кода), UI берёт для `{ok, msg}` и разбора прав.
#[derive(Debug, Clone)]
pub struct ControlResult {
    /// Код возврата (как у `hds service …` до переноса).
    pub code: i32,
    /// stdout действия (уже с нашим сообщением, если оно было).
    pub stdout: String,
    /// stderr действия.
    pub stderr: String,
}

impl ControlResult {
    /// Успех (нулевой код возврата).
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// Признак «нет прав администратора» (`sc.exe`: Access denied / код 5).
    pub fn permission_denied(&self) -> bool {
        let t = format!("{}{}", self.stdout, self.stderr).to_ascii_lowercase();
        t.contains("access is denied")
            || t.contains("отказано в доступе")
            || t.contains("code 5 ")
            || t.contains("код 5 ")
    }
}

// --- Управление: сборка аргументов / запуск ----------------------------------

/// Аргументы `sc.exe` для подкоманды управления службой.
///
/// `hds_exe` — путь к `hds.exe` для `binPath` (M7 §3.1: `current_exe()` внутри
/// общего кода указал бы на exe вызывающего — UI, а не `hds`).
#[cfg(windows)]
pub fn sc_args(sub: &str, role: &str, hds_exe: &Path) -> Vec<String> {
    let name = service_name(role);
    match sub {
        "install" => vec![
            "create".into(),
            name,
            format!(
                "binPath= \"\\\"{}\\\" service run {role}\"",
                hds_exe.display()
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
#[cfg(windows)]
pub const FAILURE_ARGS: [&str; 2] = [
    "reset= 86400",
    "actions= restart/5000/restart/5000/restart/30000",
];

/// Управление службой: `install|uninstall|start|stop` (Windows — `sc.exe`,
/// macOS — `launchctl`). `hds_exe` — путь к `hds.exe` для `binPath`/плейста.
#[cfg(windows)]
pub fn control(action: &str, role: &str, hds_exe: &Path) -> ControlResult {
    let args = sc_args(action, role, hds_exe);
    if args.is_empty() {
        return ControlResult {
            code: 2,
            stdout: String::new(),
            stderr: format!("service: неизвестная подкоманда '{action}'\n"),
        };
    }
    match std::process::Command::new("sc.exe").args(&args).output() {
        Ok(o) => {
            let mut stdout = String::from_utf8_lossy(&o.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
            let mut code = o.status.code().unwrap_or(1);
            if action == "install" && code == 0 {
                // Авторестарт при падении (риск плана: служба держит exe).
                let _ = std::process::Command::new("sc.exe")
                    .arg("failure")
                    .arg(service_name(role))
                    .args(FAILURE_ARGS.iter().flat_map(|a| a.split_whitespace()))
                    .output();
                stdout.push_str(&format!(
                    "служба {} установлена (автостарт, авторестарт 5с/5с/30с)\n",
                    service_name(role)
                ));
            }
            if action == "status" {
                // sc query: 0 — установлена; 1060 — нет.
                code = if code == 0 { 0 } else { 1 };
            }
            ControlResult {
                code,
                stdout,
                stderr,
            }
        }
        Err(e) => ControlResult {
            code: 1,
            stdout: String::new(),
            stderr: format!("service: не запустить sc.exe: {e}\n"),
        },
    }
}

// --- Windows: чтение состояния SCM ------------------------------------------

/// Разбор строки `STATE` вывода `sc query`: `running|stopped|unknown`.
///
/// Чистая функция — тестируется без SCM (в т.ч. на linux-CI).
pub fn parse_sc_state(text: &str) -> String {
    for line in text.lines() {
        let l = line.trim_start();
        if l.starts_with("STATE") {
            if l.contains("RUNNING") {
                return "running".into();
            }
            if l.contains("STOPPED") {
                return "stopped".into();
            }
            return "unknown".into();
        }
    }
    "unknown".into()
}

/// Разбор строки `START_TYPE` вывода `sc qc`: `AUTO_START` (в т.ч. delayed) → true.
///
/// Чистая функция — тестируется без SCM.
pub fn parse_sc_autostart(text: &str) -> bool {
    text.lines().any(|line| {
        let l = line.trim_start();
        l.starts_with("START_TYPE") && l.contains("AUTO_START")
    })
}

/// Состояние роли в SCM (`sc query` + `sc qc`).
#[cfg(windows)]
pub fn status(role: &str) -> ServiceStatus {
    let name = service_name(role);
    let (installed, state, detail) = match std::process::Command::new("sc.exe")
        .args(["query", &name])
        .output()
    {
        Ok(o) => {
            let out = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr).into_owned();
            if o.status.success() {
                (true, parse_sc_state(&out), out.trim().to_string())
            } else {
                let code = o.status.code().unwrap_or(-1);
                let d = if err.trim().is_empty() {
                    out.trim().to_string()
                } else {
                    err.trim().to_string()
                };
                if code == 1060 || d.contains("1060") {
                    (false, "absent".into(), d)
                } else {
                    (false, "unknown".into(), d)
                }
            }
        }
        Err(e) => (false, "unknown".into(), format!("sc.exe: {e}")),
    };
    let autostart = if installed {
        std::process::Command::new("sc.exe")
            .args(["qc", &name])
            .output()
            .ok()
            .map(|o| parse_sc_autostart(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or(false)
    } else {
        false
    };
    ServiceStatus {
        role: role.to_string(),
        kind: if installed {
            "service".into()
        } else {
            "absent".into()
        },
        state,
        autostart,
        detail,
    }
}

// --- macOS: LaunchDaemon -----------------------------------------------------

/// Запустить команду и собрать её потоки/код (macOS-управление, launchctl).
#[cfg(target_os = "macos")]
fn run_cmd(program: &str, args: &[&str]) -> ControlResult {
    match std::process::Command::new(program).args(args).output() {
        Ok(o) => ControlResult {
            code: o.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => ControlResult {
            code: 1,
            stdout: String::new(),
            stderr: format!("service: {program}: {e}\n"),
        },
    }
}

/// Путь к плейсту LaunchDaemon роли.
#[cfg(target_os = "macos")]
fn plist_path(label: &str) -> PathBuf {
    PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist"))
}

/// Управление службой в macOS (`launchctl` + плейст LaunchDaemon).
#[cfg(target_os = "macos")]
pub fn control(action: &str, role: &str, hds_exe: &Path) -> ControlResult {
    let label = daemon_label(role);
    match action {
        "install" => install_macos(role, &label, hds_exe),
        "uninstall" => {
            let mut r = run_cmd("launchctl", &["bootout", &format!("system/{label}")]);
            let _ = std::fs::remove_file(plist_path(&label));
            r.stdout
                .push_str(&format!("LaunchDaemon {label} удалён (если был)\n"));
            r.code = 0;
            r
        }
        "start" => run_cmd(
            "launchctl",
            &["kickstart", "-k", &format!("system/{label}")],
        ),
        "stop" => run_cmd(
            "launchctl",
            &["kill", "SIGTERM", &format!("system/{label}")],
        ),
        "status" => run_cmd("launchctl", &["print", &format!("system/{label}")]),
        other => ControlResult {
            code: 2,
            stdout: String::new(),
            stderr: format!("service: неизвестная подкоманда '{other}'\n"),
        },
    }
}

/// Установка LaunchDaemon: плейст в `/Library/LaunchDaemons` + `launchctl bootstrap`.
#[cfg(target_os = "macos")]
fn install_macos(role: &str, label: &str, hds_exe: &Path) -> ControlResult {
    let log_dir = crate::config::system_state_dir()
        .map(|d| d.join("data").join("logs"))
        .unwrap_or_else(|| PathBuf::from("/var/log/hermes-disk-search"));
    // F10.2 (M2-ревью): каталог логов должен существовать до старта демона.
    let _ = std::fs::create_dir_all(&log_dir);
    let body = daemon_plist(
        &hds_exe.display().to_string(),
        role,
        label,
        &log_dir.display().to_string(),
    );
    let path = plist_path(label);
    if std::fs::write(&path, body).is_err() {
        return ControlResult {
            code: 1,
            stdout: String::new(),
            stderr: format!(
                "service: нет прав на {} — запустите с sudo:\n--- 8< ---\n{body}\n--- >8 ---\n",
                path.display()
            ),
        };
    }
    run_cmd(
        "launchctl",
        &["bootstrap", "system", &format!("system/{label}")],
    )
}

/// Состояние роли в launchd (`launchctl print system/<label>` + плейст).
#[cfg(target_os = "macos")]
pub fn status(role: &str) -> ServiceStatus {
    let label = daemon_label(role);
    match std::process::Command::new("launchctl")
        .args(["print", &format!("system/{label}")])
        .output()
    {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout).into_owned();
            let state = if out.contains("state = running") {
                "running"
            } else {
                "stopped"
            };
            ServiceStatus {
                role: role.to_string(),
                kind: "service".into(),
                state: state.into(),
                // Мы всегда пишем `RunAtLoad` — загруженный LaunchDaemon автозапускается.
                autostart: plist_path(&label).is_file(),
                detail: out.trim().to_string(),
            }
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).into_owned();
            let d = if err.trim().is_empty() {
                "не загружен".to_string()
            } else {
                err.trim().to_string()
            };
            ServiceStatus::absent(role, d)
        }
        Err(e) => ServiceStatus {
            role: role.to_string(),
            kind: "absent".into(),
            state: "unknown".into(),
            autostart: false,
            detail: format!("launchctl: {e}"),
        },
    }
}

/// Содержимое LaunchDaemon-плейста (RunAtLoad + KeepAlive — старт при загрузке
/// без интерактивного входа и перезапуск при падении). `log_dir` — каталог для
/// `StandardOutPath`/`StandardErrorPath` (F10.2, M2-ревью: иначе логи демона
/// теряются).
pub fn daemon_plist(exe: &str, role: &str, label: &str, log_dir: &str) -> String {
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
    <key>StandardOutPath</key><string>{log_dir}/{role}.out.log</string>
    <key>StandardErrorPath</key><string>{log_dir}/{role}.err.log</string>
</dict>
</plist>
"#
    )
}

// --- Прочие платформы (linux-CI) ---------------------------------------------

/// На прочих платформах службы не поддерживаются (Windows — SCM, macOS — launchd).
#[cfg(all(unix, not(target_os = "macos")))]
pub fn control(action: &str, _role: &str, _hds_exe: &Path) -> ControlResult {
    ControlResult {
        code: 2,
        stdout: String::new(),
        stderr: format!(
            "service {action}: на этой платформе службы не поддерживаются (Windows — SCM, macOS — LaunchDaemon)\n"
        ),
    }
}

/// На прочих платформах состояния служб нет.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn status(role: &str) -> ServiceStatus {
    ServiceStatus::absent(
        role,
        "на этой платформе службы не поддерживаются (Windows — SCM, macOS — LaunchDaemon)",
    )
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

    /// Аргументы sc.exe: install строит binPath с вложенными кавыками вокруг exe
    /// `hds.exe` (передан вызывающим — M7 §3.1, не `current_exe()`).
    #[cfg(windows)]
    #[test]
    fn sc_install_args_quote_binpath() {
        let exe = std::path::Path::new("C:\\hds\\hds.exe");
        let args = super::sc_args("install", "mcp-http", exe);
        assert_eq!(args[0], "create");
        assert_eq!(args[1], "hermes-disk-search-mcp-http");
        let bin = &args[2];
        assert!(bin.starts_with("binPath= \"\\\""), "{bin}");
        assert!(bin.contains("\" service run mcp-http\""), "{bin}");
        assert!(bin.contains("hds.exe"), "{bin}");
        assert!(args.contains(&"start= auto".to_string()));
        // M3 T0 (F1): DisplayName должен содержать реальную роль, а не литерал
        // `<role>` (дефект M2: все службы получали имя «Hermes Disk Search: <role>»).
        assert!(
            args.contains(&"DisplayName= \"Hermes Disk Search: mcp-http\"".to_string()),
            "DisplayName без подстановки роли: {args:?}"
        );
        assert!(!args.iter().any(|a| a.contains("<role>")), "{args:?}");
        for sub in ["uninstall", "start", "stop", "status"] {
            let a = super::sc_args(sub, "ui", exe);
            assert_eq!(a.last().unwrap(), "hermes-disk-search-ui");
        }
    }

    /// LaunchDaemon-плейст: Label/ProgramArguments/RunAtLoad/KeepAlive + логи.
    #[test]
    fn plist_has_required_keys() {
        let p = daemon_plist(
            "/opt/hds/bin/hds",
            "ui",
            "com.hermes-disk-search.ui",
            "/Library/Application Support/hermes-disk-search/data/logs",
        );
        for key in [
            "<key>Label</key>",
            "<key>RunAtLoad</key><true/>",
            "<key>KeepAlive</key><true/>",
            // F10.2 (M2-ревью): логи демона не теряются.
            "<key>StandardOutPath</key>",
            "<key>StandardErrorPath</key>",
        ] {
            assert!(p.contains(key), "нет {key}");
        }
        assert!(!p.contains("{ui}.out.log"), "шаблон не подставлен");
        assert!(p.contains("/ui.out.log"), "лог stdout роли: {p}");
        assert!(p.contains("/ui.err.log"), "лог stderr роли: {p}");
        assert!(p.contains("<string>/opt/hds/bin/hds</string>"));
        assert!(p.contains("<string>service</string>"));
        assert!(p.contains("<string>ui</string>"));
        assert!(ROLES.len() == 4);
    }

    /// Разбор вывода `sc query` (M7 T1): живые состояния — чистая функция,
    /// тестируется без SCM (в т.ч. на linux-CI).
    #[test]
    fn parse_sc_query_state() {
        let running = "SERVICE_NAME: hermes-disk-search-ui \r\n\
             TYPE               : 10  WIN32_OWN_PROCESS \r\n\
             STATE              : 4  RUNNING \r\n";
        assert_eq!(super::parse_sc_state(running), "running");
        let stopped = "        STATE              : 1  STOPPED \r\n";
        assert_eq!(super::parse_sc_state(stopped), "stopped");
        let pending = "        STATE              : 2  START_PENDING \r\n";
        assert_eq!(super::parse_sc_state(pending), "unknown");
        assert_eq!(super::parse_sc_state(""), "unknown");
    }

    /// Разбор `sc qc`: автостарт (в т.ч. delayed) распознаётся, demand — нет.
    #[test]
    fn parse_sc_qc_autostart() {
        assert!(super::parse_sc_autostart(
            "START_TYPE         : 2   AUTO_START\r\n"
        ));
        assert!(super::parse_sc_autostart(
            "START_TYPE         : 2   AUTO_START (DELAYED)\r\n"
        ));
        assert!(!super::parse_sc_autostart(
            "START_TYPE         : 3   DEMAND_START\r\n"
        ));
        assert!(!super::parse_sc_autostart(""));
    }
}
