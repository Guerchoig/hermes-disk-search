//! M8 T1: unattended-параметры серверного установщика (Windows) и паритет текстов.
//!
//! Статический контракт (`PLAN_M8_INSTALL_PACKAGES.md` §3): `install_server.ps1`
//! экспонирует unattended-набор параметров и генерирует server-конфиг; macOS-аналог
//! (`install_server.command`) получает тот же набор — паритет закрывается в T2.

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

/// Ожидаемый unattended-набор параметров серверного установщика (общий для Win/mac).
const EXPECTED_PARAMS: &[&str] = &[
    "StateDir",
    "McpHost",
    "McpPort",
    "UiHost",
    "UiPort",
    "Parallel",
    "GpuDevice",
    "GpuPreset",
    "Roots",
    "EnableAccess",
    "AdminLogin",
    "AdminPassword",
    "Fetch",
    "NoFetch",
    "WhatIf",
];

/// `install_server.ps1`: unattended-параметры, server-конфиг, fetch-хуки, хинт токена.
#[test]
fn install_server_ps1_has_unattended_params() {
    let s = read("installers/install_server.ps1");
    for p in EXPECTED_PARAMS {
        assert!(
            s.contains(&format!("${p}")),
            "в install_server.ps1 нет параметра ${p}"
        );
    }
    for needle in [
        "profile: server",
        "enabled: true",
        "llm_server:",
        "  parallel:",
        "device_index:",
        "roots:",
        "fetch_engine_runtime.ps1",
        "fetch_llm_models.ps1",
        "fetch_whisper_model.ps1",
        "fetch_clip_models.ps1",
        "token new --user",
    ] {
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

/// macOS-установщик (`install_server.command`) получает тот же unattended-набор — в T2.
/// На bash параметры задаются иначе (позиционные/ENV), поэтому сверку делает T2.
#[test]
#[ignore = "T2: дописать unattended-параметры в install_server.command (macOS)"]
fn install_server_command_params_parity() {
    let s = read("installers/install_server.command");
    for p in EXPECTED_PARAMS {
        assert!(s.contains(p), "в install_server.command нет параметра {p}");
    }
}
