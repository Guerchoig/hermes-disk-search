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

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Поддерживаемые роли службы.
pub const ROLES: [&str; 5] = ["watch", "mcp-http", "ui", "llm-host", "transcribe"];

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
    ///
    /// M8 T0 (M7-1, код-ревью M7): регистр понижается **Unicode**-`to_lowercase()`, а не
    /// `to_ascii_lowercase()` — тот понижает только ASCII, поэтому локализованный отказ
    /// (`sc.exe` на рус. Windows: `[SC] OpenService: ошибка: 5:\n\nОтказано в доступе.`)
    /// не распознавался (заглавная «О» оставалась). Ложных срабатываний нет: строки ниже
    /// специфичны для отказа в доступе.
    pub fn permission_denied(&self) -> bool {
        let t = format!("{}{}", self.stdout, self.stderr).to_lowercase();
        t.contains("access is denied")
            || t.contains("отказано в доступе")
            || t.contains("code 5 ")
            || t.contains("код 5 ")
            || t.contains("failed 5")
            || t.contains("ошибка: 5")
    }
}

// --- Управление: сборка аргументов / запуск ----------------------------------

/// Простые подкоманды `sc.exe` (`delete|start|stop|query`) — по имени службы.
#[cfg(windows)]
pub fn sc_args(sub: &str, role: &str) -> Vec<String> {
    let name = service_name(role);
    match sub {
        "uninstall" => vec!["delete".into(), name],
        "start" => vec!["start".into(), name],
        "stop" => vec!["stop".into(), name],
        "status" => vec!["query".into(), name],
        _ => vec![],
    }
}

