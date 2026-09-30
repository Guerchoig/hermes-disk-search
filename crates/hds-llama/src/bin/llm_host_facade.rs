//! A5: живой фасад `:8010–8012` — инстансы по конфигу + HTTP-фасад + диспетчер VRAM.
//!
//! Собирает то, что уже проверено по частям: `registry::plan` (A2) создаёт инстансы
//! чата/эмбеддингов/реранка, `dispatch` (A4 шаг 2) решает про вытеснение и `index.pause`
//! и применяет решения, `facade` (A5) отдаёт OpenAI-совместимые эндпоинты на портах
//! `llama-server`, `http` обслуживает соединения (свой мини-сервер: crates.io недоступен).
//!
//! Порты по умолчанию — как у `llama-server` (8010/8011/8012), поэтому клиенты
//! (UI, MCP, Hermes, внешние агенты) не меняются. Если порт занят (например, ещё
//! работает Python-версия) — фасад честно скажет об этом и не станет его отбирать.
//!
//! Запуск из корня репозитория:
//! ```powershell
//! # проверочный прогон на альтернативных портах и с чатом на CPU (не трогая VRAM)
//! cargo run -p hds-llama --release --bin llm_host_facade -- --port-base 8020 --ngl 0 --hold 120
//! # боевой режим: порты 8010–8012, устройство из конфига
//! cargo run -p hds-llama --release --bin llm_host_facade -- --hold 0
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hds_llama::budget::{estimate_need_mib, kv_cache_mib};
use hds_llama::cluster::InstanceSpec;
use hds_llama::config;
use hds_llama::dispatch::{self, instance_use, Demand};
use hds_llama::error::{EngineError, Result};
use hds_llama::facade::{self, Backend, ChatRequest, ServerConfig, Thinking, Usage};
use hds_llama::gguf::{read_meta, KvBits};
use hds_llama::pause::IndexPause;
use hds_llama::registry;
use hds_llama::runtime::RuntimePaths;
use hds_llama::vram::NvmlProbe;
use hds_llama::{Cluster, Engine, VramProbe};

