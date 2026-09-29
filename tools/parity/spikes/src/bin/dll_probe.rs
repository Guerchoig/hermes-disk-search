//! Спайк 6(а, частично) (W0 §4 п.8): загрузка DLL движка openresearchtools/engine
//! через libloading и поиск экспортов cluster API.
//!
//! Запуск:  cargo run --bin dll_probe -- <путь-к-dll> [символ...]
//! Пример:
//!   cargo run --bin dll_probe -- "%APPDATA%\OpenResearchTools\TranscribeOffline\Engine\multi-node-server.dll" \
//!       llama_server_cluster_list_devices llama_server_cluster_create_instance
//!
//! libloading::Library::new() выполняет LoadLibraryW — при отсутствии зависимостей
//! (ggml-cuda.dll и пр.) вернёт код ошибки Win32, что само по себе результат спайка.

use libloading::{Library, Symbol};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("использование: dll_probe <dll> [символ...]");
        std::process::exit(2);
    });
    let symbols: Vec<String> = std::env::args().skip(2).collect();
    println!("LoadLibraryW: {}", path);
    unsafe {
        match Library::new(&path) {
            Ok(lib) => {
                println!("LOAD_OK");
                for s in symbols {
                    let r: Result<Symbol<*mut core::ffi::c_void>, _> = lib.get(s.as_bytes());
                    match r {
                        Ok(_) => println!("  EXPORT {} -> OK", s),
                        Err(e) => println!("  EXPORT {} -> НЕТ ({})", s, e),
                    }
                }
            }
            Err(e) => {
                println!("LOAD_FAIL: {}", e);
                println!("подсказка: чаще всего это не найдена ЗАВИСИМАЯ DLL (ggml-cuda.dll и пр.) — нужен тот же каталог движка в DLL search path (SetDllDirectory / копирование рядом / зависимые DLL рядом с target)");
                std::process::exit(1);
            }
        }
    }
}