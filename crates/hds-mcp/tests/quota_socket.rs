//! Сокет-тесты квот и приоритетов (M4 T4, `PLAN_M4_CONCURRENCY_SLA.md`
//! §3.3/§4): ключ лимитов — **пользователь**, пер-пользовательский предел
//! одновременных RAG, регресс M3 F10.3 («два токена одного пользователя
//! удваивали квоту»). Проверка идёт через реальный сокет
//! (`run_http_listener` + `TcpStream`) — чистые функции этот слой не ловят.
//!
//! Детерминизм: единственный слот «держит» реальный `ask_my_files`, висящий
//! на блокировке БД (песочница в `journal_mode: delete`, тест держит
//! `BEGIN EXCLUSIVE`). Ни GPU, ни Python-воркера тесты не требуют.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Тесты трогают env (`HDS_CONFIG`/`HDS_ROOT`) — идут последовательно.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Тяжёлый вызов (JSON-RPC `tools/call` + `ask_my_files`) — попадает под очередь.
const HEAVY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ask_my_files","arguments":{"question":"кто ждёт слот","limit":1}}}"#;

/// Песочница (`HDS_ROOT`) с каталогом `data/`.
fn tmp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("hds-quota-{tag}-{}-{nanos}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data")).unwrap();
    std::env::set_var("HDS_ROOT", &dir);
    dir
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

/// Тяжёлый запрос (тело одно); `read` — читать ли ответ.
fn heavy_req() -> Vec<u8> {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{HEAVY}",
        HEAVY.len()
    )
    .into_bytes()
}

/// Тяжёлый запрос с чтением ответа.
fn send_heavy(addr: SocketAddr) -> String {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(&heavy_req()).expect("write");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp
}

/// Тяжёлый запрос БЕЗ чтения ответа (соединение держим — запрос занимает слот).
fn hold_slot(addr: SocketAddr) -> TcpStream {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.write_all(&heavy_req()).expect("write");
    s
}

/// Простой GET с (необязательным) токеном.
fn send_get(addr: SocketAddr, path: &str, token: Option<&str>) -> String {
    let auth = match token {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let req = format!("GET {path} HTTP/1.1\r\nHost: t\r\n{auth}Connection: close\r\n\r\n");
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(req.as_bytes()).expect("write");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp
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

/// Конфиг: очередь включена (глобально 3 слота), пер-пользовательский предел — 1.
fn queue_cfg_body() -> &'static str {
    "profile: server\n\
     access:\n  enabled: false\n\
     db:\n  journal_mode: delete\n  read_only_conns: false\n  busy_timeout_ms: 30000\n\
     queue:\n  enabled: true\n  max_depth: 5\n  wait_timeout_sec: 6\n  policy: rr\n\
     rate_limit:\n  enabled: true\n  requests_per_min: 600\n  burst: 60\n  login_per_min: 30\n  max_inflight: 3\n  max_inflight_per_user: 1\n\
     http:\n  max_body_kb: 512\n  max_conns: 0\n  read_timeout_sec: 30\n\
     db_path: index.db\n\
     embedding:\n  base_url: \"http://127.0.0.1:1/v1\"\n\
     chat:\n  base_url: \"http://127.0.0.1:1/v1\"\n"
}

/// M4 T4 §3.3: пер-пользовательский предел. Второй ОДНОВРЕМЕННЫЙ RAG того же
/// пользователя отвергается сразу (`429` + hint про `max_inflight_per_user`),
/// хотя глобально есть свободные слоты (3). Счётчик — в снапшоте.
#[test]
fn per_user_limit_rejects_immediately_reaches_client() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmp_dir("peruser");
    let cfg = dir.join("config.yaml");
    std::fs::write(&cfg, queue_cfg_body()).unwrap();
    let lock = lock_db(&dir); // держатель слота (writer БД)
    let addr = spawn_server(&cfg);

    // 1) Первый тяжёлый запрос занимает слот пользователя (висит на блокировке БД).
    let _held = hold_slot(addr);
    wait_for(
        "executing=1 в снапшоте очереди",
        Duration::from_secs(30),
        || snapshot(&dir).is_some_and(|v| v["executing"] == 1),
    );
    let snap = snapshot(&dir).expect("снапшот очереди");
    assert_eq!(snap["max_inflight"], 3, "{snap}");
    assert_eq!(snap["max_inflight_per_user"], 1, "{snap}");

    // 2) Второй одновременный запрос того же ключа — сразу 429 (не ждёт).
    let r = send_heavy(addr);
    assert!(
        r.starts_with("HTTP/1.1 429"),
        "ожидался 429, получено: {:.200}",
        r
    );
    assert!(r.contains("Retry-After:"), "{r}");
    assert!(
        body_of(&r).contains("max_inflight_per_user"),
        "hint про пер-пользовательский предел: {}",
        body_of(&r)
    );

    // 3) Отказ виден в снапшоте (диагностика `hds check`).
    wait_for(
        "rejected_user=1 в снапшоте",
        Duration::from_secs(10),
        || snapshot(&dir).is_some_and(|v| v["rejected_user"] == 1),
    );

    drop(lock);
    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}

