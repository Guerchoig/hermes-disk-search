//! `hds-ui` — минимальный веб-интерфейс на Rust (`MIGRATION_PLAN_RUST.md` §4.1 W1):
//! статус (индекс + роли `llm-host`), поиск, RAG-вопрос, управление индексацией.
//!
//! Это **перепроектированный** UI под Rust-стек (не 1:1-порт `hds/ui_server.py`:
//! тот обслуживал Python-операционку — `llama_server`, скачивание/смену моделей,
//! правку конфига; после перехода на `llm-host` это неактуально). Роли читаются по
//! HTTP из фасада `llm-host` (`/internal/status`).

#![forbid(unsafe_code)]

pub mod config_edit;
pub mod page;
pub mod transcribe;
pub mod tree;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};

use hds_core::config::{db_abs_path, dig, load, project_root, Config};
use hds_core::{db, http};
use hds_index::heartbeat::{self, HeartbeatFile};
use hds_index::{Embedder, Sidecar};
use serde_json::{json, Value};

/// Запущена ли фоновая индексация этим UI-процессом.
static INDEXING: AtomicBool = AtomicBool::new(false);

/// Ответ маршрута: `(статус, content-type, тело)`.
pub type Reply = (u16, &'static str, String);

fn json_ok(v: Value) -> Reply {
    (200, "application/json", v.to_string())
}

/// Простое декодирование `%XX` и `+` (для query-параметров).
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Список строк из JSON-тела по ключу (`{"roots": [...]}`).
fn body_list(body: &str, key: &str) -> Vec<String> {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get(key).and_then(|a| a.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Значение query-параметра (`?a=b&c=d`).
pub fn qp(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(pct_decode(v));
            }
        } else if pair == key {
            return Some(String::new());
        }
    }
    None
}

fn connect(cfg: &Config) -> Result<rusqlite::Connection, String> {
    let dim = dig(cfg, "embedding.dim")
        .and_then(|v| v.as_i64())
        .unwrap_or(1024);
    db::connect(&db_abs_path(cfg), dim).map_err(|e| e.message())
}

fn sidecar() -> Result<Sidecar, String> {
    let root = project_root();
    let py = hds_extract::discover_python(&root)
        .ok_or_else(|| format!("не найден интерпретатор воркера в {}", root.display()))?;
    Sidecar::spawn(&py, &root, false).map_err(|e| e.message())
}

/// Индексация сейчас идёт? (heartbeat или фоновой прогон этого процесса).
pub fn index_running() -> bool {
    INDEXING.load(Ordering::Relaxed)
        || heartbeat::index_running(&HeartbeatFile::new(&project_root()), 30.0)
}

/// `/api/status`: состояние индекса + роли `llm-host` (из фасада `/internal/status`).
pub fn status_json() -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "error": e.message() }),
    };
    let mut index = json!({ "running": index_running(), "paused": project_root().join("index.pause").exists() });
    if let Ok(conn) = connect(&cfg) {
        if let Ok(st) = db::stats(&conn) {
            index["chunks"] = json!(st.chunks);
            index["statuses"] = json!(st
                .by_status
                .iter()
                .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_default(), n))
                .collect::<Vec<_>>()
                .join(", "));
            index["kinds"] = json!(st
                .by_kind
                .iter()
                .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_default(), n))
                .collect::<Vec<_>>()
                .join(", "));
            index["errors"] = json!(st.errors.len());
        }
    }
    json!({ "index": index, "llm_host": llm_host_status(&cfg) })
}

/// Заголовки запроса, важные для API (CSRF и тела).
#[derive(Debug, Default, Clone)]
pub struct ReqHeaders {
    pub origin: Option<String>,
    pub content_type: Option<String>,
    pub x_hds_ui: Option<String>,
}

