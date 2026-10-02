//! `hds db-move` — порт `hds/dbops.py::move_db`: остановить watcher/index,
//! сделать консистентную копию БД, сверить счётчики, **текстово** поправить
//! `db_path` в `config.yaml` (комментарии сохраняются) и переименовать старую БД.
//!
//! Осознанные отличия от Python (перечислены и в `W2_REPORT.md` §14):
//! * **Остановка процессов** — без psutil: watcher определяем по `watch.lock`
//!   (там PID) и завершаем `TerminateProcess`; индексацию — кооперативно
//!   (`index.stop` + ожидание, пока heartbeat не устареет). Python жёстко убивал
//!   все `-m hds.cli watch|index` (командную строку без psutil не перебрать).
//! * **Копия** — `VACUUM INTO` вместо `Connection::backup` (у `rusqlite` фича
//!   `backup` не подключена); результат тот же — консистентный снимок БД.
//! * **Guard**: если исходной БД нет — понятная ошибка (в Python пустой файл
//!   создавался молча, а сверка падала с «no such table»).
//! * **Перезапуск watcher** — наш `hds watch` (не `pythonw -m hds.cli watch`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_core::config::{config_path, db_abs_path, project_root, replace_file};
use hds_core::error::Result;
use hds_index::heartbeat::{session_state, HeartbeatFile, SessionState};

/// Параметры переноса (явные пути — для тестов без глобального `HDS_CONFIG`).
pub struct MoveDbArgs {
    pub cfg_path: PathBuf,
    pub old_db: PathBuf,
    pub new_db: PathBuf,
    pub force: bool,
    pub project_root: PathBuf,
    /// Перезапускать watcher, если он был запущен (в тестах выключаем).
    pub restart_watcher: bool,
    /// Сколько ждать кооперативной остановки индексации (`index.stop`).
    pub stop_timeout: Duration,
}

/// Итог `move_db` (как `dict` в Python: `ok`/`msg`/`watch_was_running`).
pub struct MoveResult {
    pub ok: bool,
    pub msg: String,
    pub watch_was_running: bool,
}

impl MoveResult {
    fn fail(msg: String, watch_was: bool) -> MoveResult {
        MoveResult {
            ok: false,
            msg,
            watch_was_running: watch_was,
        }
    }
}

/// `move_db`: этапы 1–6 Python-версии (см. doc модуля).
pub fn move_db(args: &MoveDbArgs) -> MoveResult {
    let old = &args.old_db;
    let new = &args.new_db;
    if new == old {
        return MoveResult::fail(
            format!("Новый путь совпадает с текущим: {}", new.display()),
            false,
        );
    }
    if new.exists() && !args.force {
        return MoveResult::fail(
            format!(
                "Целевой файл уже существует: {} (проверьте путь или включите перезапись)",
                new.display()
            ),
            false,
        );
    }
    if !old.exists() {
        return MoveResult::fail(format!("Исходная БД не найдена: {}", old.display()), false);
    }

    let watch_was = stop_watcher(&args.project_root);
    stop_index(&args.project_root, args.stop_timeout);

    if let Some(dir) = new.parent() {
        if !dir.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(dir);
        }
    }
    if new.exists() {
        let _ = std::fs::remove_file(new); // --force: VACUUM INTO не перезаписывает цель
    }
    println!(
        "[db-move] копирую {} -> {} ...",
        old.display(),
        new.display()
    );
    if let Err(e) = vacuum_into(old, new) {
        return MoveResult::fail(
            format!("Копирование не удалось: {}", e.message()),
            watch_was,
        );
    }

    let c_old = match counts(old) {
        Ok(c) => c,
        Err(e) => return MoveResult::fail(format!("Чтение старой БД: {}", e.message()), watch_was),
    };
    let c_new = match counts(new) {
        Ok(c) => c,
        Err(e) => return MoveResult::fail(format!("Чтение новой БД: {}", e.message()), watch_was),
    };
    if c_old != c_new {
        let _ = std::fs::remove_file(new);
        return MoveResult::fail(
            format!("Проверка не сошлась ({c_old:?} vs {c_new:?}) — откат"),
            watch_was,
        );
    }
    println!("[db-move] ok: файлов {}, чанков {}", c_new.0, c_new.1);

    if let Err(e) = rewrite_db_path(&args.cfg_path, new) {
        return MoveResult::fail(format!("config.yaml: {}", e.message()), watch_was);
    }

    let stamp = stamp_now();
    for suffix in ["", "-wal", "-shm"] {
        let src = PathBuf::from(format!("{}{}", old.display(), suffix));
        if src.exists() {
            let dst = PathBuf::from(format!("{}{}.moved-{}", old.display(), suffix, stamp));
            let _ = std::fs::rename(&src, &dst);
        }
    }

    if watch_was && args.restart_watcher {
        relaunch_watcher(&args.project_root);
    }

    MoveResult {
        ok: true,
        msg: format!(
            "БД перенесена в {} (файлов {}, чанков {}); старая копия: index.db.moved-{}",
            new.display(),
            c_new.0,
            c_new.1,
            stamp
        ),
        watch_was_running: watch_was,
    }
}

