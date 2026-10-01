//! Наблюдатель файловой системы — порт `hds/watcher.py` (`PLAN_W2_LLM_HOST.md` §5/B5).
//!
//! На Windows события ОС берём через **`ReadDirectoryChangesW`** — тот же механизм,
//! что использует `watchdog`/`notify`; на прочих платформах — опрос (polling).
//!
//! Почему свой backend, а не `notify`: на машине заказчика **`crates.io` недоступен**
//! (грабля §9.7 п.13), а крейта `notify` в локальном кэше нет — новые зависимости
//! добавить нельзя. `ReadDirectoryChangesW` объявлен минимальным FFI (как проверка
//! PID в `hds-llama::resident`).
//!
//! Сохранено поведение Python:
//! * `watch.lock` занимается **атомарно** (`O_CREAT|O_EXCL` → `create_new`), устаревший
//!   (умерший/чужой PID) снимается и попытка повторяется; два одновременных старта не
//!   дают дублей;
//! * `wait_stable` — ждём, пока размер файла перестанет меняться (`debounce`);
//! * события: `created`/`modified` → индексация, `deleted` → удаление из индекса,
//!   `moved` → `rename_path` (в корзину — удаление; перезапись — снять старую запись);
//! * `.tmp` и исключённые пути (корзина, системные каталоги) пропускаются;
//! * reconcile при старте (`watch.reconcile_on_start`) — через `pipeline::run_index`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hds_core::config::{dig, project_root, Config};
use hds_core::db;
use hds_core::error::Result;

use crate::embed::Embedder;
use crate::pipeline;
use crate::sidecar::{Extractor, Lemmatizer};

/// Событие наблюдения (порт кортежей очереди `_worker`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// `created`/`modified` — файл создан/изменён.
    Modified(PathBuf),
    /// `deleted` — файл удалён.
    Deleted(PathBuf),
    /// `moved` — переименование/перемещение (src → dst).
    Moved(PathBuf, PathBuf),
}

/// Счётчики наблюдателя (порт `watcher._state`).
#[derive(Debug, Clone, Default)]
pub struct WatchState {
    pub processed: u64,
    pub errors: u64,
    pub moved: u64,
    pub last_event: Option<f64>,
}

impl WatchState {
    /// Порт `watcher.status()`.
    pub fn snapshot(&self) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert("processed".into(), serde_json::json!(self.processed));
        m.insert("errors".into(), serde_json::json!(self.errors));
        m.insert("moved".into(), serde_json::json!(self.moved));
        m.insert(
            "last_event".into(),
            serde_json::json!(self.last_event),
        );
        m
    }
}