/// Регресс M3 F10.3 (M4 T4 §3.3): ключ лимитов — пользователь, поэтому ДВА
/// токена одного пользователя делят одну квоту (второй — `429`), а другой
/// пользователь имеет свой bucket.
#[test]
fn two_tokens_of_same_user_share_quota() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmp_dir("tokshare");
    // burst 1 → ровно один запрос на bucket, второй — 429 (refill 1/сек).
    let cfg = dir.join("config.yaml");
    std::fs::write(
        &cfg,
        "profile: desktop\n\
         access:\n  enabled: true\n  token_ttl_days: 90\n\
         rate_limit:\n  enabled: true\n  requests_per_min: 60\n  burst: 1\n  login_per_min: 30\n  max_inflight: 4\n\
         http:\n  max_body_kb: 512\n  max_conns: 0\n  read_timeout_sec: 30\n",
    )
    .unwrap();

    // access.yaml: anna (два токена) + boris (один).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut acc = hds_core::access::Access::default();
    for login in ["anna", "boris"] {
        acc.users.push(hds_core::access::User {
            name: login.to_string(),
            login: login.to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$placeholder".to_string(),
            admin: false,
            projects: Vec::new(),
            disabled: false,
        });
    }
    let (t_anna1, tok1) = hds_core::access::new_token("anna", 90, now).unwrap();
    let (t_anna2, tok2) = hds_core::access::new_token("anna", 90, now).unwrap();
    let (t_boris, tok3) = hds_core::access::new_token("boris", 90, now).unwrap();
    acc.tokens.extend([tok1, tok2, tok3]);
    hds_core::access::save_at(&dir.join("data").join("access.yaml"), &acc).unwrap();

    let addr = spawn_server(&cfg);

    // Первый токен anna израсходовал bucket пользователя.
    let r1 = send_get(addr, "/x", Some(&t_anna1));
    assert!(r1.starts_with("HTTP/1.1 404"), "anna tok1: {:.120}", r1);
    // Второй токен ТОГО ЖЕ пользователя — тот же bucket → 429 (квота не удвоилась).
    let r2 = send_get(addr, "/x", Some(&t_anna2));
    assert!(
        r2.starts_with("HTTP/1.1 429"),
        "anna tok2 должен делить квоту: {:.160}",
        r2
    );
    assert!(r2.contains("Retry-After:"), "{r2}");
    // Другой пользователь — свой bucket.
    let r3 = send_get(addr, "/x", Some(&t_boris));
    assert!(r3.starts_with("HTTP/1.1 404"), "boris: {:.120}", r3);

    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}

/// Тяжёлый запрос с Bearer-токеном (тело то же).
fn heavy_req_token(token: &str) -> Vec<u8> {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: t\r\nContent-Type: application/json\r\n\
         Authorization: Bearer {token}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{HEAVY}",
        HEAVY.len()
    )
    .into_bytes()
}

/// Авторизованный тяжёлый запрос с чтением ответа.
fn send_heavy_token(addr: SocketAddr, token: &str) -> String {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(&heavy_req_token(token)).expect("write");
    let mut resp = String::new();
    let _ = s.read_to_string(&mut resp);
    resp
}

