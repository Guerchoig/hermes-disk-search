//! A5/A6: фасад `:8010–8012` — тонкий бинарь над `hds_llama::host::Host`.
//!
//! Вся работа (инстансы по конфигу, диспетчер VRAM, HTTP-фасад, уборка) переехала
//! в библиотеку (`src/host.rs`): там её видно тестам, а бинарь остаётся разбором
//! argv. Порты по умолчанию — как у `llama-server` (8010/8011/8012), поэтому
//! клиенты (UI, MCP, Hermes, внешние агенты) не меняются. Если порт занят
//! (например ещё работает Python-версия) — фасад честно скажет об этом и не
//! станет его отбирать.
//!
//! Этот бинарь — **разовый прогон**: pid-файл по умолчанию не занимается
//! (`--residency` — если нужно наоборот), `--hold SEC` завершает процесс по
//! таймеру. Боевой резидентный режим — `llm_host run` (pid + лог + CLI).
//!
//! Запуск из корня репозитория:
//! ```powershell
//! # проверочный прогон на альтернативных портах и с чатом на CPU (не трогая VRAM)
//! cargo run -p hds-llama --release --bin llm_host_facade -- --port-base 8020 --ngl 0 --hold 120
//! # боевой режим: порты 8010–8012 и устройство из конфига
//! cargo run -p hds-llama --release --bin llm_host_facade -- --hold 0
//! ```

use std::path::PathBuf;

use hds_llama::facade::Thinking;
use hds_llama::host::{self, Host, HostConfig};
use hds_llama::Result;

struct Args {
    host: HostConfig,
    hold: u64,
    json: Option<PathBuf>,
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut host = HostConfig::default().without_residency();
    let mut hold = 0u64;
    let mut json: Option<PathBuf> = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> std::result::Result<String, String> {
            it.next()
                .ok_or_else(|| format!("после {name} ожидалось значение"))
        };
        match a.as_str() {
            "--config" => host.config = PathBuf::from(take("--config")?),
            "--runtime" => host.runtime = Some(PathBuf::from(take("--runtime")?)),
            "--engine-dir" => host.engine_dir = Some(PathBuf::from(take("--engine-dir")?)),
            "--host" => host.host = Some(take("--host")?),
            "--pause-dir" => host.pause_dir = PathBuf::from(take("--pause-dir")?),
            "--port-base" => {
                host.port_base = Some(
                    take("--port-base")?
                        .parse()
                        .map_err(|e| format!("--port-base: {e}"))?,
                )
            }
            "--ngl" => {
                host.ngl = Some(take("--ngl")?.parse().map_err(|e| format!("--ngl: {e}"))?)
            }
            "--thinking" => {
                host.thinking = Some(Thinking::parse(&take("--thinking")?));
            }
            "--dispatcher" => match take("--dispatcher")?.as_str() {
                "on" | "true" | "1" => host.dispatcher = true,
                "off" | "false" | "0" => host.dispatcher = false,
                other => return Err(format!("--dispatcher: ожидалось on|off, получено {other}")),
            },
            // резидентность: разовый прогон её не берёт, если не попросили явно
            "--residency" => {
                host.pid_file = Some(hds_llama::resident::default_pid_path(&host.pause_dir));
                host.log_file = Some(hds_llama::resident::default_log_path(&host.pause_dir));
            }
            "--pid" => host.pid_file = Some(PathBuf::from(take("--pid")?)),
            "--log" => host.log_file = Some(PathBuf::from(take("--log")?)),
            "--no-internal" => host.internal = false,
            "--hold" => {
                hold = take("--hold")?
                    .parse()
                    .map_err(|e| format!("--hold: {e}"))?
            }
            "--json" => json = Some(PathBuf::from(take("--json")?)),
            "--help" | "-h" => {
                return Err("использование: llm_host_facade [--config FILE] [--runtime DIR] \
                            [--engine-dir DIR] [--host HOST] [--port-base N] [--ngl N] \
                            [--thinking off|on|auto] [--dispatcher on|off] [--hold SEC] \
                            [--json FILE] [--residency] [--pid FILE] [--log FILE] [--no-internal]"
                    .to_string())
            }
            other => return Err(format!("неизвестный аргумент: {other}")),
        }
    }
    Ok(Args { host, hold, json })
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
    // путь отчёта считаем абсолютным ДО активации каталога движка (грабля §9.7 п.1)
    let json_path = args.json.as_ref().map(|p| host::absolutize(p));
    let hold = args.hold;

    let mut host = Host::start(args.host)?;
    host.wait(hold);
    host.stop();

    if let Some(json_path) = &json_path {
        let report = serde_json::json!({
            "config": host.config().config.display().to_string(),
            "engine_dir": host.engine_dir().map(|d| d.display().to_string()),
            "mode": host.mode().as_str(),
            "host": host.host(),
            "ports": host
                .ports()
                .iter()
                .map(|(r, p)| serde_json::json!({ "role": r, "port": p }))
                .collect::<Vec<_>>(),
            "thinking": host.server().thinking.as_str(),
            "max_tokens": host.server().max_tokens,
            "temperature": host.server().temperature,
            "dispatcher_enabled": host.config().dispatcher,
            "dispatcher_lines": host.dispatcher_lines(),
            "needs_mib": host.needs(),
            "instances": host
                .ids()
                .iter()
                .map(|(r, id)| serde_json::json!({ "role": r, "id": id }))
                .collect::<Vec<_>>(),
            "cleanup": host.cleanup_log(),
            "pause_present_after": host.pause_present(),
            "pid": host.pid(),
            "pid_file": host.pid_path().display().to_string(),
            "log_file": host.log_path().display().to_string(),
            "hold_sec": hold,
        });
        host::write_json_file(json_path, &report)?;
    }
    Ok(())
}