struct Args {
    config: PathBuf,
    runtime: Option<PathBuf>,
    engine_dir: Option<PathBuf>,
    host: String,
    port_base: Option<u16>,
    ngl: Option<i32>,
    thinking: Option<Thinking>,
    dispatcher: bool,
    hold: u64,
    json: Option<PathBuf>,
    /// Каталог сигнальных файлов (`index.pause`) — корень проекта.
    pause_dir: PathBuf,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut args = Args {
        config: std::env::var("HDS_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root().join("config.yaml")),
        runtime: None,
        engine_dir: None,
        host: "127.0.0.1".to_string(),
        port_base: None,
        ngl: None,
        thinking: None,
        dispatcher: true,
        hold: 0,
        json: None,
        pause_dir: repo_root(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> std::result::Result<String, String> {
            it.next()
                .ok_or_else(|| format!("после {name} ожидалось значение"))
        };
        match a.as_str() {
            "--config" => args.config = PathBuf::from(take("--config")?),
            "--runtime" => args.runtime = Some(PathBuf::from(take("--runtime")?)),
            "--engine-dir" => args.engine_dir = Some(PathBuf::from(take("--engine-dir")?)),
            "--host" => args.host = take("--host")?,
            "--pause-dir" => args.pause_dir = PathBuf::from(take("--pause-dir")?),
            "--json" => args.json = Some(PathBuf::from(take("--json")?)),
            "--port-base" => {
                args.port_base = Some(
                    take("--port-base")?
                        .parse()
                        .map_err(|e| format!("--port-base: {e}"))?,
                )
            }
            "--ngl" => {
                args.ngl = Some(
                    take("--ngl")?
                        .parse()
                        .map_err(|e| format!("--ngl: {e}"))?,
                )
            }
            "--thinking" => args.thinking = Some(Thinking::parse(&take("--thinking")?)),
            "--dispatcher" => {
                args.dispatcher = !matches!(
                    take("--dispatcher")?.trim().to_lowercase().as_str(),
                    "off" | "0" | "false" | "no"
                )
            }
            "--hold" => {
                args.hold = take("--hold")?
                    .parse()
                    .map_err(|e| format!("--hold: {e}"))?
            }
            "--help" | "-h" => {
                return Err("использование: llm_host_facade [--config FILE] [--runtime DIR] \
                            [--engine-dir DIR] [--host HOST] [--port-base N] [--ngl N] \
                            [--thinking off|on|auto] [--dispatcher on|off] [--hold SEC] \
                            [--json FILE]"
                    .to_string())
            }
            other => return Err(format!("неизвестный аргумент: {other}")),
        }
    }
    Ok(args)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ошибка: {e}");
        std::process::exit(1);
    }
}

/// Общая обёртка над кластером: движок вызывается **из одного потока за раз**.
///
/// `Cluster` держит сырой указатель движка (FFI), поэтому сам по себе не `Send`;
/// фасад обслуживает запросы в потоках сокетов, значит доступ сериализуем мьютексом.
/// Так же поступает и `llm-host`: один владелец GPU, вызовы по очереди.
struct ClusterShared(Mutex<Cluster>);

// SAFETY: доступ к движку идёт только через `with()`, то есть под мьютексом —
// параллельных вызовов одного объекта кластера не бывает.
unsafe impl Send for ClusterShared {}
unsafe impl Sync for ClusterShared {}

impl ClusterShared {
    fn with<T>(&self, f: impl FnOnce(&Cluster) -> T) -> T {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }
}

/// Живой backend фасада: инстансы по конфигу + диспетчер VRAM (A4) + пауза индексации.
struct ClusterBackend {
    cluster: Arc<ClusterShared>,
    /// Инстансы по ролям (id из `create_instance`).
    ids: BTreeMap<String, i64>,
    /// Модель и контекст роли — для `/props`.
    model_path: BTreeMap<String, String>,
    n_ctx: BTreeMap<String, i32>,
    /// Оценка «модель + KV» по ролям (для решений диспетчера).
    needs: BTreeMap<String, u64>,
    gpu: config::GpuConfig,
    nvml: Option<NvmlProbe>,
    pause: Arc<IndexPause>,
    /// Диспетчер включён (флаг `--dispatcher on`).
    dispatch_enabled: bool,
    /// Параллелизм слотов (`llama_server.parallel`) — для `/props`.
    parallel: i32,
    /// Журнал решений диспетчера (для `--json` и логов).
    decisions: Mutex<Vec<String>>,
}

impl ClusterBackend {
    /// Свободная VRAM (NVML) — источник истины (R29).
    fn free_mib(&self) -> Option<u64> {
        self.nvml
            .as_ref()
            .and_then(|p| p.snapshot())
            .map(|v| v.free_mib)
    }

    fn id_of(&self, role: &str) -> Result<i64> {
        self.ids.get(role).copied().ok_or_else(|| {
            EngineError::Other(format!(
                "инстанс роли '{role}' не создан (нет модели или роли нет в конфиге)"
            ))
        })
    }

    /// Загрузить роль, если она выгружена, и дождаться готовности.
    ///
    /// Нужен не только для `LOAD_ON_DEMAND`: диспетчер может выгрузить чат, чтобы
    /// отдать память другой роли (§8.6.2), и следующий запрос обязан его вернуть.
    fn ensure_loaded(&self, role: &str) -> Result<i64> {
        let id = self.id_of(role)?;
        let loaded = self.cluster.with(|c| {
            c.instance_by_id(id)
                .ok()
                .flatten()
                .map(|i| i.is_loaded())
                .unwrap_or(false)
        });
        if !loaded {
            self.cluster.with(|c| c.load(id))?;
            self.cluster
                .with(|c| c.wait_loaded(id, Duration::from_secs(300), Duration::from_millis(500)))?;
        }
        Ok(id)
    }