/// Авторизованный тяжёлый запрос БЕЗ чтения ответа (занимает слот/очередь).
fn hold_slot_token(addr: SocketAddr, token: &str) -> TcpStream {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.write_all(&heavy_req_token(token)).expect("write");
    s
}

/// F2 (M5 T0.2): при `access.enabled` неавторизованный ТЯЖЁЛЫЙ запрос
/// отвергается `401` ДО постановки в очередь — занятый слот не заставляет его
/// ждать `wait_timeout_sec`, место в `queue.max_depth` он не занимает
/// (регресс-защита от вектора DoS, появившегося в M4).
#[test]
fn unauthorized_heavy_rejected_before_queue() {
    let _env = ENV_LOCK.lock().unwrap();
    let dir = tmp_dir("authq");
    // Конфиг очереди (слот исполнения 1), access включён, глубина ожидания 1;
    // per-user предел ослабляем (2 > глобального 1), чтобы авторизованный
    // waiter действительно ЖДАЛ слот, а не получал немедленный `429` PerUser.
    let body = queue_cfg_body()
        .replace("access:\n  enabled: false", "access:\n  enabled: true")
        .replace("max_depth: 5", "max_depth: 1")
        .replace("max_inflight: 3", "max_inflight: 1")
        .replace("max_inflight_per_user: 1", "max_inflight_per_user: 2");
    let cfg = dir.join("config.yaml");
    std::fs::write(&cfg, body).unwrap();

    // access.yaml: пользователи anna и boris с валидными токенами (проекты
    // непустые — иначе search не трогает БД и «занятый слот» не детерминирован).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut acc = hds_core::access::Access::default();
    for (login, projects) in [
        ("anna", vec!["demo".to_string()]),
        ("boris", vec!["demo".to_string()]),
    ] {
        acc.users.push(hds_core::access::User {
            name: login.to_string(),
            login: login.to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$placeholder".to_string(),
            admin: false,
            projects,
            disabled: false,
        });
    }
    let (t_anna, tok_a) = hds_core::access::new_token("anna", 90, now).unwrap();
    let (t_boris, tok_b) = hds_core::access::new_token("boris", 90, now).unwrap();
    acc.tokens.extend([tok_a, tok_b]);
    hds_core::access::save_at(&dir.join("data").join("access.yaml"), &acc).unwrap();

    let lock = lock_db(&dir);
    let addr = spawn_server(&cfg);

    // 1) Занять единственный слот исполнения авторизованным запросом.
    let _held = hold_slot_token(addr, &t_anna);
    wait_for(
        "executing=1 в снапшоте очереди",
        Duration::from_secs(30),
        || snapshot(&dir).is_some_and(|v| v["executing"] == 1),
    );

    // 2) Неавторизованный тяжёлый запрос → мгновенный 401 (не ждёт слот).
    let started = Instant::now();
    let r = send_heavy(addr);
    assert!(
        r.starts_with("HTTP/1.1 401"),
        "ожидался 401, получено: {:.200}",
        r
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "401 пришёл после ожидания очереди — F2 не закрыт"
    );

    // 3) В очередь он не встал: depth остаётся 0 (окно наблюдения дольше
    //    среза снапшота 2 с).
    std::thread::sleep(Duration::from_secs(3));
    let snap = snapshot(&dir).expect("снапшот очереди");
    assert_eq!(
        snap["depth"], 0,
        "неавторизованный занял место в очереди: {snap}"
    );

    // 4) Валидный токен ДРУГОГО пользователя ведёт себя как раньше: встаёт
    //    в очередь и после освобождения слота получает ответ.
    let waiter = std::thread::spawn(move || send_heavy_token(addr, &t_boris));
    wait_for(
        "depth=1 (авторизованный ждёт слот)",
        Duration::from_secs(10),
        || snapshot(&dir).is_some_and(|v| v["depth"] == 1),
    );
    drop(lock); // слот освободится — держатель и waiter завершатся
    let r2 = waiter.join().unwrap();
    assert!(
        r2.starts_with("HTTP/1.1 200"),
        "авторизованный должен дождаться ответа: {:.200}",
        r2
    );

    std::env::remove_var("HDS_CONFIG");
    std::env::remove_var("HDS_ROOT");
}