/// `cmd_db_move(to, force)`: собирает пути из конфига и печатает результат.
pub fn cmd_db_move(to: &str, force: bool) -> i32 {
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let new = std::path::absolute(to).unwrap_or_else(|_| PathBuf::from(to));
    let args = MoveDbArgs {
        cfg_path: config_path(),
        old_db: db_abs_path(&cfg),
        new_db: new,
        force,
        project_root: project_root(),
        restart_watcher: true,
        stop_timeout: Duration::from_secs(120),
    };
    let res = move_db(&args);
    println!("{}", res.msg);
    if res.ok {
        0
    } else {
        1
    }
}

/// Имя lock-файла watcher (`MIGRATION_PLAN_RUST.md` §3.4).
const WATCH_LOCK: &str = "watch.lock";

/// Консистентная копия средствами SQLite: `VACUUM INTO '<new>'`.
fn vacuum_into(old: &Path, new: &Path) -> Result<()> {
    let conn = rusqlite::Connection::open(old)?;
    let target = new.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{target}'"))?;
    Ok(())
}

/// `(files, chunks)` в БД (порт `dbops.move_db.counts`).
fn counts(path: &Path) -> Result<(i64, i64)> {
    let conn = rusqlite::Connection::open(path)?;
    let files: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let chunks: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    Ok((files, chunks))
}

/// Текстовая правка `db_path` в `config.yaml` (комментарии сохраняются).
///
/// Порт `re.sub(r"(?m)^db_path:.*$", "db_path: '<safe>'", text)` + «дописать, если
/// ключа нет»; запись через `.tmp` и [`replace_file`] (атомарно, с ретраями).
fn rewrite_db_path(cfg_path: &Path, new: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(cfg_path)?;
    let text = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    let safe = new.to_string_lossy().replace('\'', "''");
    let mut out = String::new();
    let mut replaced = false;
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = body.strip_suffix('\r').unwrap_or(body);
        if body.starts_with("db_path:") {
            let nl = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(&format!("db_path: '{safe}'{nl}"));
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    if !replaced {
        out.push_str(&format!("\ndb_path: '{safe}'\n"));
    }
    let tmp = PathBuf::from(format!("{}.tmp", cfg_path.display()));
    std::fs::write(&tmp, out.as_bytes())?;
    replace_file(&tmp, cfg_path)?;
    Ok(())
}

/// Отметка времени для `.moved-<stamp>` (`time.strftime("%Y%m%d-%H%M%S")`).
fn stamp_now() -> String {
    let s = crate::support::fmt_local_datetime(crate::support::now_epoch());
    match s.split_once(' ') {
        Some((d, t)) => {
            let t = t.split('.').next().unwrap_or(t); // без микросекунд, как strftime
            format!("{}-{}", d.replace('-', ""), t.replace(':', ""))
        }
        None => "00000000-000000".to_string(),
    }
}

/// Кооперативная остановка индексации: `index.stop` + ожидание, пока heartbeat устареет.
fn stop_index(root: &Path, timeout: Duration) {
    let hb = HeartbeatFile::new(root);
    if !matches!(session_state(&hb, 30.0), SessionState::Live) {
        return;
    }
    let _ = std::fs::File::create(root.join("index.stop"));
    println!("[db-move] останавливаю индексацию (index.stop) и жду завершения...");
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        if !matches!(session_state(&hb, 30.0), SessionState::Live) {
            break;
        }
    }
}

/// Остановка watcher по PID из `watch.lock` (устаревший lock просто снимаем).
fn stop_watcher(root: &Path) -> bool {
    let lock = root.join(WATCH_LOCK);
    let pid: u32 = match std::fs::read_to_string(&lock) {
        Ok(s) => s.trim().parse().unwrap_or(0),
        Err(_) => return false,
    };
    if pid == 0 || !hds_index::watch::pid_alive(pid) {
        let _ = std::fs::remove_file(&lock);
        return false;
    }
    terminate(pid);
    let _ = std::fs::remove_file(&lock);
    println!("[db-move] останавливаю watcher: 1 шт. (pid {pid})");
    true
}

/// Завершение процесса по PID (Windows — `TerminateProcess`; POSIX — `SIGTERM`).
#[cfg(windows)]
fn terminate(pid: u32) {
    use std::os::raw::c_void;
    const PROCESS_TERMINATE: u32 = 0x0001;
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn TerminateProcess(h: *mut c_void, code: u32) -> i32;
        fn CloseHandle(h: *mut c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !h.is_null() {
            let _ = TerminateProcess(h, 1);
            let _ = CloseHandle(h);
        }
    }
}

/// POSIX-вариант завершения процесса (`kill(SIGTERM)`).
#[cfg(not(windows))]
fn terminate(pid: u32) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        let _ = kill(pid as i32, 15);
    }
}

/// Перезапуск watcher нашим бинарём (`hds watch`), отсоединённо, без окна.
fn relaunch_watcher(root: &Path) {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("watch").current_dir(root);
    // Отвязываем от stdio родителя: иначе фоновый watcher держит пайпы
    // вызывающего (при переадресации вывода родительский процесс «не завершается»).
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    match cmd.spawn() {
        Ok(_) => println!("[db-move] watcher перезапущен"),
        Err(e) => println!("[db-move] не удалось перезапустить watcher: {e}"),
    }
}
