//! A6: `llm-host` — резидентный владелец GPU и его CLI.
//!
//! Один процесс-владелец GPU: держит инстансы чата/эмбеддингов/реранка, фасад
//! `:8010–8012` (как `llama-server`, клиенты не меняются), диспетчер VRAM и
//! `index.pause`; пишет `data/llm-host.pid` и `data/logs/llm-host.log`.
//!
//! Управление — **через сам фасад** (внутренние маршруты `/internal/*`): отдельный
//! канал (сигналы, именованные трубы) не нужен, а кросс-процессная адресация
//! инстансов движка всё равно отсутствует (замер A3), поэтому HTTP — единственный
//! способ договориться с уже запущенным хостом.
//!
//! ```powershell
//! # боевой запуск (порты 8010-8012 и устройство из конфига)
//! cargo run -p hds-llama --release --bin llm_host -- run
//! # проверка: чат на CPU, альтернативные порты, без pid-файла, на 60 секунд
//! cargo run -p hds-llama --release --bin llm_host -- run --port-base 8020 --ngl 0 --no-residency --hold 60
//! # управление запущенным хостом
//! cargo run -p hds-llama --release --bin llm_host -- status
//! cargo run -p hds-llama --release --bin llm_host -- devices
//! cargo run -p hds-llama --release --bin llm_host -- load rerank
//! cargo run -p hds-llama --release --bin llm_host -- unload rerank
//! cargo run -p hds-llama --release --bin llm_host -- stop
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use hds_llama::config;
use hds_llama::facade::Thinking;
use hds_llama::host::{self, Host, HostConfig, LocalStatusArgs};
use hds_llama::resident;
use hds_llama::{client_json, EngineError, Result};

/// Подкоманда CLI.
enum Cmd {
    Run,
    Status,
    Load(String),
    Unload(String),
    Devices,
    Stop,
}

struct Args {
    cmd: Cmd,
    host: HostConfig,
    /// Порт фасада для внутренних вызовов (иначе — из конфига, первый ролевой).
    port: Option<u16>,
    json: Option<PathBuf>,
    hold: u64,
    /// Только локальный отчёт, без опроса резидента.
    local: bool,
    /// Локальный отчёт: не открывать движок.
    no_engine: bool,
    baseline_used_mib: Option<u64>,
    /// `stop --force`: завершить резидент по pid-файлу, не полагаясь на HTTP
    /// (движок мог зависнуть и не отдать уборку — `W4_REPORT.md` §14).
    force: bool,
}