    /// Перед запросом: решить по VRAM и применить решение (ARB-1/ARB-2).
    ///
    /// Возвращает `PauseLease`, который держится до конца запроса: на `Drop` пауза
    /// снимается (если ставили её мы) — индексные роли вернутся сами.
    fn prepare(&self, role: &str) -> Option<hds_llama::PauseLease> {
        if !self.dispatch_enabled {
            return None;
        }
        let need = self.needs.get(role).copied().unwrap_or(0);
        if need == 0 {
            return None;
        }
        let plan = self.cluster.with(|cluster| {
            // сколько освободит каждый инстанс — по ЕГО собственной роли, а не по
            // запрошенной: иначе арифметика вытеснения врёт (поймано в живом прогоне)
            let uses = cluster
                .instances()
                .map(|list| {
                    list.iter()
                        .map(|i| {
                            let own = self.needs.get(&i.name).copied().unwrap_or(0);
                            instance_use(i, &i.name.clone(), own.max(if i.name == role { need } else { 0 }), 0)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            dispatch::plan_query(&self.gpu, self.free_mib(), &Demand::new(role, need), &uses)
        });
        let log = self
            .cluster
            .with(|cluster| dispatch::apply(cluster, &self.pause, &plan));
        for line in plan.lines().iter().chain(log.iter()) {
            println!("[dispatcher] {line}");
        }
        if let Ok(mut d) = self.decisions.lock() {
            d.push(format!("{role}: {}", plan.verdict.as_str()));
            d.extend(plan.lines());
        }
        self.pause.lease(&format!("запрос роли {role}")).ok()
    }
}

impl Backend for ClusterBackend {
    fn chat(&self, req: &ChatRequest) -> Result<(String, Usage)> {
        let _lease = self.prepare("chat");
        let id = self.ensure_loaded("chat")?;
        let out = self.cluster.with(|c| {
            c.chat_complete(
                id,
                &req.prompt,
                req.n_predict,
                req.temperature,
                req.reasoning(),
            )
        })?;
        if !out.ok {
            return Err(EngineError::Other(format!(
                "чат-инстанс вернул ошибку: {}",
                if out.error.is_empty() {
                    "без текста ошибки"
                } else {
                    &out.error
                }
            )));
        }
        Ok((
            out.text,
            Usage {
                prompt_tokens: out.metrics.prompt_tokens,
                completion_tokens: out.metrics.decoded_tokens,
            },
        ))
    }

    fn embeddings(&self, body_json: &str) -> Result<String> {
        let _lease = self.prepare("embedding");
        let id = self.ensure_loaded("embedding")?;
        let out = self
            .cluster
            .with(|c| c.embeddings_json(id, body_json, true))?;
        out.ensure_ok()
            .map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(out.json)
    }

    fn rerank(&self, body_json: &str) -> Result<String> {
        let _lease = self.prepare("rerank");
        let id = self.ensure_loaded("rerank")?;
        let out = self.cluster.with(|c| c.rerank_json(id, body_json))?;
        out.ensure_ok()
            .map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(out.json)
    }

    fn props(&self, role: &str) -> Option<serde_json::Value> {
        let id = self.ids.get(role).copied()?;
        Some(serde_json::json!({
            "model_path": self.model_path.get(role).cloned().unwrap_or_default(),
            "n_ctx": self.n_ctx.get(role).copied().unwrap_or(0),
            "total_slots": self.parallel.max(1),
            "state": self
                .cluster
                .with(|c| c.instance_by_id(id).ok().flatten().map(|i| i.state_name.clone()))
                .unwrap_or_default(),
        }))
    }
}

/// Значение из YAML по пути `a.b.c` (для `chat.*`, которых нет в `LlmHostConfig`).
fn dig<'a>(root: &'a serde_yaml::Value, path: &str) -> Option<&'a serde_yaml::Value> {
    let mut cur = root;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn dig_str(root: &serde_yaml::Value, path: &str) -> Option<String> {
    dig(root, path).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn dig_i64(root: &serde_yaml::Value, path: &str) -> Option<i64> {
    dig(root, path).and_then(|v| v.as_i64())
}

fn dig_f64(root: &serde_yaml::Value, path: &str) -> Option<f64> {
    dig(root, path).and_then(|v| v.as_f64())
}

fn run() -> Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    let json_path = args.json.as_ref().map(|p| absolutize(p));
    let cfg = config::load(&args.config)?;
    // `chat.*` и порты ролей берём из YAML напрямую (в `LlmHostConfig` их пока нет)
    let yaml: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(&args.config).map_err(|e| {
            EngineError::Other(format!("конфиг {}: {e}", args.config.display()))
        })?,
    )
    .map_err(|e| EngineError::Other(format!("конфиг {}: {e}", args.config.display())))?;
    let paths = match &args.runtime {
        Some(r) => RuntimePaths::new(r.clone()),
        None => RuntimePaths::from_env()?,
    };
    let nvml = NvmlProbe::open(0).ok();
    if let Some(p) = &nvml {
        if let Some(v) = p.snapshot() {
            println!(
                "NVML {}: занято {} / {} МиБ, свободно {} МиБ",
                p.name(),
                v.used_mib,
                v.total_mib,
                v.free_mib
            );
        }
    } else {
        println!("NVML недоступен: бюджет VRAM не проверяется (R29)");
    }

    let engine = Engine::open(args.engine_dir.as_deref())?;
    let _cwd = engine.activate()?; // cwd движка обязателен (грабли A1/A2)
    let cluster = engine.create_cluster()?;
    println!("движок: {}", engine.dir().display());

    // --- инстансы по конфигу (A2) ---
    let devices = cluster.devices()?;
    let planned = registry::plan(&cfg, &paths.root, &devices);
    let mut ids: BTreeMap<String, i64> = BTreeMap::new();
    let mut model_path: BTreeMap<String, String> = BTreeMap::new();
    let mut n_ctx_map: BTreeMap<String, i32> = BTreeMap::new();
    let mut needs: BTreeMap<String, u64> = BTreeMap::new();
    for p in &planned {
        let inst = match p {
            registry::RolePlan::Ready(i) => i,
            registry::RolePlan::Failed { role, error } => {
                println!("роль {role}: не поднята — {error}");
                continue;
            }
        };
        let mut spec: InstanceSpec = inst.spec.clone();
        if let Some(ngl) = args.ngl {
            spec.n_gpu_layers = Some(ngl);
            spec.allow_cpu = Some(true);
        }
        let file_mib = std::fs::metadata(&inst.model_path)
            .map(|m| m.len() >> 20)
            .unwrap_or(0);
        if let Ok(meta) = read_meta(&inst.model_path) {
            let n_ctx = spec.n_ctx.unwrap_or(0) as i64;
            let need = estimate_need_mib(
                &meta,
                file_mib,
                n_ctx,
                cfg.parallel.max(1) as i64,
                KvBits::F16,
            );
            needs.insert(inst.role.clone(), need);
            println!(
                "роль {}: модель {} МиБ, KV f16 {:.0} МиБ (KV-слоёв {} из {}), нужно {} МиБ",
                inst.role,
                file_mib,
                kv_cache_mib(&meta, n_ctx, cfg.parallel.max(1) as i64, KvBits::F16),
                meta.kv_layer_count(),
                meta.block_count,
                need
            );
            if let Some(free) = nvml.as_ref().and_then(|p| p.snapshot()).map(|v| v.free_mib) {
                if need + cfg.gpu.reserve_mb > free {
                    println!(
                        "  внимание: свободно {free} МиБ — не хватает; авто-деградации нет \
                         (вытеснение — диспетчер, отчёт — в 503)"
                    );
                }
            }
        }
        let id = cluster.create_instance(&spec)?;
        println!("  инстанс '{}' id={id} создан", inst.role);
        ids.insert(inst.role.clone(), id);
        model_path.insert(inst.role.clone(), inst.model_path.display().to_string());
        n_ctx_map.insert(inst.role.clone(), spec.n_ctx.unwrap_or(0));
    }
    if ids.is_empty() {
        return Err(EngineError::Other(
            "ни одной роли не удалось поднять — проверьте модели в общем рантайме \
             (`installers/ensure_llama_runtime.ps1`) и конфиг"
                .to_string(),
        ));
    }
    // чат держим резидентно (`KEEP_LOADED`), остальные загрузятся по требованию
    if let Some(chat) = ids.get("chat").copied() {
        println!("загружаю чат-инстанс (KEEP_LOADED)…");
        cluster.load(chat)?;
        let inst =
            cluster.wait_loaded(chat, Duration::from_secs(300), Duration::from_millis(500))?;
        println!("  чат: {}", inst.state_name);
    }
    // --- порты фасада: по конфигу или с базовым смещением (для проверок) ---
    let ports = ports_for(&ids, args.port_base)?;

    let backend = Arc::new(ClusterBackend {
        cluster: Arc::new(ClusterShared(Mutex::new(cluster))),
        ids: ids.clone(),
        model_path,
        n_ctx: n_ctx_map,
        needs: needs.clone(),
        gpu: cfg.gpu.clone(),
        nvml,
        pause: Arc::new(IndexPause::new(args.pause_dir.clone())),
        dispatch_enabled: args.dispatcher,
        parallel: cfg.parallel,
        decisions: Mutex::new(Vec::new()),
    });

    let server_cfg = ServerConfig {
        host: args.host.clone(),
        ports: ports.clone(),
        thinking: args
            .thinking
            .unwrap_or_else(|| Thinking::parse(&dig_str(&yaml, "chat.thinking").unwrap_or_else(|| "off".to_string()))),
        max_tokens: dig_i64(&yaml, "chat.max_tokens").unwrap_or(600) as i32,
        temperature: dig_f64(&yaml, "chat.temperature").unwrap_or(0.2) as f32,
        ..ServerConfig::default()
    };
    println!(
        "фасад: thinking по умолчанию {}, max_tokens {}, temperature {}",
        server_cfg.thinking.as_str(),
        server_cfg.max_tokens,
        server_cfg.temperature
    );
    let (stop, handles) = facade::serve(&server_cfg, Arc::clone(&backend) as Arc<dyn Backend>)?;

    // --- работаем: `--hold SEC` (0 — пока не остановят) ---
    println!(
        "готово: {} (Ctrl+C/остановка — уборка инстансов ниже)",
        ports
            .iter()
            .map(|(r, p)| format!("{r}:{p}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let started = Instant::now();
    while stop.load(std::sync::atomic::Ordering::Relaxed) == false {
        if args.hold > 0 && started.elapsed() >= Duration::from_secs(args.hold) {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }

    // --- журнал решений, уборка и отчёт ---
    let decisions = backend
        .decisions
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    println!("\nрешения диспетчера за прогон: {}", decisions.len() / 2);
    let mut cleanup = Vec::new();
    for (role, id) in &ids {
        let _ = backend.cluster.with(|c| c.unload(*id));
        let removed = backend.cluster.with(|c| c.remove_instance(*id));
        cleanup.push(format!("{role}: remove_instance -> {removed:?}"));
    }
    let pause_left = backend.pause.is_paused();
    println!(
        "инстансы сняты: {}; index.pause сейчас: {}",
        cleanup.len(),
        if pause_left { "стоит" } else { "нет" }
    );

    if let Some(json_path) = &json_path {
        let report = serde_json::json!({
            "config": cfg.path.display().to_string(),
            "engine_dir": engine.dir().display().to_string(),
            "host": server_cfg.host,
            "ports": ports.iter().map(|(r, p)| serde_json::json!({ "role": r, "port": p }))
                .collect::<Vec<_>>(),
            "thinking": server_cfg.thinking.as_str(),
            "max_tokens": server_cfg.max_tokens,
            "temperature": server_cfg.temperature,
            "dispatcher_enabled": args.dispatcher,
            "dispatcher_lines": decisions,
            "needs_mib": needs,
            "instances": ids.iter().map(|(r, id)| serde_json::json!({ "role": r, "id": id }))
                .collect::<Vec<_>>(),
            "cleanup": cleanup,
            "pause_present_after": pause_left,
            "hold_sec": args.hold,
        });
        if let Some(dir) = json_path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| EngineError::Other(format!("json: {e}")))?;
        std::fs::write(json_path, text)
            .map_err(|e| EngineError::Other(format!("{}: {e}", json_path.display())))?;
        println!("отчёт: {}", json_path.display());
    }
    Ok(())
}

/// Порты фасада по ролям: из конфига-умолчаний или с базовым смещением (`--port-base`).
fn ports_for(ids: &BTreeMap<String, i64>, base: Option<u16>) -> Result<Vec<(String, u16)>> {
    ids.keys()
        .map(|role| {
            let port = match base {
                Some(b) => match role.as_str() {
                    "chat" => b,
                    "embedding" => b + 1,
                    "rerank" => b + 2,
                    _ => 0,
                },
                None => facade::default_port(role),
            };
            if port == 0 {
                return Err(EngineError::Other(format!(
                    "для роли '{role}' не нашлось порта (фасад знает chat/embedding/rerank)"
                )));
            }
            Ok((role.clone(), port))
        })
        .collect()
}