/// CSRF-защита POST (аналог `_csrf_ok`): Origin пуст или loopback **и**
/// (`Content-Type: application/json` **или** заголовок `X-HDS-UI: 1`).
fn csrf_ok(h: &ReqHeaders) -> bool {
    let origin_ok = match h.origin.as_deref() {
        None | Some("") => true,
        Some(o) => {
            let lo = o.to_lowercase();
            lo.starts_with("http://127.0.0.1") || lo.starts_with("http://localhost")
        }
    };
    let ctype_ok = h
        .content_type
        .as_deref()
        .map(|c| {
            c.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        })
        .unwrap_or(false);
    let marker_ok = h.x_hds_ui.as_deref() == Some("1");
    origin_ok && (ctype_ok || marker_ok)
}

/// `/api/diagnostics`: полный `hds check` (общий код `hds_index::diag::run_checks`,
/// тот же, что у `hds check`) плюс состояние резидентного `llm-host`
/// (владельца портов 8010–8012).
pub fn diagnostics_json() -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            let checks = vec![
                json!({ "id": "config", "status": "fail", "title": "config.yaml",
                                       "msg": e.message() }),
            ];
            return json!({ "checks": checks, "ok": false, "config": config_path_str() });
        }
    };

    let mut checks: Vec<Value> = hds_index::diag::run_checks(&cfg)
        .iter()
        .map(|c| json!({ "id": c.id, "status": c.status, "title": c.title, "msg": c.msg, "fix": c.fix }))
        .collect();

    let lh = llm_host_status(&cfg);
    checks.push(json!({
        "id": "llm_host",
        "status": if lh["up"].as_bool() == Some(true) { "ok" } else { "warn" },
        "title": "llm-host",
        "msg": if lh["up"].as_bool() == Some(true) { format!("up (pid {})", lh["pid"]) } else { "не отвечает".to_string() }
    }));

    let ok = checks.iter().all(|c| c["status"] != "fail");
    json!({ "checks": checks, "ok": ok, "config": config_path_str() })
}

/// Путь конфига (для диагностики).
fn config_path_str() -> String {
    hds_core::config::config_path().display().to_string()
}

fn llm_host_status(cfg: &Config) -> Value {
    let base = dig(cfg, "chat.base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://127.0.0.1:8010/v1");
    let (host, port, _) = match hds_index::embed::split_base(base) {
        Ok(x) => x,
        Err(_) => return json!({ "up": false }),
    };
    match http::request(
        &host,
        port,
        "GET",
        "/internal/status",
        &[],
        None,
        std::time::Duration::from_secs(3),
    ) {
        Ok(r) if r.status == 200 => {
            let v = r.json().unwrap_or_else(|_| json!({}));
            let roles = v
                .get("lines")
                .and_then(|l| l.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str())
                        .filter(|s| {
                            s.contains("chat") || s.contains("embedding") || s.contains("rerank")
                        })
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default();
            json!({
                "up": true,
                "mode": v.get("mode").cloned().unwrap_or(Value::Null),
                "pid": v.get("pid").cloned().unwrap_or(Value::Null),
                "roles": roles,
            })
        }
        _ => json!({ "up": false }),
    }
}

/// `/api/search?q=&limit=&kinds=`.
pub fn search_json(query: &str, limit: i64, kinds: &str) -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "error": e.message() }),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return json!({ "error": e }),
    };
    let emb = Embedder::from_config(&cfg);
    let side = match sidecar() {
        Ok(s) => s,
        Err(e) => return json!({ "error": e }),
    };
    let kinds_v: Option<Vec<String>> = {
        let v: Vec<String> = kinds
            .split(',')
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    };
    let lim = limit.clamp(1, 30) as usize;
    let res = hds_search::search(
        &conn,
        Some(&emb),
        &side,
        &cfg,
        query,
        kinds_v.as_deref(),
        lim,
    );
    side.shutdown();
    Value::Array(
        res.iter()
            .map(|r| {
                let mut v = r.to_json();
                v["location"] = json!(r.location());
                v
            })
            .collect(),
    )
}

/// `/api/ask?q=`.
pub fn ask_json(question: &str) -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "error": e.message() }),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return json!({ "error": e }),
    };
    let emb = Embedder::from_config(&cfg);
    let side = match sidecar() {
        Ok(s) => s,
        Err(e) => return json!({ "error": e }),
    };
    let out = hds_search::ask(&conn, Some(&emb), &side, &cfg, question, 8);
    side.shutdown();
    out.to_json()
}