fn usage() -> String {
    "использование: llm_host <run|status|load <role>|unload <role>|devices|stop> [флаги]\n\
     флаги: [--config FILE] [--runtime DIR] [--engine-dir DIR] [--host HOST] [--port N]\n\
     \x20      [--port-base N] [--ngl N] [--thinking off|on|auto] [--dispatcher on|off]\n\
     \x20      [--hold SEC] [--json FILE] [--local] [--no-engine] [--baseline-used-mib N]\n\
     \x20      [--no-residency] [--no-log] [--no-internal]\n\
     \x20      [--force]  # stop: завершить резидент по pid-файлу, если HTTP/движок завис"
        .to_string()
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut host = HostConfig::default();
    let mut port = None;
    let mut json = None;
    let mut hold = 0u64;
    let mut local = false;
    let mut no_engine = false;
    let mut baseline_used_mib = None;
    let mut force = false;
    let mut it = std::env::args().skip(1);
    let first = it.next().ok_or_else(usage)?;
    let cmd = match first.as_str() {
        "run" => Cmd::Run,
        "status" => Cmd::Status,
        "devices" => Cmd::Devices,
        "stop" => Cmd::Stop,
        "load" => Cmd::Load(it.next().ok_or("после load ожидалась роль")?),
        "unload" => Cmd::Unload(it.next().ok_or("после unload ожидалась роль")?),
        "--help" | "-h" | "help" => return Err(usage()),
        other => return Err(format!("неизвестная подкоманда '{other}'\n{}", usage())),
    };
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
            "--port" => {
                port = Some(
                    take("--port")?
                        .parse()
                        .map_err(|e| format!("--port: {e}"))?,
                )
            }
            "--port-base" => {
                host.port_base = Some(
                    take("--port-base")?
                        .parse()
                        .map_err(|e| format!("--port-base: {e}"))?,
                )
            }
            "--ngl" => host.ngl = Some(take("--ngl")?.parse().map_err(|e| format!("--ngl: {e}"))?),
            "--thinking" => host.thinking = Some(Thinking::parse(&take("--thinking")?)),
            "--dispatcher" => match take("--dispatcher")?.as_str() {
                "on" | "true" | "1" => host.dispatcher = true,
                "off" | "false" | "0" => host.dispatcher = false,
                other => return Err(format!("--dispatcher: ожидалось on|off, получено {other}")),
            },
            "--hold" => {
                hold = take("--hold")?
                    .parse()
                    .map_err(|e| format!("--hold: {e}"))?
            }
            "--json" => json = Some(PathBuf::from(take("--json")?)),
            "--local" => local = true,
            "--no-engine" => no_engine = true,
            "--baseline-used-mib" => {
                baseline_used_mib = Some(
                    take("--baseline-used-mib")?
                        .parse()
                        .map_err(|e| format!("--baseline-used-mib: {e}"))?,
                )
            }
            "--no-residency" => {
                // только pid-файл: лог — отдельная ручка (`--no-log`), иначе порядок
                // флагов решал бы, будет ли лог (грабля, поймана `arb_scenarios.py`)
                host.pid_file = None;
            }
            "--no-log" => host.log_file = None,
            "--force" => force = true,
            "--pid" => host.pid_file = Some(PathBuf::from(take("--pid")?)),
            "--log" => host.log_file = Some(PathBuf::from(take("--log")?)),
            other => return Err(format!("неизвестный аргумент: {other}")),
        }
    }
    Ok(Args {
        cmd,
        host,
        port,
        json,
        hold,
        local,
        no_engine,
        baseline_used_mib,
        force,
    })
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
    match &args.cmd {
        Cmd::Run => cmd_run(&args),
        Cmd::Status => cmd_status(&args),
        Cmd::Devices => cmd_internal(&args, "devices", None),
        Cmd::Load(role) => cmd_internal(&args, "load", Some(role.clone())),
        Cmd::Unload(role) => cmd_internal(&args, "unload", Some(role.clone())),
        Cmd::Stop => {
            if args.force {
                cmd_stop_force(&args)
            } else {
                cmd_internal(&args, "stop", None)
            }
        }
    }
}

/// `llm_host run` — резидентный режим (pid-файл + лог; остановка — `stop`/Ctrl+C).
fn cmd_run(args: &Args) -> Result<()> {
    let json_path = args.json.as_ref().map(|p| host::absolutize(p));
    let mut host = match Host::start(args.host.clone()) {
        Ok(h) => h,
        Err(e) => {
            // самая частая причина — уже запущенный резидент: подсказка важнее трейса
            eprintln!("не удалось запустить llm-host: {e}");
            eprintln!(
                "подсказка: проверьте `llm_host status` (что уже работает) и {}",
                args.host.pid_path().display()
            );
            return Err(e);
        }
    };
    if host.ports().is_empty() {
        eprintln!(
            "внимание: фасад не поднят (нет ролей чата/эмбеддингов/реранка) — \
             управление через `/internal/*` недоступно"
        );
    }
    host.wait(args.hold);
    host.stop();
    if let Some(path) = &json_path {
        let report = serde_json::json!({
            "config": host.config().config.display().to_string(),
            "mode": host.mode().as_str(),
            "engine_dir": host.engine_dir().map(|d| d.display().to_string()),
            "host": host.host(),
            "ports": host
                .ports()
                .iter()
                .map(|(r, p)| serde_json::json!({ "role": r, "port": p }))
                .collect::<Vec<_>>(),
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
            "uptime_sec": host.uptime().as_secs(),
        });
        host::write_json_file(path, &report)?;
    }
    Ok(())
}

/// Адреса для внутренних вызовов: `--port` → один адрес, иначе порты ролей
/// из конфига (по порядку: `chat`, `embedding`, `rerank`).
fn candidate_urls(args: &Args, cfg: &config::LlmHostConfig, host_addr: &str) -> Vec<String> {
    if let Some(p) = args.port {
        return vec![format!("http://{host_addr}:{p}")];
    }
    host::client_ports(cfg, host_addr, args.host.port_base)
}

