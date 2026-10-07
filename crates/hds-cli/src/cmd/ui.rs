//! `hds ui` — минимальный веб-интерфейс (статус/поиск/ask/индексация).

/// `cmd_ui(host, port)`: блокирующий веб-сервер; 0 — по завершении.
///
/// M2 T2: перед стартом инициализируем каталог состояния (`server` —
/// ProgramData / Application Support; ошибки не фатальны — UI работает и так).
pub fn cmd_ui(host: Option<String>, port: Option<u16>) -> i32 {
    let host = host.unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.unwrap_or(8765);
    if let Ok(cfg) = hds_core::config::load() {
        match hds_core::config::ensure_state_dir(&cfg) {
            Ok(d) => println!("hds ui: состояние в {}", d.display()),
            Err(e) => eprintln!("hds ui: каталог состояния: {e}"),
        }
    }
    println!("hds ui: http://{host}:{port}");
    match hds_ui::run_http(&host, port) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
