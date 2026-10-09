//! Сокет-тесты очереди тяжёлых RAG (M4 T3, `PLAN_M4_CONCURRENCY_SLA.md`
//! §3.2/§4): коды отказа должны **доходить до клиента** (не `connection reset`,
//! как в регресе F4 из T0), плюс кросс-процессный снапшот очереди
//! (`<state_dir>/data/mcp-queue.json` — его читают `hds check`/`/api/diagnostics`).
//!
//! Детерминизм: единственный слот исполнения «держит» реальный `ask_my_files`,
//! который висит на блокировке БД — песочница открыта в `journal_mode: delete`,
//! а тест удерживает `BEGIN EXCLUSIVE` (в rollback-journal писатель блокирует и
//! читателя). Ни GPU, ни сети, ни Python-воркера тест не требует.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Тесты трогают env (`HDS_CONFIG`/`HDS_ROOT`) — идут последовательно.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Тяжёлый вызов (JSON-RPC `tools/call` + `ask_my_files`) — попадает под очередь.
const HEAVY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ask_my_files","arguments":{"question":"что там с отчётами","limit":1}}}"#;

/// Песочница (`HDS_ROOT`) с каталогом `data/`.
fn tmp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-mcpq-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::env::set_var("HDS_ROOT", &dir);
    dir
}

/// Конфиг песочницы: очередь включена, один слот исполнения; БД в `delete`-режиме
/// (см. модульный комментарий). `wait_sec` — потолок ожидания (снапшот очереди
/// пишется раз в 2 с, поэтому ожидание должно быть длиннее — иначе состояние
/// «запрос ждёт» не попадёт в снапшот).
fn write_config(dir: &Path, max_depth: u32, wait_sec: u64) -> PathBuf {
    let body = format!(
        "profile: server\n\
         access:\n  enabled: false\n\
         db:\n  journal_mode: delete\n  read_only_conns: false\n  busy_timeout_ms: 30000\n\
         queue:\n  enabled: true\n  max_depth: {max_depth}\n  wait_timeout_sec: {wait_sec}\n  policy: rr\n\
         rate_limit:\n  enabled: true\n  requests_per_min: 600\n  burst: 60\n  login_per_min: 30\n  max_inflight: 1\n\
         http:\n  max_body_kb: 512\n  max_conns: 0\n  read_timeout_sec: 30\n\
         db_path: index.db\n\
         embedding:\n  base_url: \"http://127.0.0.1:1/v1\"\n\
         chat:\n  base_url: \"http://127.0.0.1:1/v1\"\n"
    );
    let p = dir.join("config.yaml");
    std::fs::write(&p, body).unwrap();
    p
}

/// Создать БД песочницы и удерживать блокировку писателя (`BEGIN EXCLUSIVE`).
fn lock_db(dir: &Path) -> rusqlite::Connection {
    let opts = hds_core::db::DbOptions {
        busy_timeout_ms: 100,
        journal_mode: "delete".to_string(),
    };
    let conn = hds_core::db::open_rw(&dir.join("index.db"), 1024, &opts).expect("БД песочницы");
    conn.execute_batch("BEGIN EXCLUSIVE").expect("блокировка");
    conn
}

/// Поднять MCP-сервер на порту `0` и вернуть адрес.
fn spawn_server(cfg: &Path) -> SocketAddr {
    std::env::set_var("HDS_CONFIG", cfg);
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        let _ = hds_mcp::run_http_listener(l, "/mcp");
    });
    addr
}

fn heavy_request() -> String {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{HEAVY}",
        HEAVY.len()
    )
}

/// Тяжёлый запрос с чтением ответа.
fn send_heavy(addr: SocketAddr) -> String {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(heavy_request().as_bytes()).expect("write");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp
}

/// Тяжёлый запрос БЕЗ чтения ответа (соединение держим — запрос занимает слот).
fn hold_slot(addr: SocketAddr) -> TcpStream {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.write_all(heavy_request().as_bytes()).expect("write");
    s
}

/// Снапшот очереди (кросс-процессная диагностика).
fn snapshot(dir: &Path) -> Option<serde_json::Value> {
    hds_core::queue::read_snapshot(&dir.join("data").join("mcp-queue.json"))
}

fn body_of(resp: &str) -> String {
    resp.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
}

fn wait_for(what: &str, timeout: Duration, cond: impl Fn() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("не дождались: {what}");
}

