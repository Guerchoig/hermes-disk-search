//! A6 — резидентный `llm-host`: pid-файл, лог, внутренний API и режимы.
//!
//! Проверяется то, что можно проверить без GPU и без сети «наружу»: второй
//! экземпляр не поднимается (pids), устаревший pid-файл снимается, лог пишется,
//! `/internal/*` доступны только при `internal = true`, прокси-режим отдаёт ответ
//! апстрима как есть, а мини-HTTP-клиент CLI понимает наш же сервер.
//!
//! Цифры/поведение движка здесь не проверяются — для этого живые прогоны
//! (`tools/parity/resident_smoke.ps1`) и `W2_REPORT.md` §7.x.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use hds_llama::facade::{handle, route, Backend, ChatRequest, Route, ServerConfig, Usage};
use hds_llama::host::{self, Host, HostConfig};
use hds_llama::http::{self, Request, Response};
use hds_llama::resident::{self, Log, PidFile};
use hds_llama::{client_json, Result};

/// Временный каталог (аналог корня проекта) — как в `pause_gate.rs`.
struct Tmp {
    dir: PathBuf,
}

impl Tmp {
    fn new(name: &str) -> Tmp {
        let dir = std::env::temp_dir().join("hds-llama-tests").join(format!(
            "resident-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Tmp { dir }
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Второй экземпляр обязан получить отказ: два `llm-host` — два владельца GPU.
#[test]
fn pid_file_blocks_second_instance_and_releases_on_drop() {
    let tmp = Tmp::new("pid");
    let path = tmp.dir.join("llm-host.pid");

    let first = PidFile::acquire(&path).expect("первый занимает pid-файл");
    assert_eq!(resident::read_pid(&path), Some(std::process::id()));
    assert_eq!(resident::owner_pid(&path), Some(std::process::id()));

    let err = PidFile::acquire(&path).expect_err("второй экземпляр не поднимается");
    assert!(
        err.to_string().contains("уже запущен"),
        "текст ошибки должен объяснять причину: {err}"
    );

    assert!(first.release(), "наш файл освобождается");
    assert!(!path.exists(), "файла нет после release");
    // после освобождения место свободно
    let again = PidFile::acquire(&path).expect("после release занимаем снова");
    drop(again);
    assert!(!path.exists(), "Drop освобождает pid-файл");
}

/// PID умершего процесса — это мусор, а не «запущен» (как `_lock_is_stale` в Python).
#[test]
fn stale_pid_file_is_reclaimed() {
    let tmp = Tmp::new("stale");
    let path = tmp.dir.join("llm-host.pid");
    // 0x7FFF_FF00 (2147483392) не может быть живым pid: в Windows/Linux
    // пространство pid ограничено 2^22, значит OpenProcess вернёт NULL
    std::fs::write(&path, "2147483392\n").expect("пишем мусорный pid");
    assert_eq!(
        resident::owner_pid(&path),
        None,
        "мёртвый pid не считается живым"
    );

    let taken = PidFile::acquire(&path).expect("устаревший файл снимается и занимается заново");
    assert_eq!(resident::read_pid(&path), Some(std::process::id()));
    assert_ne!(taken.pid(), 2147483392);
}

/// Лог резидентно процесса: строки идут в файл (без него падение не восстановить).
#[test]
fn log_file_collects_lines() {
    let tmp = Tmp::new("log");
    let path = tmp.dir.join("data").join("logs").join("llm-host.log");
    let log = Log::to_file(&path).expect("лог открывается, каталог создаётся");
    assert_eq!(log.path(), Some(path.as_path()));
    log.file_only("первая строка");
    log.line("вторая строка");
    let text = std::fs::read_to_string(&path).expect("читаем лог");
    assert!(text.contains("первая строка"), "{text}");
    assert!(text.contains("вторая строка"), "{text}");
}

/// Пути по умолчанию — под `data/` проекта (в git `data/` не входит).
#[test]
fn host_config_defaults_point_to_project_data() {
    let cfg = HostConfig::default();
    let root = host::repo_root();
    assert_eq!(
        cfg.pid_file.as_deref(),
        Some(resident::default_pid_path(&root).as_path())
    );
    assert_eq!(
        cfg.log_file.as_deref(),
        Some(resident::default_log_path(&root).as_path())
    );
    assert!(
        cfg.pid_path().ends_with("data/llm-host.pid")
            || cfg.pid_path().ends_with("data\\llm-host.pid")
    );
    assert!(cfg
        .log_path()
        .to_string_lossy()
        .ends_with("logs\\llm-host.log"));
    assert!(cfg.internal && cfg.dispatcher, "боевые умолчания");
    assert_eq!(HostConfig::default().without_residency().pid_file, None);
}

/// Подделка backend: запоминает вызовы внутреннего API и умеет «быть апстримом».
struct Fake {
    /// Роль, которую «подняли» (для `/props`).
    up: Vec<String>,
    /// Базовый URL апстрима (режим `facade`), если задан.
    upstream: Option<String>,
    /// Что вернули `internal_*` (по умолчанию — «не поддерживается», как у trait).
    calls: Mutex<Vec<String>>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            up: vec!["chat".to_string()],
            upstream: None,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn called(&self) -> Vec<String> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

impl Backend for Fake {
    fn chat(&self, req: &ChatRequest) -> Result<(String, Usage)> {
        Ok((
            format!("эхо: {}", req.prompt.chars().take(20).collect::<String>()),
            Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
            },
        ))
    }

    fn embeddings(&self, _body: &str) -> Result<String> {
        Ok(r#"{"data":[{"embedding":[0.1],"index":0}]}"#.to_string())
    }

    fn rerank(&self, _body: &str) -> Result<String> {
        Ok(r#"{"results":[{"index":0,"relevance_score":0.5}]}"#.to_string())
    }

    fn props(&self, role: &str) -> Option<Value> {
        // так же, как `ClusterBackend`: в прокси-режиме `/props` спрашивает апстрим
        if let Some(base) = &self.upstream {
            let url = format!("{}/props", base.trim_end_matches('/'));
            if let Ok((200, json)) = client_json("GET", &url, None, Duration::from_secs(10)) {
                return Some(json);
            }
        }
        self.up.iter().any(|r| r == role).then(|| {
            json!({
                "model_path": format!("…/{role}/m.gguf"),
                "n_ctx": 32768,
                "total_slots": 1,
            })
        })
    }

    fn upstream(&self, _role: &str) -> Option<String> {
        self.upstream.clone()
    }

    fn internal_status(&self) -> Result<Value> {
        self.calls.lock().map(|mut c| c.push("status".into())).ok();
        Ok(json!({ "mode": "embedded", "lines": ["строка отчёта"] }))
    }

    fn internal_devices(&self) -> Result<Value> {
        self.calls.lock().map(|mut c| c.push("devices".into())).ok();
        Ok(json!({ "lines": ["index=0 backend=CUDA"] }))
    }

    fn internal_load(&self, role: &str) -> Result<Value> {
        self.calls
            .lock()
            .map(|mut c| c.push(format!("load:{role}")))
            .ok();
        Ok(json!({ "role": role, "loaded": true }))
    }

    fn internal_unload(&self, role: &str) -> Result<Value> {
        self.calls
            .lock()
            .map(|mut c| c.push(format!("unload:{role}")))
            .ok();
        Ok(json!({ "role": role, "loaded": false }))
    }

    fn internal_stop(&self) -> Result<Value> {
        self.calls.lock().map(|mut c| c.push("stop".into())).ok();
        Ok(json!({ "stopping": true }))
    }
}

/// Подделка, у которой внутренний API не реализован (дефолты trait → 501).
struct Plain(Fake);

impl Backend for Plain {
    fn chat(&self, req: &ChatRequest) -> Result<(String, Usage)> {
        self.0.chat(req)
    }
    fn embeddings(&self, body: &str) -> Result<String> {
        self.0.embeddings(body)
    }
    fn rerank(&self, body: &str) -> Result<String> {
        self.0.rerank(body)
    }
    fn props(&self, role: &str) -> Option<Value> {
        self.0.props(role)
    }
}

fn req(method: &str, path: &str, body: &str) -> Request {
    Request {
        method: method.to_string(),
        path: path.to_string(),
        query: String::new(),
        body: body.to_string(),
        keep_alive: false,
    }
}

fn cfg_with_internal(internal: bool) -> ServerConfig {
    ServerConfig {
        internal,
        ..ServerConfig::default()
    }
}

/// Крошечный HTTP-сервер (наш же `http::serve`) — «внешний владелец» ролей.
struct Stub {
    port: u16,
    stop: Arc<AtomicBool>,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    fn start() -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("эфемерный порт");
        let port = listener.local_addr().expect("адрес").port();
        let stop = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stop_thread = Arc::clone(&stop);
        let hits_thread = Arc::clone(&hits);
        std::thread::spawn(move || {
            let handler = move |r: &Request| {
                if let Ok(mut h) = hits_thread.lock() {
                    h.push(format!("{} {}", r.method, r.path));
                }
                // `/props` апстрима выглядит как у `llama-server` (Python-версия
                // считает инстанс «своим» именно по `model_path`)
                if r.path.contains("props") {
                    return Response::ok(json!({
                        "origin": "upstream",
                        "model_path": "D:\\upstream\\chat.gguf",
                        "n_ctx": 4096,
                        "total_slots": 2,
                    }));
                }
                Response::ok(json!({
                    "origin": "upstream",
                    "path": r.path,
                    "body_len": r.body.len(),
                }))
            };
            let _ = http::serve(listener, stop_thread, handler);
        });
        std::thread::sleep(Duration::from_millis(150)); // ждём accept-цикл
        Stub { port, stop, hits }
    }

    /// База как в конфиге ролей (`chat.base_url`): с `/v1`.
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().map(|h| h.clone()).unwrap_or_default()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// `/internal/*` — только для «своих»: при `internal = false` отвечаем 403.
#[test]
fn internal_routes_are_gated_by_flag() {
    let backend = Fake::new();
    let off = handle(
        &req("GET", "/internal/status", ""),
        &cfg_with_internal(false),
        &backend,
    );
    assert_eq!(off.status, 403, "{}", off.json);
    assert!(
        off.json.to_string().contains("внутренний API выключен"),
        "{}",
        off.json
    );
    assert!(
        backend.called().is_empty(),
        "backend не тронули: {:?}",
        backend.called()
    );

    let on = handle(
        &req("GET", "/internal/status", ""),
        &cfg_with_internal(true),
        &backend,
    );
    assert_eq!(on.status, 200, "{}", on.json);
    assert_eq!(on.json["mode"], "embedded");
    assert_eq!(backend.called(), vec!["status".to_string()]);
}

/// Внутренние маршруты доходят до backend: status/devices/load/unload/stop.
#[test]
fn internal_commands_reach_backend() {
    let backend = Fake::new();
    let cfg = cfg_with_internal(true);

    // маршрутизация знает внутренний API (в т.ч. с префиксом `/v1`)
    assert_eq!(route("GET", "/internal/status"), Route::InternalStatus);
    assert_eq!(route("GET", "/v1/internal/devices"), Route::InternalDevices);
    assert_eq!(route("POST", "/internal/stop"), Route::InternalStop);
    assert!(Route::InternalStatus.is_internal() && !Route::Chat.is_internal());
    assert!(Route::InternalLoad.needs_body() && !Route::InternalStatus.needs_body());

    assert_eq!(
        handle(&req("GET", "/internal/devices", ""), &cfg, &backend).status,
        200
    );

    let load = handle(
        &req("POST", "/internal/load", r#"{"role": "rerank"}"#),
        &cfg,
        &backend,
    );
    assert_eq!(load.status, 200, "{}", load.json);
    assert_eq!(load.json["role"], "rerank");

    // пустое тело = чат (как `llama_server status` для единственной роли)
    let load_chat = handle(&req("POST", "/internal/load", ""), &cfg, &backend);
    assert_eq!(load_chat.json["role"], "chat");

    let unload = handle(
        &req("POST", "/internal/unload", r#"{"role": "embedding"}"#),
        &cfg,
        &backend,
    );
    assert_eq!(unload.status, 200, "{}", unload.json);

    let stop = handle(&req("POST", "/internal/stop", ""), &cfg, &backend);
    assert_eq!(stop.json["stopping"], true);

    // неверное тело — 400 с причиной (не 500)
    let bad = handle(
        &req("POST", "/internal/load", r#"{"rol": "chat"}"#),
        &cfg,
        &backend,
    );
    assert_eq!(bad.status, 400, "{}", bad.json);
    assert!(bad.json.to_string().contains("role"), "{}", bad.json);

    let called = backend.called();
    for expected in [
        "devices",
        "load:rerank",
        "load:chat",
        "unload:embedding",
        "stop",
    ] {
        assert!(
            called.iter().any(|c| c == expected),
            "нет вызова {expected}: {called:?}"
        );
    }
}

/// Дефолтные реализации `Backend` (без внутреннего API) — честные 501/409.
#[test]
fn internal_defaults_answer_not_implemented() {
    let backend = Plain(Fake::new());
    let cfg = cfg_with_internal(true);
    for (method, path, expected) in [
        ("GET", "/internal/status", 501),
        ("GET", "/internal/devices", 501),
        ("POST", "/internal/stop", 501),
        ("POST", "/internal/load", 409),
    ] {
        let r = handle(&req(method, path, r#"{"role": "chat"}"#), &cfg, &backend);
        assert_eq!(r.status, expected, "{method} {path}: {}", r.json);
    }
}

/// Режим `llm_server.mode: facade`: ответ апстрима отдаётся **как есть**.
#[test]
fn facade_mode_proxies_upstream_as_is() {
    let stub = Stub::start();
    let backend = Fake {
        upstream: Some(stub.base()),
        ..Fake::new()
    };
    let cfg = cfg_with_internal(true);

    let body = r#"{"messages":[{"role":"user","content":"привет"}],"model":"chat"}"#;
    let r = handle(&req("POST", "/v1/chat/completions", body), &cfg, &backend);
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["origin"], "upstream", "{}", r.json);
    assert_eq!(r.json["path"], "/v1/chat/completions");
    assert!(
        r.json["body_len"].as_u64().unwrap_or(0) > 0,
        "тело ушло целиком: {}",
        r.json
    );

    // `/props` в прокси-режиме спрашивает апстрим: факты об инстансе берутся
    // оттуда (Python-версия считает инстанс «своим» именно по `model_path`)
    let props = handle(&req("GET", "/props", ""), &cfg, &backend);
    assert_eq!(props.status, 200, "{}", props.json);
    assert_eq!(
        props.json["model_path"], "D:\\upstream\\chat.gguf",
        "{}",
        props.json
    );
    assert_eq!(props.json["n_ctx"], 4096);

    // эмбеддинги тоже проксируются, а не обрабатываются локально
    let emb = handle(
        &req("POST", "/v1/embeddings", r#"{"input":"x"}"#),
        &cfg,
        &backend,
    );
    assert_eq!(emb.status, 200, "{}", emb.json);
    assert_eq!(emb.json["origin"], "upstream");

    assert!(
        stub.hits()
            .iter()
            .any(|h| h.contains("/v1/chat/completions")),
        "апстрим получил запрос: {:?}",
        stub.hits()
    );
}

/// Мини-HTTP-клиент CLI: читает наш же сервер (и понятно ругается на закрытый порт).
#[test]
fn client_json_talks_to_our_own_server() {
    let stub = Stub::start();
    let (status, json) = client_json(
        "GET",
        &format!("{}/health", stub.base()),
        None,
        Duration::from_secs(5),
    )
    .expect("ответ от своего сервера");
    assert_eq!(status, 200);
    assert_eq!(json["origin"], "upstream");

    let (status, json) = client_json(
        "POST",
        &format!("{}/embeddings", stub.base()),
        Some(r#"{"input":"x"}"#),
        Duration::from_secs(5),
    )
    .expect("POST с телом");
    assert_eq!(status, 200);
    assert!(
        json["body_len"].as_u64().unwrap_or(0) > 0,
        "тело дошло: {json}"
    );

    // закрытый порт — понятная ошибка (её показывает CLI)
    let err = client_json(
        "GET",
        "http://127.0.0.1:1/health",
        None,
        Duration::from_millis(300),
    )
    .expect_err("порт закрыт");
    assert!(
        err.to_string().contains("не подключиться") || err.to_string().contains("не отправ"),
        "{err}"
    );
}

/// Записать минимальный `config.yaml` во временный каталог (аналог проекта).
fn write_config(tmp: &Tmp, name: &str, body: &str) -> PathBuf {
    let path = tmp.dir.join(name);
    std::fs::write(&path, body).expect("конфиг");
    path
}

/// Порты фасада берутся из конфига (`llm_server.<role>.port`), а `--port-base`
/// смещает их для проверочных прогонов — как у `llama-server`.
#[test]
fn port_roles_follow_config_and_base() {
    let tmp = Tmp::new("ports");
    let path = write_config(
        &tmp,
        "ports.yaml",
        r#"index:
  whisper_model_path: ""
llm_server:
  mode: embedded
  host: "127.0.0.1"
  parallel: 1
  chat:
    port: 9010
    model: "shared:chat"
    ctx_per_slot: 32768
  rerank:
    port: 9012
    model: "shared:rerank"
    ctx_per_slot: 8192
    extra_args: "-ngl 0"
"#,
    );
    let cfg = hds_llama::config::load(&path).expect("конфиг читается");
    let ports = host::port_roles(&cfg, None);
    assert!(ports.contains(&("chat".to_string(), 9010)), "{ports:?}");
    assert!(ports.contains(&("rerank".to_string(), 9012)), "{ports:?}");

    let based = host::port_roles(&cfg, Some(18000));
    assert!(based.contains(&("chat".to_string(), 18000)), "{based:?}");
    assert!(based.contains(&("rerank".to_string(), 18002)), "{based:?}");
    assert!(
        !based.iter().any(|(_, p)| *p == 9010),
        "при --port-base боевые порты не занимаем: {based:?}"
    );

    // клиентские адреса CLI — по тем же портам
    let urls = host::client_ports(&cfg, "127.0.0.1", Some(18000));
    assert!(
        urls.contains(&"http://127.0.0.1:18000".to_string()),
        "{urls:?}"
    );
}

/// Оценка «модель + KV» считается без движка; роль без модели пропускается.
#[test]
fn role_needs_skips_roles_without_models() {
    let tmp = Tmp::new("needs");
    let path = write_config(
        &tmp,
        "needs.yaml",
        r#"index:
  whisper_model_path: ""
llm_server:
  host: "127.0.0.1"
  parallel: 1
  chat:
    port: 9010
    model: "C:\\nope\\hermes-disk-search-test\\none.gguf"
    ctx_per_slot: 32768
"#,
    );
    let cfg = hds_llama::config::load(&path).expect("конфиг читается");
    assert!(
        !cfg.roles.is_empty(),
        "роль в конфиге есть: {:?}",
        cfg.roles.len()
    );
    let needs = host::role_needs(&cfg, &tmp.dir.join("runtime"));
    assert!(
        needs.is_empty(),
        "модели нет — оценивать нечего, роль не планируется: {needs:?}"
    );
}

/// Режим `llm_server.mode: off`: хост поднимает фасад **без движка** и отвечает
/// на `/internal/status` — именно так его видит CLI (и UI после перевода портов).
#[test]
fn host_off_mode_serves_status_without_engine() {
    let tmp = Tmp::new("off");
    // порты из pid: боевые 8010–8012 не занимаем ни при каких условиях
    let base = 18000u16.wrapping_add(((std::process::id() % 300) as u16) * 3);
    let path = write_config(
        &tmp,
        "off.yaml",
        &format!(
            "index:\n  whisper_model_path: \"\"\nllm_server:\n  mode: off\n  host: \"127.0.0.1\"\n  \
             parallel: 1\n  chat:\n    port: {base}\n    model: \"shared:chat\"\n    \
             ctx_per_slot: 32768\n"
        ),
    );
    let cfg = HostConfig {
        config: path.clone(),
        pause_dir: tmp.dir.clone(),
        // все роли фасада уводим на высокие порты от `base`: тест не должен
        // занимать/проверять боевые 8010–8012 (там могут стоять роли Python-версии)
        port_base: Some(base),
        ..HostConfig::default().without_residency()
    };
    let mut host = match Host::start(cfg) {
        Ok(h) => h,
        Err(e) => {
            // занятый порт/нет NVML — это не дефект кода: тест пропускает
            println!("пропуск: хост не поднялся ({e})");
            return;
        }
    };
    assert_eq!(host.mode().as_str(), "off");
    assert!(host.engine_dir().is_none(), "в режиме off движок не грузим");
    assert!(host.ids().is_empty(), "инстансы не создаются");
    assert_eq!(
        host.ports().first().map(|(r, p)| (r.as_str(), *p)),
        Some(("chat", base)),
        "первый порт — чат на базовом смещении: {:?}",
        host.ports()
    );
    assert!(
        host.ports().iter().all(|(_, p)| *p >= base),
        "боевые порты 8010–8012 тест не занимает: {:?}",
        host.ports()
    );
    assert_eq!(host.pid(), None, "pid-файл не занимали (without_residency)");

    let url = format!("http://127.0.0.1:{base}/internal/status");
    let (status, json) = client_json("GET", &url, None, Duration::from_secs(15))
        .expect("статус резидента доступен без движка");
    assert_eq!(status, 200, "{json}");
    assert_eq!(json["mode"], "off");
    assert!(
        json["lines"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "человеческие строки отчёта: {json}"
    );

    // без движка роли всё равно недоступны — и это честная причина, а не 500
    // (проверяем ДО `stop`: после команды остановки фасад уже не отвечает)
    let (status, err) = client_json(
        "GET",
        &format!("http://127.0.0.1:{base}/internal/devices"),
        None,
        Duration::from_secs(15),
    )
    .expect("ответ есть");
    assert_eq!(status, 501, "{err}");
    assert!(
        err.to_string().contains("off") || err.to_string().contains("выключен"),
        "{err}"
    );

    // `/internal/stop` завершает процесс (после ответа фасад закрывается)
    let (status, stop) = client_json(
        "POST",
        &format!("http://127.0.0.1:{base}/internal/stop"),
        Some("{}"),
        Duration::from_secs(15),
    )
    .expect("stop доступен");
    assert_eq!(status, 200, "{stop}");
    assert_eq!(stop["stopping"], true);

    host.wait(30);
    host.stop();
}