/// `llm-host status`: сначала `internal/status` резидента, иначе локальный отчёт.
///
/// Резидент — источник истины (он владеет инстансами), но при выключенном хосте
/// пользователь всё равно должен увидеть состояние: тогда считаем локально тем же
/// кодом, что и `llm_host_status` (общий `host::local_status`).
fn cmd_status(args: &Args) -> Result<()> {
    let cfg = config::load(&args.host.config)?;
    let host_addr = args.host.host.clone().unwrap_or_else(|| cfg.host.clone());
    let urls = candidate_urls(args, &cfg, &host_addr);
    if !args.local {
        for url in &urls {
            let target = format!("{url}/internal/status");
            if let Ok((200, json)) = client_json("GET", &target, None, Duration::from_secs(5)) {
                println!("llm-host (резидент): {url}");
                print_lines(&json);
                println!(
                    "режим: {}; pid {}; uptime {} с; порты: {}",
                    json.get("mode").and_then(|v| v.as_str()).unwrap_or("?"),
                    json.get("pid").and_then(|v| v.as_i64()).unwrap_or(0),
                    json.get("uptime_sec").and_then(|v| v.as_u64()).unwrap_or(0),
                    json.get("ports")
                        .and_then(|v| v.as_array())
                        .map(|a| a
                            .iter()
                            .filter_map(|p| Some(format!(
                                "{}:{}",
                                p.get("role")?.as_str()?,
                                p.get("port")?.as_u64()?
                            )))
                            .collect::<Vec<_>>()
                            .join(", "))
                        .unwrap_or_default()
                );
                if let Some(path) = &args.json {
                    host::write_json_file(&host::absolutize(path), &json)?;
                }
                if roles_broken(&json) {
                    std::process::exit(1);
                }
                return Ok(());
            }
        }
        println!(
            "резидент llm-host не отвечает (проверены порты: {}) — показываю локальный отчёт \
             (запуск: `llm_host run`)",
            urls.join(", ")
        );
        // L1: HTTP молчит — читаем heartbeat резидента: его пишет отдельный поток, поэтому
        // «кто держит движок и сколько» видно и при мёртвом HTTP (`W4_REPORT.md` §14).
        let hb_path = resident::default_heartbeat_path(&args.host.pause_dir);
        let now = resident::unix_now();
        match resident::Heartbeat::read(&hb_path) {
            Some(hb) if hb.is_live(now) => {
                println!("heartbeat ({} с назад): {}", hb.age_sec(now), hb.line())
            }
            Some(hb) => println!(
                "heartbeat не свежий ({} с назад) — {}; похоже, резидент завис на вызове движка",
                hb.age_sec(now),
                hb.line()
            ),
            None => println!(
                "heartbeat {} не найден — резидент либо старой сборки, либо не писал состояние",
                hb_path.display()
            ),
        }
        println!(
            "подсказка: если движок занят давно, штатный stop не сработает — \
             используйте `llm_host stop --force` (завершит процесс по pid-файлу)"
        );
    }

    let local = host::local_status(&LocalStatusArgs {
        config: args.host.config.clone(),
        runtime: args.host.runtime.clone(),
        engine_dir: args.host.engine_dir.clone(),
        baseline_used_mib: args.baseline_used_mib,
        pause_dir: args.host.pause_dir.clone(),
        no_engine: args.no_engine,
        nvml_index: args.host.nvml_index,
    })?;
    for line in local.lines() {
        println!("{line}");
    }
    if let Some(path) = &args.json {
        host::write_json_file(&host::absolutize(path), &local.report.json())?;
    }
    if local.report.roles.iter().any(|r| {
        r.state_code == Some(hds_llama::ffi::state::FAILED)
            || r.notes.iter().any(|n| n.contains("не спланирована"))
    }) {
        std::process::exit(1);
    }
    Ok(())
}

