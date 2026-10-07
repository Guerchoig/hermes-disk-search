//! `hds ui` — минимальный веб-интерфейс (статус/поиск/ask/индексация).

/// `cmd_ui(host, port)`: блокирующий веб-сервер; 0 — по завершении.
///
/// M2 T3: host/port берутся из секции `ui` конфига (CLI-флаги — переопределение);
/// M2 T2: перед стартом инициализируется каталог состояния (`server` —
/// ProgramData / Application Support; ошибки не фатальны — UI работает и так).
pub fn cmd_ui(host: Option<String>, port: Option<u16>) -> i32 {
    let loaded = hds_core::config::load();
    if let Ok(cfg) = &loaded {
        match hds_core::config::ensure_state_dir(cfg) {
            Ok(d) => println!("hds ui: состояние в {}", d.display()),
            Err(e) => eprintln!("hds ui: каталог состояния: {e}"),
        }
    }
    let (cfg_host, cfg_port) = match &loaded {
        Ok(cfg) => hds_ui::bind_from_config(cfg),
        Err(_) => ("127.0.0.1".to_string(), 8765),
    };
    let host = host.unwrap_or(cfg_host);
    let port = port.unwrap_or(cfg_port);
    println!("hds ui: http://{host}:{port}");
    match hds_ui::run_http(&host, port) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
