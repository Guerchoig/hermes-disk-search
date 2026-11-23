//! `hds stop` — порт `hds/cli.py::cmd_stop`: создать `<root>/index.stop`
//! (файл-сигнал аккуратной остановки индексации).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use hds_core::error::Result;

/// Создать (или обнулить) `index.stop` в корне проекта — как `open(sf, "w")`.
///
/// A6 (AG-34): файл остаётся пустым, но время изменения ставится **явно**.
/// Повторная запись нуля байт mtime не обновляла, а демон зеркала считает
/// сигналом только файл, изменённый после его старта, — второй `hds stop`
/// подряд молча ничего не останавливал.
pub fn stop(project_root: &Path) -> Result<PathBuf> {
    let sf = project_root.join("index.stop");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&sf)?;
    file.set_modified(SystemTime::now())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// AG-34: повторный `hds stop` обновляет mtime, файл остаётся пустым.
    #[test]
    fn stop_refreshes_signal_time_and_keeps_file_empty() {
        let dir = std::env::temp_dir().join(format!("hds-stop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = stop(&dir).unwrap();
        let t1 = std::fs::metadata(&first).unwrap().modified().unwrap();
        // Гранулярность mtime на разных ФС разная: ждём заметно больше тика.
        std::thread::sleep(std::time::Duration::from_millis(30));
        let second = stop(&dir).unwrap();
        let t2 = std::fs::metadata(&second).unwrap().modified().unwrap();
        assert_eq!(first, second);
        assert!(t2 > t1, "mtime должен обновиться: {t1:?} -> {t2:?}");
        assert_eq!(
            std::fs::metadata(&second).unwrap().len(),
            0,
            "сигнал остаётся пустым файлом"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