/// Управление индексацией (`start`/`stop`/`pause`/`resume`).
pub fn index_action(action: &str, full: bool) -> Value {
    let root = project_root();
    match action {
        "stop" => {
            let _ = std::fs::write(root.join("index.stop"), b"");
            json!({ "ok": true, "msg": "сигнал остановки отправлен" })
        }
        "pause" => {
            let _ = std::fs::write(root.join("index.pause"), b"");
            json!({ "ok": true, "msg": "индексация на паузе" })
        }
        "resume" => {
            let _ = std::fs::remove_file(root.join("index.pause"));
            json!({ "ok": true, "msg": "пауза снята" })
        }
        "start" => {
            if index_running() {
                return json!({ "ok": false, "msg": "индексация уже идёт" });
            }
            INDEXING.store(true, Ordering::Relaxed);
            std::thread::spawn(move || {
                let _ = run_index_bg(full);
                INDEXING.store(false, Ordering::Relaxed);
            });
            json!({ "ok": true, "msg": if full { "запущена полная индексация" } else { "запущена инкрементальная индексация" } })
        }
        other => json!({ "ok": false, "msg": format!("неизвестное действие: {other}") }),
    }
}

fn run_index_bg(full: bool) -> Result<(), String> {
    let cfg = load().map_err(|e| e.message())?;
    let conn = connect(&cfg)?;
    let emb = Embedder::from_config(&cfg);
    let stop = project_root().join("index.stop");
    if stop.exists() {
        let _ = std::fs::remove_file(&stop);
    }
    let side = sidecar()?;
    let media = hds_index::transcribe::MediaRouter::new(&side, &cfg);
    let args = hds_index::RunIndexArgs {
        full,
        prune: true,
        quiet: true,
        ..Default::default()
    };
    let r = hds_index::pipeline::run_index(&conn, &cfg, &emb, &media, &side, &args);
    side.shutdown();
    r.map(|_| ()).map_err(|e| e.message())
}

/// Базовый `(host, port)` фасада `llm-host` из `chat.base_url`.
fn llm_host_addr(cfg: &Config) -> Option<(String, u16)> {
    let base = dig(cfg, "chat.base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://127.0.0.1:8010/v1");
    hds_index::embed::split_base(base)
        .ok()
        .map(|(h, p, _)| (h, p))
}

fn llm_host_healthy(host: &str, port: u16, timeout: std::time::Duration) -> bool {
    matches!(
        http::request(host, port, "GET", "/health", &[], None, timeout),
        Ok(r) if (200..300).contains(&r.status)
    )
}

/// Найти исполняемый файл по списку имён: рядом с текущим exe, затем `bin/`,
/// `target/release`, `target/debug` (dev-дерево).
pub(crate) fn find_exe(names: &[&str]) -> Option<std::path::PathBuf> {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(cur) = std::env::current_exe() {
        if let Some(d) = cur.parent() {
            dirs.push(d.to_path_buf());
        }
    }
    let root = project_root();
    dirs.push(root.join("bin"));
    dirs.push(root.join("target").join("release"));
    dirs.push(root.join("target").join("debug"));
    for d in dirs {
        for n in names {
            let c = d.join(n);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// Путь к бинарю резидента `llm_host(.exe)`: рядом с текущим `hds`, иначе
/// `bin/` → `target/release/` → `target/debug/` (dev-дерево).
fn llm_host_exe() -> Option<std::path::PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["llm_host.exe", "llm_host"]
    } else {
        &["llm_host", "llm_host.exe"]
    };
    find_exe(names)
}

/// Запустить процесс **detached** (без окна, без наследования stdio) — общий путь для
/// резидента `llm-host` и обоих демонов (автотранскрибации и индексации).
pub(crate) fn spawn_detached(
    exe: &std::path::Path,
    args: &[&str],
    cwd: &std::path::Path,
) -> Result<u32, String> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|c| c.id()).map_err(|e| e.to_string())
}

