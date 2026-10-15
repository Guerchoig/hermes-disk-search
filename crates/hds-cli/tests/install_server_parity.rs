//! M8 T1/T2: unattended-параметры серверных установщиков (Windows/macOS) — паритет.
//!
//! Статический контракт (`PLAN_M8_INSTALL_PACKAGES.md` §3): `install_server.ps1` и
//! `install_server.command` экспонируют один и тот же unattended-набор и генерируют
//! server-конфиг. Тест ловит рассинхрон/потерю параметров между сборками.

use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("прочитать {}: {e}", p.display()))
}

/// Ожидаемый unattended-набор: (PowerShell-параметр, bash-флаг).
const PARAMS: &[(&str, &str)] = &[
    ("StateDir", "--statedir"),
    ("McpHost", "--mcp-host"),
    ("McpPort", "--mcp-port"),
    ("UiHost", "--ui-host"),
    ("UiPort", "--ui-port"),
    ("Parallel", "--parallel"),
    ("GpuDevice", "--gpu-device"),
    ("GpuPreset", "--gpu-preset"),
    ("Roots", "--roots"),
    ("EnableAccess", "--enable-access"),
    ("AdminLogin", "--admin-login"),
    ("AdminPassword", "--admin-password"),
    ("Fetch", "--fetch"),
    ("NoFetch", "--no-fetch"),
    ("SkipDeps", "--skip-deps"),
    ("NoConfigPin", "--no-config-pin"),
    ("WhatIf", "--whatif"),
];

/// Маркеры сгенерированного server-конфига и сопутствующих шагов (в обоих установщиках).
const CONFIG_NEEDLES: &[&str] = &[
    "profile: server",
    "enabled: true",
    "llm_server:",
    "  parallel:",
    "device_index:",
    "roots:",
    "fetch_engine_runtime.ps1",
    "token new --user",
];

/// `install_server.ps1`: unattended-параметры, server-конфиг, fetch-хуки, хинт токена.
#[test]
fn install_server_ps1_has_unattended_params() {
    let s = read("installers/install_server.ps1");
    for (ps, _) in PARAMS {
        assert!(
            s.contains(&format!("${ps}")),
            "в install_server.ps1 нет параметра ${ps}"
        );
    }
    for needle in CONFIG_NEEDLES {
        assert!(s.contains(needle), "в install_server.ps1 нет '{needle}'");
    }
    // Обратная совместимость: прежние параметры M2 на месте.
    for p in ["Root", "Roles", "KeepAtLogOn"] {
        assert!(
            s.contains(&format!("${p}")),
            "прежний параметр ${p} удалён из install_server.ps1"
        );
    }
}

/// `install_server.command` (macOS): тот же unattended-набор (bash-флаги) + конфиг.
#[test]
fn install_server_command_has_unattended_params() {
    let s = read("installers/install_server.command");
    for (_, flag) in PARAMS {
        assert!(
            s.contains(flag),
            "в install_server.command нет флага {flag}"
        );
    }
    for needle in CONFIG_NEEDLES {
        assert!(
            s.contains(needle),
            "в install_server.command нет '{needle}'"
        );
    }
    // macOS-специфика: LaunchDaemon, ad-hoc подпись, снятие карантина, sudo-гейт.
    for needle in ["launchctl", "codesign", "xattr", "run with sudo"] {
        assert!(
            s.contains(needle),
            "в install_server.command нет '{needle}'"
        );
    }
    // Обратная совместимость: позиционные аргументы и прежний набор ролей.
    assert!(
        s.contains("watch,mcp-http,ui,llm-host"),
        "роли по умолчанию потеряны"
    );
}

/// `update.ps1` (M8 T4): серверный профиль (стоп/старт SCM-служб), окно обслуживания,
/// откат и листинг версий — плюс сохранённая desktop-ветка (Планировщик).
#[test]
fn update_ps1_is_server_profile_aware() {
    let s = read("installers/update.ps1");
    for needle in [
        "[ValidateSet(\"desktop\", \"server\")]",
        "$Profile",
        "hermes-disk-search-",
        "sc.exe stop",
        "sc.exe start",
        "Wait-ScState",
        "install_app_version.ps1",
        "app\\current",
        "-RollbackTo",
        "-ListVersions",
        "$WhatIf",
    ] {
        assert!(s.contains(needle), "в update.ps1 нет '{needle}'");
    }
    // Прежняя desktop-ветка (Планировщик) и останов резидента сохранены.
    for needle in [
        "Stop-ScheduledTask",
        "Start-ScheduledTask",
        "HermesDiskSearchLlmHost",
        "llm_host.exe",
    ] {
        assert!(s.contains(needle), "в update.ps1 нет '{needle}'");
    }
}
