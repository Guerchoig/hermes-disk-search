//! A4 (шаг 2) → A6: `llm_host_status` — состояние ролей, VRAM и паузы.
//!
//! Тонкий бинарь: всю сборку отчёта делает `hds_llama::host::local_status` (тот же
//! код, что у `llm_host status`, когда резидент не отвечает). Показывает:
//! устройства движка, свободную VRAM (NVML — источник истины, R29), бюджет
//! (`gpu.reserve_mb`/cap/вычет), `index.pause` и heartbeat индексации, роли с их
//! состоянием/контекстом/потребностью и прогноз диспетчера. Плюс `--json` — те же
//! поля для UI/MCP.
//!
//! Запуск из корня репозитория:
//! `cargo run -p hds-llama --release --bin llm_host_status -- [--config FILE]
//!  [--runtime DIR] [--engine-dir DIR] [--json FILE] [--baseline-used-mib N]
//!  [--pause-dir DIR] [--no-engine] [--nvml-index N]`

use std::path::PathBuf;

use hds_llama::host::{self, LocalStatusArgs};
use hds_llama::Result;

struct Args {
    status: LocalStatusArgs,
    json: Option<PathBuf>,
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut status = LocalStatusArgs::default();
    let mut json: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> std::result::Result<String, String> {
            it.next()
                .ok_or_else(|| format!("после {name} ожидалось значение"))
        };
        match a.as_str() {
            "--config" => status.config = PathBuf::from(take("--config")?),
            "--runtime" => status.runtime = Some(PathBuf::from(take("--runtime")?)),
            "--engine-dir" => status.engine_dir = Some(PathBuf::from(take("--engine-dir")?)),
            "--json" => json = Some(PathBuf::from(take("--json")?)),
            "--pause-dir" => status.pause_dir = PathBuf::from(take("--pause-dir")?),
            "--baseline-used-mib" => {
                status.baseline_used_mib = Some(
                    take("--baseline-used-mib")?
                        .parse()
                        .map_err(|e| format!("--baseline-used-mib: {e}"))?,
                )
            }
            "--nvml-index" => {
                status.nvml_index = take("--nvml-index")?
                    .parse()
                    .map_err(|e| format!("--nvml-index: {e}"))?
            }
            "--no-engine" => status.no_engine = true,
            "--help" | "-h" => {
                return Err("использование: llm_host_status [--config FILE] [--runtime DIR] \
                            [--engine-dir DIR] [--json FILE] [--baseline-used-mib N] \
                            [--pause-dir DIR] [--nvml-index N] [--no-engine]"
                    .to_string())
            }
            other => return Err(format!("неизвестный аргумент: {other}")),
        }
    }
    Ok(Args { status, json })
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ошибка: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    // путь отчёта — абсолютным до активации каталога движка (грабля §9.7 п.1)
    let json_path = args.json.as_ref().map(|p| host::absolutize(p));

    let local = host::local_status(&args.status)?;
    for line in local.lines() {
        println!("{line}");
    }
    if let Some(path) = &json_path {
        host::write_json_file(path, &local.report.json())?;
    }

    // код возврата: сломанные роли/не спланированные модели — 1 (для автопроверок)
    if local.report.roles.iter().any(|r| {
        r.state_code == Some(hds_llama::ffi::state::FAILED)
            || r.notes.iter().any(|n| n.contains("не спланирована"))
    }) {
        std::process::exit(1);
    }
    Ok(())
}