/// Дождаться нужного состояния файла-сигнала — не дольше `secs` секунд.
pub(crate) fn wait_file(path: &std::path::Path, want_present: bool, secs: u64) -> bool {
    for _ in 0..secs.saturating_mul(4).max(1) {
        if path.exists() == want_present {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    path.exists() == want_present
}

/// Путь к бинарю `hds(.exe)`: UI обычно запущен внутри него (`hds ui`), поэтому
/// сначала проверяем текущий процесс, затем — поиск рядом / `bin` / `target`.
pub(crate) fn hds_exe() -> Option<std::path::PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["hds.exe", "hds"]
    } else {
        &["hds", "hds.exe"]
    };
    if let Ok(cur) = std::env::current_exe() {
        if let Some(n) = cur.file_name().and_then(|n| n.to_str()) {
            if names.contains(&n) {
                return Some(cur);
            }
        }
    }
    find_exe(names)
}

// --- Демон индексации (`hds watch`): статус и управление из UI -------------------

/// Статус демона индексации: `watch.lock` (с распознаванием устаревшего) плюс
/// состояние самой индексации по heartbeat / `index.pause` / `index.stop`.
pub fn watch_status() -> Value {
    let root = project_root();
    let lock = root.join("watch.lock");
    let stale = lock.exists() && hds_index::watch::lock_is_stale(&lock);
    let running = lock.exists() && !stale;
    let pid = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|_| running);
    let cfg = load().unwrap_or(Config::Null);
    let roots: Vec<String> = dig(&cfg, "index.roots")
        .and_then(|v| v.as_sequence())
        .map(|s| {
            s.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    json!({
        "running": running,
        "stale": stale,
        "pid": pid,
        "lock": lock.display().to_string(),
        "stop_requested": root.join("index.stop").exists(),
        "paused": root.join("index.pause").exists(),
        "indexing": index_running(),
        "roots": roots,
    })
}

/// Мягкая остановка демона индексации: пишем `index.stop` и ждём ухода процесса.
///
/// `index.stop` проверяется в конвейере **перед каждым файлом** (`pipeline::run_index`),
/// а до цикла идёт обход дерева (`collect_files`) — поэтому в худшем случае watcher
/// уходит не мгновенно. Поведение:
/// * процесс уже не запущен → `ok`, `pending: false`;
/// * вышел за отведённые секунды → `ok`, `pending: false`, «остановлен»;
/// * ещё не вышел → тоже `ok` (файл-сигнал уже стоит, выход произойдёт на ближайшей
///   безопасной точке), но `pending: true` — UI показывает это честно.
fn stop_watch(lock: &std::path::Path, stop: &std::path::Path, secs: u64) -> Value {
    if !watch_status()["running"].as_bool().unwrap_or(false) {
        let _ = std::fs::remove_file(stop);
        return json!({ "ok": true, "pending": false, "msg": "демон индексации не запущен",
            "status": watch_status() });
    }
    if let Err(e) = std::fs::write(stop, b"") {
        return json!({ "ok": false, "pending": false,
            "msg": format!("не создать {}: {e}", stop.display()) });
    }
    if wait_file(lock, false, secs) {
        return json!({ "ok": true, "pending": false, "msg": "демон индексации остановлен",
            "status": watch_status() });
    }
    let busy = watch_status()["indexing"].as_bool().unwrap_or(false);
    json!({ "ok": true, "pending": true,
        "msg": if busy {
            "остановка запрошена: идёт индексация — watcher завершится сразу после текущего \
             файла (index.stop уже стоит)".to_string()
        } else {
            format!("остановка запрошена (index.stop): watcher завершится на ближайшей проверке \
                     — за {secs} с не успел (идёт обход дерева)")
        },
        "status": watch_status() })
}

/// Управление демоном индексации из UI: `start` | `stop` | `restart` | `status`.
///
/// Симметрично демону автотранскрибации (`transcribe::daemon_json`):
/// * запуск — detached `<hds.exe> watch` (дубль невозможен: `watch.lock`); при старте
///   снимаем `index.stop` (иначе новый watcher завершится сразу) и устаревший lock;
/// * остановка — мягкая ([`stop_watch`]): `index.stop`, честный `pending`, если процесс
///   ещё занят (задача Планировщика такой выход **не** перезапускает).
pub fn watch_json(action: &str) -> Value {
    let root = project_root();
    let lock = root.join("watch.lock");
    let stop = root.join("index.stop");
    match action.trim() {
        "" | "status" => watch_status(),
        "stop" => stop_watch(&lock, &stop, 15),
        "start" => {
            let st = watch_status();
            if st["running"].as_bool().unwrap_or(false) {
                return json!({ "ok": false, "msg": "демон индексации уже запущен", "status": st });
            }
            if st["roots"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
                return json!({ "ok": false,
                    "msg": "в config.yaml не заданы index.roots — watcher выйдет сразу",
                    "status": st });
            }
            let _ = std::fs::remove_file(&lock); // устаревший lock не мешает старту
            let _ = std::fs::remove_file(&stop); // иначе новый watcher завершится сразу
            let Some(exe) = hds_exe() else {
                return json!({ "ok": false,
                    "msg": "не найден бинарь hds (рядом с UI / bin / target/{release,debug})" });
            };
            if let Err(e) = spawn_detached(&exe, &["watch"], &root) {
                return json!({ "ok": false, "msg": format!("не запустить {}: {e}", exe.display()) });
            }
            if !wait_file(&lock, true, 15) {
                return json!({ "ok": false,
                    "msg": "watcher не занял watch.lock за 15 с — проверьте index.roots \
                            и доступность дисков",
                    "status": watch_status() });
            }
            json!({ "ok": true, "msg": "демон индексации запущен", "status": watch_status() })
        }
        "restart" => {
            // для перезапуска ждём дольше: законный случай — длинный обход/индексация
            let stopped = stop_watch(&lock, &stop, 45);
            if !stopped["ok"].as_bool().unwrap_or(false) {
                return stopped;
            }
            if stopped["pending"].as_bool().unwrap_or(false) {
                return json!({ "ok": false,
                    "msg": "текущий watcher ещё завершается (index.stop стоит) — повторите \
                            перезапуск чуть позже",
                    "status": watch_status() });
            }
            let mut res = watch_json("start");
            res["msg"] = json!(format!(
                "перезапуск: {}",
                res["msg"].as_str().unwrap_or("готово")
            ));
            res
        }
        other => json!({ "ok": false,
            "msg": format!("неизвестное действие: {other} (start|stop|restart|status)") }),
    }
}

/// `POST /api/llm-host/restart`: остановить резидент `llm-host` и поднять заново.
///
/// Свой процесс нельзя перезапустить «сам в себе», но резидент — отдельный, поэтому
/// UI (`hds ui`): (1) просит фасад `:8010` завершиться (`/internal/stop`),
/// (2) ждёт освобождения портов и pid-файла, (3) запускает `llm_host run` заново
/// (detached, без окна) и (4) ждёт `/health`. Модель грузится лениво — первый ответ
/// после перезапуска будет с задержкой.
pub fn llm_host_restart() -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "ok": false, "msg": e.message() }),
    };
    let (host, port) = match llm_host_addr(&cfg) {
        Some(a) => a,
        None => return json!({ "ok": false, "msg": "не разобрать chat.base_url" }),
    };
    let exe = match llm_host_exe() {
        Some(p) => p,
        None => {
            return json!({ "ok": false,
                "msg": "не найден бинарь llm_host (рядом с hds / bin / target/{release,debug})" })
        }
    };
    let root = project_root();
    let pid_file = root.join("data").join("llm-host.pid");

    // 1. Остановка, если резидент отвечает (или остался pid-файл).
    let was_up = llm_host_healthy(&host, port, std::time::Duration::from_secs(2));
    if was_up || pid_file.exists() {
        let _ = http::request(
            &host,
            port,
            "POST",
            "/internal/stop",
            &[],
            None,
            std::time::Duration::from_secs(10),
        );
    }
    // 2. Ждём остановки: порт не отвечает И pid-файл снят (макс ~60 c).
    let mut stopped = !was_up && !pid_file.exists();
    for _ in 0..120 {
        if stopped {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        stopped =
            !pid_file.exists() && !llm_host_healthy(&host, port, std::time::Duration::from_secs(1));
    }
    if !stopped {
        return json!({ "ok": false,
            "msg": "резидент не завершился за 60 с — перезапуск отменён (см. data/logs/llm-host.log)" });
    }

    // 3. Запуск заново (detached, без окна) — общий хелпер `spawn_detached`.
    let pid = match spawn_detached(&exe, &["run"], &root) {
        Ok(p) => p,
        Err(e) => {
            return json!({ "ok": false, "msg": format!("не запустить {}: {e}", exe.display()) })
        }
    };

    // 4. Ждём готовности фасада (макс ~180 c: резидент поднимает роли).
    let mut up = false;
    for _ in 0..360 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if llm_host_healthy(&host, port, std::time::Duration::from_secs(1)) {
            up = true;
            break;
        }
    }
    if up {
        json!({ "ok": true, "msg": format!("llm-host перезапущен (pid {pid})"), "pid": pid })
    } else {
        json!({ "ok": false,
            "msg": format!("llm-host запущен (pid {pid}), но /health не ответил за 180 с — см. data/logs/llm-host.log") })
    }
}

