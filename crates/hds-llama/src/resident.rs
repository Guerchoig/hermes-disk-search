//! A6 — резидентность `llm-host`: pid-файл, лог-файл, защита от второго экземпляра.
//!
//! Почему это отдельный модуль, а не пара вызовов `fs::write` в бинаре:
//! * **второй экземпляр `llm-host` — это второй владелец GPU.** Роли-соседи
//!   (Python `llama-server`, `anonymizer_proxy`) занимают VRAM молча, а два
//!   `llm-host` будут выгружать инстансы друг друга; поэтому pid-файл
//!   создаётся **эксклюзивно** (`create_new`, как `O_CREAT|O_EXCL` в
//!   `hds/watcher.py::_acquire_lock`), а устаревший (процесс умер) — снимается;
//! * **лог нужен после падения.** Резидентный процесс пишет свои решения
//!   (в т.ч. вытеснение VRAM) в `data/logs/llm-host.log`, иначе инцидент
//!   «роль не поднялась» не восстановить: консоли у процесса, запущенного
//!   Планировщиком задач, нет.
//!
//! Раскладка (совпадает с Python-менеджером ролей `hds/llama_server.py`,
//! который держит `data/pid-<role>.pid` и `data/logs/<role>.log`):
//! ```text
//! data\llm-host.pid      — pid резидентного процесса (текстом, как watcher)
//! data\logs\llm-host.log — лог решений/ошибок
//! ```

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{EngineError, Result};

/// Каталог локальных данных проекта (`data/` — в git не входит, `.gitignore`).
pub const DATA_DIR: &str = "data";
/// Подкаталог логов.
pub const LOG_SUBDIR: &str = "logs";
/// Имя pid-файла резидентно процесса.
pub const PID_FILE: &str = "llm-host.pid";
/// Имя лог-файла резидентно процесса.
pub const LOG_FILE: &str = "llm-host.log";
/// Сколько попыток занять pid-файл (как `_acquire_lock(attempts=3)` в Python:
/// между попытками снимается устаревший файл).
pub const ACQUIRE_ATTEMPTS: usize = 3;

/// Каталог локальных данных (`<корень проекта>/data`).
pub fn data_dir(root: &Path) -> PathBuf {
    root.join(DATA_DIR)
}

/// Путь pid-файла по умолчанию (`data/llm-host.pid`).
pub fn default_pid_path(root: &Path) -> PathBuf {
    data_dir(root).join(PID_FILE)
}

/// Путь лог-файла по умолчанию (`data/logs/llm-host.log`).
pub fn default_log_path(root: &Path) -> PathBuf {
    data_dir(root).join(LOG_SUBDIR).join(LOG_FILE)
}

/// Прочитать pid из файла (`None` — файла нет или там не число).
pub fn read_pid(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse::<u32>().ok().filter(|p| *p != 0)
}

/// Живой владелец pid-файла (`None` — файла нет, содержимое битое или процесс умер).
///
/// Смысл ровно как `_lock_is_stale` в `hds/watcher.py`: «файл есть, а процесса нет» —
/// это не «запущен», а мусор после аварийного выхода.
pub fn owner_pid(path: &Path) -> Option<u32> {
    let pid = read_pid(path)?;
    if platform::alive(pid) {
        Some(pid)
    } else {
        None
    }
}

/// Занятый pid-файл: освобождается при `Drop` (и только свой файл).
#[derive(Debug)]
pub struct PidFile {
    path: PathBuf,
    pid: u32,
}