/// Опции `sc.exe create` в «классическом» синтаксисе — передаются через
/// `raw_arg`, а **не** argv.
///
/// `hds_exe` — путь к `hds.exe` для `binPath` (M7 §3.1: `current_exe()` внутри
/// общего кода указал бы на exe вызывающего — UI, а не `hds`).
///
/// Почему не argv: значение `binPath= "\"<exe>\" service run <role>"` содержит
/// вложенные кавыки и пробелы; при передаче через `Command::args` Rust
/// перекавычивает аргумент, и `sc.exe` под администратором отвергает параметры
/// (печатает usage) — живая находка T5 (до этого `install` падал на валидации).
#[cfg(windows)]
pub fn sc_install_raw(role: &str, hds_exe: &Path) -> String {
    // R3.5 (PLAN_TRANSCRIBE_SERVICE): служба `transcribe` зависит от владельца GPU
    // (`llm-host`) — иначе стартует раньше движка и задание падает.
    let depend = if role == "transcribe" {
        format!(" depend= {}", service_name("llm-host"))
    } else {
        String::new()
    };
    format!(
        " binPath= \"\\\"{}\\\" service run {role}\" start= auto DisplayName= \"Hermes Disk Search: {role}\" obj= LocalSystem{depend}",
        hds_exe.display()
    )
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
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("sc.exe");
    if action == "install" {
        // Опции `create` с вложенными кавыками — только через `raw_arg` (см. `sc_install_raw`).
        cmd.arg("create")
            .arg(service_name(role))
            .raw_arg(sc_install_raw(role, hds_exe));
    } else {
        let args = sc_args(action, role);
        if args.is_empty() {
            return ControlResult {
                code: 2,
                stdout: String::new(),
                stderr: format!("service: неизвестная подкоманда '{action}'\n"),
            };
        }
        cmd.args(&args);
    }
    match cmd.output() {
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

/// Разбор вывода `sc query`: `running|stopped|unknown`.
///
/// По **значениям** (`RUNNING`/`STOPPED`), а не по метке строки: вывод `sc`
/// локализован (`sc qc` на рус. Windows печатает `Тип_запуска`, см. ниже), поэтому
/// привязка к англ. метке ненадёжна. Чистая функция — тестируется без SCM.
pub fn parse_sc_state(text: &str) -> String {
    if text.contains("RUNNING") {
        return "running".into();
    }
    if text.contains("STOPPED") {
        return "stopped".into();
    }
    "unknown".into()
}

/// Разбор `sc qc`: автостарт — по значению `AUTO_START` (в т.ч. `AUTO_START (DELAYED)`).
///
/// Метка строки локализована (`START_TYPE` / `Тип_запуска`), значение — нет; поэтому
/// ищем значение. Чистая функция — тестируется без SCM.
pub fn parse_sc_autostart(text: &str) -> bool {
    text.contains("AUTO_START")
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
            let bootout = run_cmd("launchctl", &["bootout", &format!("system/{label}")]);
            let path = plist_path(&label);
            let had = path.exists();
            // M8 T0 (M7-4, код-ревью M7): код `launchctl bootout` больше не затирается —
            // он печатается в сообщении; успех `uninstall` определяем по факту удаления
            // плейста (идемпотентно: «демон не загружен» без плейста — это не ошибка,
            // а вот не удалить существующий плейст (нет прав) — честный отказ).
            let removed = std::fs::remove_file(&path).is_ok() || !had;
            let bootout_code = bootout.code;
            let mut r = bootout;
            r.stdout.push_str(&format!(
                "LaunchDaemon {label}: plist {} (bootout код {bootout_code})\n",
                if removed {
                    "удалён/уже отсутствовал"
                } else {
                    "НЕ удалён — нужен sudo"
                }
            ));
            r.code = if removed { 0 } else { 1 };
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
    if std::fs::write(&path, &body).is_err() {
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

// --- Корень установки по записи службы (обёртки обновления, RELEASING.md) -----
//
// Зачем: обновление серверной поставки — это подмена каталога версии за
// junction `app\current`, и запускающему апдейтер нужно знать **корень
// установки** (`<install>\app\current` → `<install>`). Спрашивать путь руками не
// надо: раскладку уже знает сама служба — её `binPath` (Windows) / первый
// `<string>` в `ProgramArguments` плейста (macOS) указывают на
// `<install>\…\bin\hds.exe`. Обёртки `update.cmd` / `update.command` берут корень
// отсюда (`bin\hds.exe service root`), поэтому парсеры — чистые функции с тестами.

/// Корень установки по записи `binPath` службы (чистая функция — есть тест).
///
/// Принимает и голое значение (`"<путь>" service run <role>`), и целую строку
/// вывода `sc qc`: путь берётся из первых кавычек, а если их нет — до `.exe`.
/// Раскладки поставки:
/// * versioned: `<install>\app\current\bin\hds.exe` → `<install>` (владелец `app\`);
/// * плоская:   `<install>\bin\hds.exe`             → `<install>`.
pub fn install_root_from_binpath(binpath: &str) -> Option<PathBuf> {
    let exe = exe_path_in(binpath)?;
    install_root_from_exe(Path::new(&exe))
}

/// Корень установки по тексту вывода `sc qc` (чистая функция — есть тест).
///
/// Метки `sc qc` локализованы (`BIN_PATH` / `ПУТЬ_К_ДВОИЧНОМУ_ФАЙЛУ`), поэтому
/// опираемся не на метку, а на то, что путь ведёт к **нашему** `hds.exe`.
pub fn install_root_from_sc_qc(text: &str) -> Option<PathBuf> {
    text.lines().find_map(|line| {
        let ours = exe_path_in(line)
            .map(|e| e.to_ascii_lowercase().ends_with("hds.exe"))
            .unwrap_or(false);
        ours.then(|| install_root_from_binpath(line)).flatten()
    })
}

/// Корень установки по тексту LaunchDaemon-плейста macOS (чистая функция — есть тест):
/// первый `<string>` внутри `ProgramArguments` — это `<install>/…/bin/hds`.
pub fn install_root_from_plist(text: &str) -> Option<PathBuf> {
    let after = text.split("ProgramArguments").nth(1)?;
    let start = after.find("<string>")? + "<string>".len();
    let rest = &after[start..];
    let end = rest.find("</string>")?;
    install_root_from_exe(Path::new(rest[..end].trim()))
}

/// Путь к exe из строки записи службы: между первыми кавычками, иначе — до `.exe`
/// (у строки `sc qc` перед путём стоит локализованная метка с `:`; последний
/// пробел перед `.exe` отделяет её от пути).
fn exe_path_in(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(i) = s.find('"') {
        let rest = &s[i + 1..];
        let j = rest.find('"')?;
        let path = rest[..j].trim();
        return (!path.is_empty()).then(|| path.to_string());
    }
    let low = s.to_ascii_lowercase();
    let end = low.find(".exe")? + ".exe".len();
    let start = low[..end].rfind([' ', '\t']).map(|p| p + 1).unwrap_or(0);
    let path = s[start..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// Общий шаг: путь к exe → корень установки (см. [`install_root_from_binpath`]).
fn install_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    if dir.as_os_str().is_empty() {
        return None;
    }
    let base = if dir.file_name().map(|n| n == "bin").unwrap_or(false) {
        dir.parent()?
    } else {
        dir
    };
    // versioned-раскладка: `<install>\app\current` → корень — владелец `app\`.
    if base.file_name().map(|n| n == "current").unwrap_or(false) {
        if let Some(app) = base.parent() {
            if app.file_name().map(|n| n == "app").unwrap_or(false) {
                if let Some(root) = app.parent().filter(|p| !p.as_os_str().is_empty()) {
                    return Some(root.to_path_buf());
                }
            }
        }
    }
    Some(base.to_path_buf())
}

/// Корень установки зарегистрированных ролей (`None` — ни одна не стоит:
/// dev-запуск из репозитория или службы ставились не из поставки).
pub fn installed_root() -> Option<PathBuf> {
    ROLES.iter().find_map(|role| installed_root_for(role))
}

/// Корень установки по SCM-записи роли (Windows: `sc qc`).
#[cfg(windows)]
fn installed_root_for(role: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("sc.exe")
        .args(["qc", &service_name(role)])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    install_root_from_sc_qc(&String::from_utf8_lossy(&out.stdout))
}

/// Корень установки по плейсту LaunchDaemon роли (`/Library/LaunchDaemons`).
#[cfg(target_os = "macos")]
fn installed_root_for(role: &str) -> Option<PathBuf> {
    let text = std::fs::read_to_string(plist_path(&daemon_label(role))).ok()?;
    install_root_from_plist(&text)
}

/// На прочих платформах служб нет — корня тоже.
#[cfg(all(unix, not(target_os = "macos")))]
fn installed_root_for(_role: &str) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::{
        daemon_label, daemon_plist, install_root_from_binpath, install_root_from_plist,
        install_root_from_sc_qc, parse_role, service_name, ControlResult, ROLES,
    };
    use std::path::PathBuf;

    /// M2 T4 / PLAN_TRANSCRIBE_SERVICE R3.1: роли — пять (добавлена `transcribe`);
    /// имена служб/лейблы однозначно выводятся.
    #[test]
    fn roles_and_names() {
        assert_eq!(parse_role("ui"), Some("ui"));
        assert_eq!(parse_role("mcp-http"), Some("mcp-http"));
        assert_eq!(parse_role("watch"), Some("watch"));
        assert_eq!(parse_role("llm-host"), Some("llm-host"));
        assert_eq!(parse_role("transcribe"), Some("transcribe"));
        assert_eq!(parse_role("nope"), None);
        assert_eq!(service_name("ui"), "hermes-disk-search-ui");
        assert_eq!(service_name("transcribe"), "hermes-disk-search-transcribe");
        assert_eq!(daemon_label("llm-host"), "com.hermes-disk-search.llm-host");
    }

    /// M8 T0 (M7-1, код-ревью M7): локализованный отказ `sc.exe` (ru-Windows) распознаётся
    /// как «нет прав», хотя сообщение начинается с заглавной кириллицы (раньше
    /// `to_ascii_lowercase()` оставлял «О» заглавной и проверка не срабатывала).
    #[test]
    fn permission_denied_matches_localized_denied() {
        // Точное сообщение ru-Windows (снято живым `sc.exe stop <служба>` без админа).
        let ru = ControlResult {
            code: 5,
            stdout: "[SC] OpenService: ошибка: 5:\r\n\r\nОтказано в доступе.\r\n".into(),
            stderr: String::new(),
        };
        assert!(
            ru.permission_denied(),
            "русский отказ должен ловиться: {ru:?}"
        );

        // Английский вариант — Unicode-регистр не сломал прежний ASCII-фолбэк.
        let en = ControlResult {
            code: 5,
            stdout: "[SC] OpenService FAILED 5:\n\nAccess is denied.".into(),
            stderr: String::new(),
        };
        assert!(
            en.permission_denied(),
            "английский отказ должен ловиться: {en:?}"
        );

        // Обычная ошибка (1060, служба не установлена) — не ложное срабатывание.
        let other = ControlResult {
            code: 1060,
            stdout: "[SC] OpenService: ошибка: 1060:\r\n\r\nУказанная служба не установлена.\r\n"
                .into(),
            stderr: String::new(),
        };
        assert!(
            !other.permission_denied(),
            "1060 — не отказ в доступе: {other:?}"
        );
    }

    /// Опции `sc.exe create` (классический raw-синтаксис, T5): binPath с вложенными
    /// кавыками вокруг exe `hds.exe` (передан вызывающим — M7 §3.1, не `current_exe()`).
    #[cfg(windows)]
    #[test]
    fn sc_install_raw_quotes_binpath() {
        let exe = std::path::Path::new("C:\\hds\\hds.exe");
        let raw = super::sc_install_raw("mcp-http", exe);
        assert!(
            raw.contains("binPath= \"\\\"C:\\hds\\hds.exe\\\" service run mcp-http\""),
            "{raw}"
        );
        assert!(raw.contains("start= auto"), "{raw}");
        // M3 T0 (F1): DisplayName должен содержать реальную роль, а не литерал `<role>`.
        assert!(
            raw.contains("DisplayName= \"Hermes Disk Search: mcp-http\""),
            "{raw}"
        );
        assert!(raw.contains("obj= LocalSystem"), "{raw}");
        assert!(!raw.contains("<role>"), "{raw}");
    }

    /// R3.5 (PLAN_TRANSCRIBE_SERVICE): служба `transcribe` объявляет зависимость от
    /// владельца GPU (`llm-host`) — иначе SCM стартует её раньше движка.
    #[cfg(windows)]
    #[test]
    fn sc_install_raw_transcribe_depends_on_llm_host() {
        let raw = super::sc_install_raw("transcribe", std::path::Path::new("C:\\hds\\hds.exe"));
        assert!(
            raw.contains("service run transcribe"),
            "binPath роли: {raw}"
        );
        assert!(
            raw.contains("depend= hermes-disk-search-llm-host"),
            "нет зависимости от llm-host: {raw}"
        );
        // Прочие роли зависимости не несут.
        let ui = super::sc_install_raw("ui", std::path::Path::new("C:\\hds\\hds.exe"));
        assert!(!ui.contains("depend="), "у ui не должно быть depend: {ui}");
    }

    /// Простые подкоманды `sc.exe` — по имени службы (T5: `create` тут больше нет).
    #[cfg(windows)]
    #[test]
    fn sc_simple_args_name() {
        for sub in ["uninstall", "start", "stop", "status"] {
            let a = super::sc_args(sub, "ui");
            assert_eq!(a.last().unwrap(), "hermes-disk-search-ui", "{sub}: {a:?}");
        }
        assert!(super::sc_args("install", "ui").is_empty());
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
        assert!(ROLES.len() == 5);
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
    ///
    /// Метки `sc qc` локализованы (`START_TYPE` англ. / `Тип_запуска` рус.),
    /// значения — нет; тест покрывает оба варианта (T5-находка).
    #[test]
    fn parse_sc_qc_autostart() {
        for auto in [
            "START_TYPE         : 2   AUTO_START\r\n",
            "START_TYPE         : 2   AUTO_START (DELAYED)\r\n",
            "        Тип_запуска          : 2   AUTO_START\r\n",
        ] {
            assert!(super::parse_sc_autostart(auto), "{auto:?}");
        }
        for demand in [
            "START_TYPE         : 3   DEMAND_START\r\n",
            "        Тип_запуска          : 3   DEMAND_START\r\n",
            "",
        ] {
            assert!(!super::parse_sc_autostart(demand), "{demand:?}");
        }
    }
    /// Обёртки обновления (`update.cmd`/`update.command`) берут корень установки
    /// из `bin\hds.exe service root`: обе раскладки поставки и мусор.
    #[test]
    fn install_root_from_binpath_layouts() {
        // versioned-поставка: <install>\app\current\bin\hds.exe
        assert_eq!(
            install_root_from_binpath(
                r#""C:\Users\Sasha\hds-server\app\current\bin\hds.exe" service run llm-host"#
            ),
            Some(PathBuf::from(r"C:\Users\Sasha\hds-server")),
            "versioned: корень — владелец app\\"
        );
        // плоская раскладка: <install>\bin\hds.exe
        assert_eq!(
            install_root_from_binpath(r#""D:\hds\bin\hds.exe" service run ui"#),
            Some(PathBuf::from(r"D:\hds"))
        );
        // macOS-подобный путь (из плейста)
        assert_eq!(
            install_root_from_binpath(r#""/opt/hds/app/current/bin/hds" service run watch"#),
            Some(PathBuf::from("/opt/hds"))
        );
        // без кавычек: путь отделяется от локализованной метки последним пробелом
        assert_eq!(
            install_root_from_binpath(
                "ПУТЬ_К_ДВОИЧНОМУ_ФАЙЛУ : C:\\hds\\bin\\hds.exe service run ui"
            ),
            Some(PathBuf::from("C:\\hds"))
        );
        for junk in ["", "   ", "BIN_PATH       : (null)", "не путь"] {
            assert_eq!(install_root_from_binpath(junk), None, "{junk:?}");
        }
    }

    /// `sc qc` фильтрует «наши» службы (путь кончается на `hds.exe`) и не
    /// опирается на локализованную метку (`BIN_PATH` / `ПУТЬ_К_ДВОИЧНОМУ_ФАЙЛУ`).
    #[test]
    fn install_root_from_sc_qc_picks_our_hds() {
        let localized = "SERVICE_NAME: hermes-disk-search-llm-host\r\n\
             \x20       TYPE                  : 10  WIN32_OWN_PROCESS\r\n\
             \x20       ПУТЬ_К_ДВОИЧНОМУ_ФАЙЛУ : \"C:\\Users\\Sasha\\hds-server\\app\\current\\bin\\hds.exe\" service run llm-host\r\n";
        assert_eq!(
            install_root_from_sc_qc(localized),
            Some(PathBuf::from(r"C:\Users\Sasha\hds-server")),
            "локализованная метка не мешает"
        );
        let foreign = "BIN_PATH       : \"C:\\other\\agent.exe\" service run llm-host\r\n";
        assert_eq!(
            install_root_from_sc_qc(foreign),
            None,
            "чужой exe — не наш корень"
        );
        assert_eq!(install_root_from_sc_qc(""), None);
    }

    /// Корень из плейста LaunchDaemon (macOS): первый `<string>` в `ProgramArguments`.
    #[test]
    fn install_root_from_plist_macos() {
        let p = daemon_plist(
            "/opt/hds/app/current/bin/hds",
            "ui",
            "com.hermes-disk-search.ui",
            "/Library/Application Support/hermes-disk-search/data/logs",
        );
        assert_eq!(
            install_root_from_plist(&p),
            Some(PathBuf::from("/opt/hds")),
            "плейст указывает на <install>/app/current/bin/hds"
        );
        assert_eq!(install_root_from_plist("no plist here"), None);
    }
}