/// Порт `wait_stable`: ждём стабилизации размера файла (debounce), не дольше `max_wait`.
///
/// Возвращает `false`, если файл исчез (в Python — `return False`).
pub fn wait_stable(path: &Path, debounce: u64, max_wait: u64) -> bool {
    let t0 = Instant::now();
    let mut last_size: i64 = -1;
    let mut stable: u64 = 0;
    while t0.elapsed().as_secs() < max_wait {
        let size = match std::fs::metadata(path) {
            Ok(m) => m.len() as i64,
            Err(_) => return false, // файл исчез
        };
        if size == last_size {
            stable += 1;
            if stable * 2 >= debounce {
                return true;
            }
        } else {
            stable = 0;
            last_size = size;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    true
}

/// Порт `_pid_alive` (Windows — `OpenProcess`, POSIX — `kill(pid, 0)`).
pub fn pid_alive(pid: u32) -> bool {
    platform::alive(pid)
}

#[cfg(windows)]
mod platform {
    use std::os::raw::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(handle: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;

    /// Порт `_pid_alive` для Windows (безопасен: `os.kill(0)` в Python убивает!).
    pub fn alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
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

    /// `kill(pid, 0)` — проверка существования процесса без сигнала.
    pub fn alive(pid: u32) -> bool {
        pid != 0 && unsafe { kill(pid as i32, 0) == 0 }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    /// На прочих платформах считаем процесс живым (пессимистично, как в Python без psutil).
    pub fn alive(_pid: u32) -> bool {
        true
    }
}

fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Занятый `watch.lock` (RAII: снимается на `release`/`Drop`).
pub struct WatchLock {
    path: PathBuf,
}

impl WatchLock {
    /// Порт `_acquire_lock`: атомарно (`create_new`) занять `watch.lock`.
    ///
    /// `None` — уже работает другой watcher. Устаревший lock (PID ≤ 0 или процесс
    /// умер) снимается и попытка повторяется.
    ///
    /// Отличие от Python: без `psutil` не проверяем **имя/командную строку** процесса
    /// (там `_lock_pid_is_watcher`), а считаем живой PID владельцем — пессимистично,
    /// как Python без `psutil` (`return True`). Дубль watcher'а опаснее пропуска старта.
    pub fn acquire(project_root: &Path, attempts: u32) -> Result<Option<WatchLock>> {
        let lock = project_root.join("watch.lock");
        for _ in 0..attempts.max(1) {
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let _ = write!(f, "{}", std::process::id());
                    return Ok(Some(WatchLock { path: lock }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !lock_is_stale(&lock) {
                        return Ok(None); // уже работает
                    }
                    let _ = std::fs::remove_file(&lock); // lock от умершего watcher'а
                    continue;
                }
                Err(_) => return Ok(None), // нет прав/ФС чудит — не рискуем вторым watcher'ом
            }
        }
        Ok(None)
    }

    /// Путь lock-файла.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Порт `_remove_lock`.
    pub fn release(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        self.release();
    }
}

/// Порт `_lock_is_stale`: lock оставлен умершим/пустым процессом.
pub fn lock_is_stale(lock: &Path) -> bool {
    let pid: u32 = std::fs::read_to_string(lock)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    pid == 0 || !pid_alive(pid)
}

/// Разбор `FILE_NOTIFY_INFORMATION` из буфера `ReadDirectoryChangesW`.
///
/// `Action`: 1=ADDED, 2=REMOVED, 3=MODIFIED, 4=RENAMED_OLD, 5=RENAMED_NEW.
/// Переименование приходит двумя записями; `pending_old` переносит «старое» имя
/// между записями и вызовами.
pub fn parse_notifications(
    buf: &[u8],
    root: &Path,
    pending_old: &mut Option<PathBuf>,
) -> Vec<WatchEvent> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 12 <= buf.len() {
        let next = u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
        let action = u32::from_le_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]]);
        let len =
            u32::from_le_bytes([buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11]]) as usize;
        let name_start = off + 12;
        if name_start + len > buf.len() {
            break;
        }
        let units: Vec<u16> = (0..len / 2)
            .map(|i| u16::from_le_bytes([buf[name_start + 2 * i], buf[name_start + 2 * i + 1]]))
            .collect();
        let name = String::from_utf16_lossy(&units);
        let path = root.join(name);
        match action {
            1 | 3 => out.push(WatchEvent::Modified(path)),
            2 => out.push(WatchEvent::Deleted(path)),
            4 => *pending_old = Some(path),
            5 => {
                if let Some(old) = pending_old.take() {
                    out.push(WatchEvent::Moved(old, path));
                }
            }
            _ => {}
        }
        if next == 0 {
            break;
        }
        off += next as usize;
    }
    out
}

#[cfg(windows)]
mod win_watch {
    use std::os::raw::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    use super::{parse_notifications, WatchEvent};

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            lp_file_name: *const u16,
            access: u32,
            share: u32,
            sa: *mut c_void,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> *mut c_void;
        fn ReadDirectoryChangesW(
            handle: *mut c_void,
            buffer: *mut c_void,
            len: u32,
            subtree: i32,
            filter: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
            routine: *mut c_void,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    const FILE_LIST_DIRECTORY: u32 = 0x0001;
    const FILE_SHARE_ALL: u32 = 0x0007; // READ|WRITE|DELETE
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    // FILE_NOTIFY_CHANGE_FILE_NAME|DIR_NAME|SIZE|LAST_WRITE
    const NOTIFY_FILTER: u32 = 0x0001 | 0x0002 | 0x0008 | 0x0010;

    /// Поток `ReadDirectoryChangesW` по одному корню (рекурсивно).
    ///
    /// Поток не присоединяем: при выходе процесса он умирает вместе с ним
    /// (в Python — daemon-поток Observer).
    pub fn spawn_root(root: &std::path::Path, tx: Sender<WatchEvent>, stop: Arc<AtomicBool>) {
        let root: PathBuf = root.to_path_buf();
        std::thread::spawn(move || {
            let wide: Vec<u16> = root
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_ALL,
                    std::ptr::null_mut(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    std::ptr::null_mut(),
                )
            };
            if handle.is_null() || handle == (-1isize as *mut c_void) {
                return;
            }
            let mut buf = vec![0u8; 64 * 1024];
            let mut pending_old: Option<PathBuf> = None;
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let mut returned: u32 = 0;
                let ok = unsafe {
                    ReadDirectoryChangesW(
                        handle,
                        buf.as_mut_ptr() as *mut c_void,
                        buf.len() as u32,
                        1,
                        NOTIFY_FILTER,
                        &mut returned,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    break;
                }
                for ev in parse_notifications(&buf[..returned as usize], &root, &mut pending_old) {
                    if tx.send(ev).is_err() {
                        break;
                    }
                }
            }
            unsafe {
                CloseHandle(handle);
            }
        });
    }
}