impl PidFile {
    /// Занять pid-файл эксклюзивно.
    ///
    /// * файла нет → создаём и пишем свой pid;
    /// * файл есть, процесс жив → `Err` с готовым текстом «остановите его или
    ///   удалите файл» (второй экземпляр молча не поднимается);
    /// * файл есть, процесс мёртв → снимаем мусор и пробуем снова.
    pub fn acquire(path: impl Into<PathBuf>) -> Result<PidFile> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                EngineError::Other(format!("не удалось создать {}: {e}", dir.display()))
            })?;
        }
        let pid = std::process::id();
        for _ in 0..ACQUIRE_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    writeln!(f, "{pid}").map_err(|e| {
                        EngineError::Other(format!("не удалось записать {}: {e}", path.display()))
                    })?;
                    f.flush().ok();
                    return Ok(PidFile { path, pid });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some(other) = owner_pid(&path) {
                        return Err(EngineError::Other(format!(
                            "llm-host уже запущен (pid {other}, файл {}): второй экземпляр — \
                             второй владелец GPU, поэтому не поднимаемся. Остановите его \
                             (`llm-host stop`) или удалите файл, если процесс завис",
                            path.display()
                        )));
                    }
                    // устаревший pid-файл (процесс умер) — снимаем и пробуем снова
                    let _ = std::fs::remove_file(&path);
                }
                Err(e) => {
                    return Err(EngineError::Other(format!(
                        "не удалось занять {}: {e}",
                        path.display()
                    )))
                }
            }
        }
        Err(EngineError::Other(format!(
            "не удалось занять pid-файл {} за {ACQUIRE_ATTEMPTS} попытки",
            path.display()
        )))
    }

    /// Путь pid-файла (для лога/`status`).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Наш pid.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Удалить файл, если он всё ещё наш (`true` — удалили).
    ///
    /// Проверка содержимого важна: если pid-файл уже переписал другой процесс,
    /// удалять его нельзя (иначе новый `llm-host` остался бы без охраны).
    pub fn release(&self) -> bool {
        if read_pid(&self.path) == Some(self.pid) && std::fs::remove_file(&self.path).is_ok() {
            return true;
        }
        false
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        self.release();
    }
}

/// Лог резидентно процесса: строки идут и в консоль, и в файл.
///
/// Формат намеренно простой (строка как есть, без времени) — так же выглядят
/// логи ролей у Python-менеджера (`data/logs/<role>.log` — это stdout ребёнка).
#[derive(Debug, Default)]
pub struct Log {
    path: Option<PathBuf>,
    file: Option<Mutex<File>>,
}

impl Log {
    /// Лог без файла (разовые прогоны, `--no-log`).
    pub fn silent() -> Log {
        Log::default()
    }

    /// Лог в файл (append; каталог создаётся).
    pub fn to_file(path: impl Into<PathBuf>) -> std::io::Result<Log> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Log {
            path: Some(path),
            file: Some(Mutex::new(file)),
        })
    }

    /// Путь лог-файла (`None` — лог только в консоль).
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Обычная строка: stdout + файл.
    pub fn line(&self, msg: &str) {
        println!("{msg}");
        self.write(msg);
    }

    /// Предупреждение/ошибка: stderr + файл.
    pub fn note(&self, msg: &str) {
        eprintln!("{msg}");
        self.write(msg);
    }

    /// Только в файл (для машинных отчётов рядом с человеческим выводом).
    pub fn file_only(&self, msg: &str) {
        self.write(msg);
    }

    fn write(&self, msg: &str) {
        // ошибки записи не должны ронять хост: лог — вспомогательный канал
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = writeln!(f, "{msg}");
                let _ = f.flush();
            }
        }
    }
}

/// Проверка «процесс с таким pid жив» — минимальным FFI, без новых зависимостей.
///
/// На машине заказчика `crates.io` недоступен (грабли W2 §9.7 п.13), поэтому
/// вместо включения фич `windows-sys::Win32_System_Threading` объявляем две
/// функции kernel32 напрямую (kernel32 линкуется к процессу всегда).
#[cfg(windows)]
mod platform {
    use std::os::raw::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(handle: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// `PROCESS_QUERY_LIMITED_INFORMATION`: прав достаточно, чтобы спросить
    /// состояние процесса (работает и для системных процессов).
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    /// `STILL_ACTIVE` — процесс ещё работает.
    const STILL_ACTIVE: u32 = 259;

    pub fn alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false; // нет такого процесса или нет прав
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == STILL_ACTIVE
        }
    }
}

#[cfg(unix)]
mod platform {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    /// `kill(pid, 0)` — проверка существования процесса без отправки сигнала.
    pub fn alive(pid: u32) -> bool {
        pid != 0 && unsafe { kill(pid as i32, 0) == 0 }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    /// На прочих платформах считаем процесс живым (лучше перестраховаться:
    /// второй владелец GPU опаснее, чем «не смог запуститься»).
    pub fn alive(_pid: u32) -> bool {
        true
    }
}