/// `POST /api/cline/sync`: привести настройки Cline под `config.yaml` — окна
/// контекста моделей (`~/.cline/data/settings/models.json`), MCP-сервер
/// (`cline_mcp_settings.json`, `mcp.json`), правило и скилл disk-search.
///
/// Логика одна на UI, CLI (`hds cline-sync`) и инсталляторы — `hds_core::cline::sync`.
pub fn cline_sync_json() -> Value {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "ok": false, "error": e.message() }),
    };
    hds_core::cline::sync(&cfg, &project_root(), false).to_json()
}

/// Маршрутизация (чистая функция — тестируется без сокетов).
///
/// `path` — без query; `query` — часть после `?`; `h` — заголовки; `body` — тело POST.
pub fn route(method: &str, path: &str, query: &str, h: &ReqHeaders, body: &str) -> Reply {
    let p = path.trim_end_matches('/');
    match (method, p) {
        ("GET", "") | ("GET", "/index.html") => {
            (200, "text/html; charset=utf-8", page::PAGE.to_string())
        }
        ("GET", "/api/status") => json_ok(status_json()),
        ("GET", "/api/tree") => json_ok(tree::tree_json(
            qp(query, "walk").is_some(),
            qp(query, "refresh").is_some(),
        )),
        ("GET", "/api/diagnostics") => json_ok(diagnostics_json()),
        ("GET", "/api/watch/daemon") => json_ok(watch_status()),
        ("GET", "/api/transcribe/daemon") => json_ok(transcribe::daemon_status()),
        ("GET", "/api/transcribe/list") => json_ok(transcribe::list_json()),
        ("GET", "/api/transcribe/file") => {
            let name = qp(query, "name").unwrap_or_default();
            if name.trim().is_empty() {
                return (
                    400,
                    "application/json",
                    json!({ "error": "нет параметра name" }).to_string(),
                );
            }
            json_ok(transcribe::file_json(&name))
        }
        ("GET", "/api/config") => json_ok(config_edit::get_config()),
        ("GET", "/api/search") => {
            let q = qp(query, "q").unwrap_or_default();
            if q.trim().is_empty() {
                return (
                    400,
                    "application/json",
                    json!({ "error": "нет параметра q" }).to_string(),
                );
            }
            let limit = qp(query, "limit").and_then(|v| v.parse().ok()).unwrap_or(8);
            let kinds = qp(query, "kinds").unwrap_or_default();
            json_ok(search_json(&q, limit, &kinds))
        }
        ("GET", "/api/ask") => {
            let q = qp(query, "q").unwrap_or_default();
            if q.trim().is_empty() {
                return (
                    400,
                    "application/json",
                    json!({ "error": "нет параметра q" }).to_string(),
                );
            }
            json_ok(ask_json(&q))
        }
        ("POST", _) => {
            if !csrf_ok(h) {
                return (
                    403,
                    "application/json",
                    json!({ "error": "cross-origin/тип запроса отклонён" }).to_string(),
                );
            }
            match p {
                "/api/index/start" => {
                    let full = qp(query, "full")
                        .map(|v| v == "1" || v == "true")
                        .unwrap_or(false);
                    json_ok(index_action("start", full))
                }
                "/api/index/stop" => json_ok(index_action("stop", false)),
                "/api/index/pause" => json_ok(index_action("pause", false)),
                "/api/index/resume" => json_ok(index_action("resume", false)),
                "/api/roots/save" => {
                    let roots = body_list(body, "roots");
                    json_ok(config_edit::set_roots(&roots))
                }
                "/api/llm-host/restart" => json_ok(llm_host_restart()),
                "/api/cline/sync" => json_ok(cline_sync_json()),
                "/api/config/excludes" => {
                    let paths = body_list(body, "paths");
                    json_ok(config_edit::set_exclude_paths(&paths))
                }
                "/api/transcribe/speakers" => {
                    let v = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
                    let name = v
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    if name.trim().is_empty() {
                        return (
                            400,
                            "application/json",
                            json!({ "error": "нет поля name" }).to_string(),
                        );
                    }
                    let names = v.get("names").cloned().unwrap_or_else(|| json!({}));
                    json_ok(transcribe::speakers_json(&name, &names))
                }
                "/api/transcribe/apply" => {
                    let v = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
                    let name = v
                        .get("name")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    if name.trim().is_empty() {
                        return (
                            400,
                            "application/json",
                            json!({ "error": "нет поля name" }).to_string(),
                        );
                    }
                    json_ok(transcribe::apply_json(&name))
                }
                "/api/watch/daemon" => {
                    let v = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
                    let action = v
                        .get("action")
                        .and_then(|x| x.as_str())
                        .unwrap_or("status")
                        .to_string();
                    json_ok(watch_json(&action))
                }
                "/api/transcribe/daemon" => {
                    let v = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
                    let action = v
                        .get("action")
                        .and_then(|x| x.as_str())
                        .unwrap_or("status")
                        .to_string();
                    json_ok(transcribe::daemon_json(&action))
                }
                "/api/config/transcribe-dirs" => {
                    let v = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
                    let inbox = v
                        .get("inbox_dir")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let out = v
                        .get("out_dir")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    json_ok(config_edit::set_transcribe_dirs(&inbox, &out))
                }
                _ => (
                    404,
                    "application/json",
                    json!({ "error": "not found", "path": path }).to_string(),
                ),
            }
        }
        _ => (
            404,
            "application/json",
            json!({ "error": "not found", "path": path }).to_string(),
        ),
    }
}

/// Читать один HTTP/1.1-запрос: `(метод, путь, query, заголовки, тело)`.
fn read_request(stream: &mut TcpStream) -> Option<(String, String, String, ReqHeaders, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 1 << 20 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut parts = lines.next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let mut h = ReqHeaders::default();
    let mut content_length = 0usize;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            match key.as_str() {
                "origin" => h.origin = Some(val),
                "content-type" => h.content_type = Some(val),
                "x-hds-ui" => h.x_hds_ui = Some(val),
                "content-length" => content_length = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Some((
        method,
        path,
        query,
        h,
        String::from_utf8_lossy(&body).into_owned(),
    ))
}

fn write_reply(stream: &mut TcpStream, (status, ctype, body): Reply) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// Запустить веб-интерфейс (блокирующе).
pub fn run_http(host: &str, port: u16) -> Result<(), String> {
    let listener =
        TcpListener::bind((host, port)).map_err(|e| format!("bind {host}:{port}: {e}"))?;
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                std::thread::spawn(move || {
                    if let Some((m, p, q, h, b)) = read_request(&mut stream) {
                        write_reply(&mut stream, route(&m, &p, &q, &h, &b));
                    }
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}