/// Запуск backend'а для корня: Windows — `ReadDirectoryChangesW`, иначе — опрос.
pub fn spawn_root_watcher(root: &Path, tx: Sender<WatchEvent>, stop: Arc<AtomicBool>) {
    #[cfg(windows)]
    {
        win_watch::spawn_root(root, tx, stop);
    }
    #[cfg(not(windows))]
    {
        poll_watch::spawn_root(root, tx, stop);
    }
}

/// Абсолютный путь (`os.path.abspath`).
fn abs_path(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Порт `path_excluded`: корзина/системные каталоги и `exclude_paths`.
pub fn is_excluded(cfg: &Config, path: &Path) -> bool {
    let (excludes, _limits) = pipeline::index_filter(cfg);
    excludes.excludes_path(path)
}

/// Порт `_bump`: счётчики обработанных/ошибок.
fn bump(state: &mut WatchState, status: &str) {
    state.processed += 1;
    if status.starts_with("error") {
        state.errors += 1;
    }
}

/// Порт тела `_worker`: обработка одного события очереди.
///
/// Возвращает статус-строку (для логов/тестов) либо `None`, если событие пропущено.
pub fn handle_event(
    conn: &rusqlite::Connection,
    cfg: &Config,
    emb: &Embedder,
    extractor: &dyn Extractor,
    lemmatizer: &dyn Lemmatizer,
    event: &WatchEvent,
    state: &mut WatchState,
) -> Result<Option<String>> {
    let debounce = dig(cfg, "watch.debounce_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(8);
    let max_wait = dig(cfg, "watch.max_stable_wait")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);
    match event {
        WatchEvent::Modified(p) => {
            let path = abs_path(p);
            if path
                .extension()
                .map(|e| e.eq_ignore_ascii_case("tmp"))
                .unwrap_or(false)
            {
                return Ok(None);
            }
            if is_excluded(cfg, &path) {
                return Ok(None); // корзина/системные каталоги
            }
            if wait_stable(&path, debounce, max_wait) && path.exists() {
                let (status, _k) = pipeline::process_file(
                    conn, cfg, emb, &path, false, extractor, lemmatizer, None,
                )?;
                println!("[watch] {} -> {}", path.display(), status);
                bump(state, &status);
                return Ok(Some(status));
            }
            Ok(None)
        }
        WatchEvent::Deleted(p) => {
            let path = abs_path(p);
            if db::remove_path(conn, &pipeline::path_str(&path))? {
                bump(state, "removed_from_index");
                return Ok(Some("removed_from_index".into()));
            }
            Ok(None)
        }
        WatchEvent::Moved(src, dst) => {
            let src = abs_path(src);
            let dst = abs_path(dst);
            let dst_excluded = is_excluded(cfg, &dst);
            let src_s = pipeline::path_str(&src);
            let dst_s = pipeline::path_str(&dst);
            if db::get_file_by_path(conn, &src_s)?.is_some() {
                if dst_excluded {
                    db::remove_path(conn, &src_s)?; // файл ушёл в корзину — из индекса
                } else {
                    // перезапись: старая запись dst упала бы на UNIQUE(path) при rename
                    if db::get_file_by_path(conn, &dst_s)?.is_some() {
                        db::remove_path(conn, &dst_s)?;
                    }
                    db::rename_path(conn, &src_s, &dst_s)?;
                }
                state.processed += 1;
                state.moved += 1;
                return Ok(Some("moved".into()));
            } else if !dst_excluded && dst.exists() {
                let (status, _k) = pipeline::process_file(
                    conn, cfg, emb, &dst, false, extractor, lemmatizer, None,
                )?;
                bump(state, &status);
                return Ok(Some(status));
            }
            Ok(None)
        }
    }
}

/// Порт `run_watch`: наблюдатель + worker + reconcile при старте.
///
/// Остановка: файл `index.stop` (как у индексатора) или завершение процесса
/// (Ctrl+C); при жёстком убийстве `watch.lock` остаётся, но следующий старт
/// снимает его как устаревший (`lock_is_stale`).
pub fn run_watch(
    conn: &rusqlite::Connection,
    cfg: &Config,
    emb: &Embedder,
    extractor: &dyn Extractor,
    lemmatizer: &dyn Lemmatizer,
    roots: Option<Vec<PathBuf>>,
) -> Result<i32> {
    let proj = project_root();
    let roots: Vec<PathBuf> = match roots {
        Some(r) => r,
        None => pipeline::string_list(cfg, "index.roots")
            .iter()
            .map(PathBuf::from)
            .collect(),
    };
    if roots.is_empty() {
        println!("Не заданы index.roots в config.yaml");
        return Ok(1);
    }
    let lock = match WatchLock::acquire(&proj, 3)? {
        Some(l) => l,
        None => {
            println!("[watch] Наблюдатель уже запущен (watch.lock). Выход.");
            return Ok(0);
        }
    };

    // наблюдатель включаем ДО reconcile: события во время долгого обхода копятся
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    for r in &roots {
        let r = abs_path(r);
        if r.is_dir() {
            spawn_root_watcher(&r, tx.clone(), Arc::clone(&stop));
            println!("[watch] наблюдаю: {}", r.display());
        }
    }

    if dig(cfg, "watch.reconcile_on_start")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
    {
        println!("[watch] Сверка индекса с дисками (быстрый stat-обход)...");
        let args = pipeline::RunIndexArgs {
            roots: Some(roots.clone()),
            prune: true,
            quiet: true,
            ..Default::default()
        };
        if let Err(e) = pipeline::run_index(conn, cfg, emb, extractor, lemmatizer, &args) {
            // сверка не должна убивать наблюдателя: события ФС важнее
            eprintln!("[watch] ошибка сверки (наблюдение продолжается): {}", e.message());
        }
    }

    println!("[watch] Готово. События обрабатываются автоматически. Ctrl+C — остановка.");
    let stop_file = proj.join("index.stop");
    let mut state = WatchState::default();
    loop {
        if stop_file.exists() {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ev) => {
                state.last_event = Some(epoch_now());
                if let Err(e) = handle_event(conn, cfg, emb, extractor, lemmatizer, &ev, &mut state)
                {
                    state.errors += 1;
                    eprintln!("[watch] ошибка: {}", e.message());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    stop.store(true, Ordering::Relaxed);
    drop(lock); // снять watch.lock
    println!(
        "[watch] Остановлен. Обработано событий: {}, ошибок: {}",
        state.processed, state.errors
    );
    Ok(0)
}

/// Опрос-фallback для не-Windows (macOS-ветка — вне DoD W2, `MIGRATION_PLAN_RUST.md` §10.0).
#[cfg(not(windows))]
mod poll_watch {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;
    use std::time::Duration;

    use super::WatchEvent;

    fn scan(root: &Path, seen: &mut HashMap<PathBuf, (u64, u64)>) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        let mut now = HashMap::new();
        for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path().to_path_buf();
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let sig = (md.len(), mtime);
            if seen.get(&p) != Some(&sig) {
                events.push(WatchEvent::Modified(p.clone()));
            }
            now.insert(p, sig);
        }
        for p in seen.keys() {
            if !now.contains_key(p) {
                events.push(WatchEvent::Deleted(p.clone()));
            }
        }
        *seen = now;
        events
    }

    pub fn spawn_root(root: &Path, tx: Sender<WatchEvent>, stop: Arc<AtomicBool>) {
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut seen = HashMap::new();
            while !stop.load(Ordering::Relaxed) {
                for ev in scan(&root, &mut seen) {
                    if tx.send(ev).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    }
}