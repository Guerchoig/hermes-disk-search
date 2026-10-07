//! Бинарь веб-интерфейса disk-search.
//!
//! M2 T3: bind-адрес — из секции `ui` конфига (`ui.host`/`ui.port`), дефолт
//! `127.0.0.1:8765`; флаги `--host/--port` переопределяют конфиг.

use std::env;

fn main() {
    let mut host: Option<String> = None;
    let mut port: Option<u16> = None;
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--host" if i + 1 < args.len() => {
                host = Some(args[i + 1].clone());
                i += 2;
            }
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse().ok();
                i += 2;
            }
            _ => i += 1,
        }
    }
    let cfg = hds_core::config::load().ok();
    if let Some(cfg) = &cfg {
        if let Err(e) = hds_core::config::ensure_state_dir(cfg) {
            eprintln!("hds-ui: каталог состояния: {e}");
        }
    }
    let (cfg_host, cfg_port) = match &cfg {
        Some(cfg) => hds_ui::bind_from_config(cfg),
        None => ("127.0.0.1".to_string(), 8765),
    };
    let host = host.unwrap_or(cfg_host);
    let port = port.unwrap_or(cfg_port);
    println!("hds-ui: http://{host}:{port}");
    if let Err(e) = hds_ui::run_http(&host, port) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
