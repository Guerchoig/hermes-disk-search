//! `hds ui` — минимальный веб-интерфейс (статус/поиск/ask/индексация).

/// `cmd_ui(host, port)`: блокирующий веб-сервер; 0 — по завершении.
pub fn cmd_ui(host: Option<String>, port: Option<u16>) -> i32 {
    let host = host.unwrap_or_else(|| "127.0.0.1".to_string());
    let port = port.unwrap_or(8765);
    println!("hds ui: http://{host}:{port}");
    match hds_ui::run_http(&host, port) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}
