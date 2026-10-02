//! Бинарь веб-интерфейса disk-search (`hds_ui`). По умолчанию `http://127.0.0.1:8765`.

use std::env;

fn main() {
    let mut host = "127.0.0.1".to_string();
    let mut port: u16 = 8765;
    let args: Vec<String> = env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--host" if i + 1 < args.len() => {
                host = args[i + 1].clone();
                i += 2;
            }
            "--port" if i + 1 < args.len() => {
                port = args[i + 1].parse().unwrap_or(8765);
                i += 2;
            }
            _ => i += 1,
        }
    }
    println!("hds-ui: http://{host}:{port}");
    if let Err(e) = hds_ui::run_http(&host, port) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