/// `llm-host stop --force` — завершить резидент по pid-файлу, не полагаясь на HTTP.
///
/// Зачем: штатный `stop` идёт через `/internal/stop`, а уборка в `Host::stop` зовёт
/// `remove_instance` под мьютексом движка — при зависшем движке (`W4_REPORT.md` §14)
/// резидент остаётся с занятой VRAM и не отвечает. Здесь сначала просим по-хорошему
/// (короткий таймаут, без ожидания уборки), затем завершаем процесс и ждём, пока
/// освободится pid-файл.
fn cmd_stop_force(args: &Args) -> Result<()> {
    let pid_path = args.host.pid_path();
    let pid = resident::owner_pid(&pid_path).ok_or_else(|| {
        EngineError::Other(format!(
            "резидент не найден: {} пуст или процесс уже мёртв — `stop --force` нечего делать",
            pid_path.display()
        ))
    })?;
    let cfg = config::load(&args.host.config)?;
    let host_addr = args.host.host.clone().unwrap_or_else(|| cfg.host.clone());
    for url in candidate_urls(args, &cfg, &host_addr) {
        let target = format!("{url}/internal/stop");
        if client_json("POST", &target, None, Duration::from_secs(3)).is_ok() {
            println!("штатный stop отправлен ({url}) — жду завершения до 10 с");
            break;
        }
    }
    if wait_pid_file_free(&pid_path, Duration::from_secs(10)) {
        println!("резидент остановлен штатно (pid {pid})");
        return Ok(());
    }
    println!(
        "штатная уборка не завершилась (движок занят и не отдаёт мьютекс) — завершаю pid {pid} \
         принудительно"
    );
    if !resident::terminate(pid) {
        return Err(EngineError::Other(format!(
            "не удалось завершить pid {pid} (taskkill); завершите вручную и повторите"
        )));
    }
    if wait_pid_file_free(&pid_path, Duration::from_secs(15)) {
        println!("резидент завершён (pid {pid}); VRAM освобождена, `llm_host run` поднимет заново");
        return Ok(());
    }
    // процесс убит, а pid-файл остался: снимаем как устаревший, иначе следующий старт
    // откажется подниматься («второй владелец GPU»)
    let _ = std::fs::remove_file(&pid_path);
    println!(
        "резидент завершён (pid {pid}); устаревший pid-файл {} снят",
        pid_path.display()
    );
    Ok(())
}

/// Дождаться, пока pid-файл перестанет указывать на живой процесс.
fn wait_pid_file_free(path: &std::path::Path, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if resident::owner_pid(path).is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// `llm-host load|unload|devices|stop` — через внутренний API резидента.
fn cmd_internal(args: &Args, action: &str, role: Option<String>) -> Result<()> {
    let cfg = config::load(&args.host.config)?;
    let host_addr = args.host.host.clone().unwrap_or_else(|| cfg.host.clone());
    let urls = candidate_urls(args, &cfg, &host_addr);
    let method = if action == "devices" { "GET" } else { "POST" };
    // загрузка роли идёт с диска (секунды-минуты), остальное быстро
    let timeout = if action == "load" {
        Duration::from_secs(600)
    } else {
        Duration::from_secs(30)
    };
    let body = role
        .as_ref()
        .map(|r| serde_json::json!({ "role": r }).to_string());
    for url in &urls {
        let target = format!("{url}/internal/{action}");
        match client_json(method, &target, body.as_deref(), timeout) {
            Ok((status, json)) if (200..300).contains(&status) => {
                println!("llm-host: {action} — ок ({url})");
                print_lines(&json);
                if let Some(state) = json.get("state").and_then(|v| v.as_str()) {
                    println!("состояние роли: {state}");
                }
                if action == "stop" {
                    println!(
                        "хост завершается: уборка инстансов и освобождение pid-файла — в его логе"
                    );
                }
                return Ok(());
            }
            Ok((status, json)) => {
                let msg = json
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("без текста ошибки");
                return Err(EngineError::Other(format!(
                    "{target}: HTTP {status}: {msg}"
                )));
            }
            // порт не отвечает: возможно, это роль без фасада — пробуем следующий
            Err(_) => continue,
        }
    }
    Err(EngineError::Other(format!(
        "резидент llm-host не отвечает (проверены порты: {}) — запустите `llm_host run`",
        urls.join(", ")
    )))
}

/// Напечатать готовые человеческие строки из ответа (`lines`).
fn print_lines(json: &serde_json::Value) {
    for line in json
        .get("lines")
        .and_then(|l| l.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
    {
        println!("{line}");
    }
}

/// Есть ли в отчёте резидента сломанные роли (для кода возврата `status`).
fn roles_broken(json: &serde_json::Value) -> bool {
    json.get("roles")
        .and_then(|r| r.as_array())
        .map(|roles| {
            roles.iter().any(|r| {
                let failed = r
                    .get("state")
                    .and_then(|s| s.as_str())
                    .map(|s| s == "FAILED")
                    .unwrap_or(false);
                let unplanned = r
                    .get("notes")
                    .and_then(|n| n.as_array())
                    .map(|a| {
                        a.iter().any(|x| {
                            x.as_str()
                                .map(|s| s.contains("не спланирована"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);
                failed || unplanned
            })
        })
        .unwrap_or(false)
}
