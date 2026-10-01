//! `hds stop` — порт `hds/cli.py::cmd_stop`: создать `<root>/index.stop`
//! (файл-сигнал аккуратной остановки индексации).

use std::path::{Path, PathBuf};

use hds_core::error::Result;

/// Создать (или обнулить) `index.stop` в корне проекта — как `open(sf, "w")`.
pub fn stop(project_root: &Path) -> Result<PathBuf> {
    let sf = project_root.join("index.stop");
    std::fs::File::create(&sf)?;
    Ok(sf)
}

/// `cmd_stop`: печатает путь сигнала и пояснение; 0 — успех.
pub fn cmd_stop() -> i32 {
    let root = hds_core::config::project_root();
    match stop(&root) {
        Ok(sf) => {
            println!("Сигнал остановки создан: {}", sf.display());
            println!("Индексатор завершит текущий файл и остановится (обработанное сохранится).");
            0
        }
        Err(e) => {
            println!("Не удалось создать сигнал остановки: {}", e.message());
            1
        }
    }
}