/// Занятый слот + ожидающий, который не дождался → `503`; следующий при полной
/// глубине (`max_depth: 1`) → `429`. Оба кода доходят до клиента, метрики — в снапшоте.
#[test]
fn queue_full_and_timeout_codes_reach_client() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmp_dir("q1");
    // Ожидание 4 с (снапшот пишется раз в 2 с) — «запрос ждёт» видно в метриках.
    let cfg = write_config(&dir, 1, 4);
    let lock = lock_db(&dir); // слот будет занят, пока держим писателя БД
    let addr = spawn_server(&cfg);

    // 1) Тяжёлый запрос занял единственный слот (висит на блокировке БД).
    let _held = hold_slot(addr);
    wait_for(
        "executing=1 в снапшоте очереди",
        Duration::from_secs(30),
        || snapshot(&dir).is_some_and(|v| v["executing"] == 1),
    );
    let snap = snapshot(&dir).expect("снапшот очереди пишет MCP-процесс");
    assert_eq!(snap["enabled"], true, "{snap}");
    assert_eq!(snap["max_inflight"], 1, "{snap}");
    assert_eq!(snap["policy"], "rr", "{snap}");
    assert_eq!(snap["wait_timeout_sec"], 4, "{snap}");
    assert!(snap["pid"].as_u64().unwrap_or(0) > 0, "{snap}");
    // «Кто держит слот» — пер-пользовательская картина (ключ `local`: авторизация
    // выключена).
    let per = snap["per_user"].as_array().expect("per_user");
    assert!(
        per.iter().any(|u| u["executing"] == 1),
        "кто исполняет: {snap}"
    );

    // 2) Второй тяжёлый запрос встаёт в очередь (глубина 1) и ждёт слот.
    let waiter = std::thread::spawn(move || send_heavy(addr));
    wait_for(
        "depth=1 в снапшоте (запрос ждёт слот)",
        Duration::from_secs(10),
        || snapshot(&dir).is_some_and(|v| v["depth"] == 1),
    );

    // 3) Третий — очередь полна (`max_depth: 1`) → 429 сразу.
    let full = send_heavy(addr);
    assert!(
        full.starts_with("HTTP/1.1 429"),
        "ожидался 429, получено: {:.200}",
        full
    );
    assert!(full.contains("Retry-After:"), "{full}");
    assert!(full.contains("X-Queue-Depth: 1"), "{full}");
    assert!(
        body_of(&full).contains("queue.max_depth"),
        "hint про глубину: {}",
        body_of(&full)
    );

    // 4) Ожидающий не дождался за 1 с → 503 (не обрыв соединения — регресс F4).
    let r = waiter.join().unwrap();
    assert!(
        r.starts_with("HTTP/1.1 503"),
        "ожидался 503, получено: {:.200}",
        r
    );
    assert!(r.contains("Retry-After:"), "{r}");
    assert!(
        body_of(&r).contains("queue.wait_timeout_sec"),
        "hint про ожидание: {}",
        body_of(&r)
    );

    // 5) Метрики отказов/ожиданий видны в снапшоте (для `hds check`).
    wait_for(
        "метрики отказов в снапшоте",
        Duration::from_secs(10),
        || snapshot(&dir).is_some_and(|v| v["rejected_full"] == 1 && v["timeouts"] == 1),
    );
    let snap = snapshot(&dir).unwrap();
    assert!(snap["wait_p50_ms"].as_u64().unwrap_or(0) >= 3000, "{snap}");
    assert_eq!(snap["granted"], 1, "{snap}");
    assert!(
        snap["total_wait_ms"].as_u64().unwrap_or(0) >= 3000,
        "ожидание учтено: {snap}"
    );

    drop(lock);
    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}

/// Очередь выключена (десктоп-дефолт) — прежнее поведение M3: мгновенный `429`
/// по `max_inflight`, ожидания нет.
#[test]
fn queue_disabled_keeps_m3_behaviour() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmp_dir("q2");
    // Тот же конфиг, но `queue.enabled: false` (как в desktop-профиле).
    let body = std::fs::read_to_string(write_config(&dir, 1, 4))
        .unwrap()
        .replace("queue:\n  enabled: true", "queue:\n  enabled: false");
    let cfg = dir.join("config.yaml");
    std::fs::write(&cfg, body).unwrap();
    let lock = lock_db(&dir);
    let addr = spawn_server(&cfg);

    let _held = hold_slot(addr);
    // Слот занят (первый запрос висит на блокировке БД); очередь выключена →
    // следующий тяжёлый запрос сразу получает 429 без ожидания.
    let mut r = String::new();
    for _ in 0..40 {
        r = send_heavy(addr);
        if r.starts_with("HTTP/1.1 429") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        r.starts_with("HTTP/1.1 429"),
        "ожидался 429 без ожидания, получено: {:.200}",
        r
    );
    // M4 T4: без очереди лимит ключуется пользователем, и hint адресный —
    // про пер-пользовательский предел (был общий «too many concurrent …»).
    assert!(
        body_of(&r).contains("rate_limit.max_inflight_per_user"),
        "подсказка про пер-пользовательский предел: {}",
        body_of(&r)
    );

    drop(lock);
    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}

/// T0.9 (F9 из M4_CODE_REVIEW.md): обрыв клиента во время ожидания слота —
/// `QueueReject::Aborted`: место в очереди освобождается, счётчик `aborted`
/// растёт, ответ не отправляется (соединение уже закрыто клиентом).
#[test]
fn aborted_client_releases_queue_slot() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmp_dir("q3");
    let cfg = write_config(&dir, 1, 4);
    let lock = lock_db(&dir); // держатель слота (писатель БД)
    let addr = spawn_server(&cfg);

    // 1) Слот занят (запрос висит на блокировке БД).
    let _held = hold_slot(addr);
    wait_for(
        "executing=1 в снапшоте очереди",
        Duration::from_secs(30),
        || snapshot(&dir).is_some_and(|v| v["executing"] == 1),
    );

    // 2) Второй тяжёлый запрос встал в очередь.
    let waiter = hold_slot(addr);
    wait_for(
        "depth=1 в снапшоте (запрос ждёт)",
        Duration::from_secs(10),
        || snapshot(&dir).is_some_and(|v| v["depth"] == 1),
    );

    // 3) Клиент оборвал соединение, не читая ответ.
    let _ = waiter.shutdown(std::net::Shutdown::Both);

    // 4) Место освободилось, обрыв зафиксирован в метриках (`hds check`).
    wait_for(
        "aborted=1, depth=0 в снапшоте",
        Duration::from_secs(10),
        || {
            snapshot(&dir)
                .is_some_and(|v| v["aborted"].as_u64().unwrap_or(0) >= 1 && v["depth"] == 0)
        },
    );

    drop(lock);
    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}
