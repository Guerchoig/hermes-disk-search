//! M6-T0 спайк «CUDA + Vulkan в одном `llm-host`» (`PLAN_M6_GPU_MULTIVENDOR.md` §4 T0).
//!
//! В одном процессе движка поднимаются ДВА инстанса на разных вендорах:
//! * chat-роль (модель чата) на CUDA-устройстве (первый `backend=CUDA`);
//! * embedding-роль (bge-m3) на Vulkan-устройстве (первый `backend=Vulkan`).
//!
//! Для каждой пары раундов измеряется: время загрузки каждой роли, латентности
//! chat/embeddings вперемешку, время выгрузки, VRAM NVIDIA (NVML) по стадиям и
//! `memory_free` движка по устройствам (AMD UMA виден только движком/пробой DXGI).
//! Критерий T0: обе роли реально грузятся на своих картах, инференс interleaved
//! работает, unload/load повторяется без зависаний и ошибок.
//!
//! Запуск (из корня репозитория):
//! `cargo run -p hds-llama --release --bin a7_multiinstance_probe -- [--engine-dir DIR]
//!  [--model-chat GGUF] [--model-embed GGUF] [--n-ctx-chat N] [--n-ctx-embed N]
//!  [--rounds N] [--n-predict N] [--json FILE]`
//!
//! Отчёт — `tools/parity/out/m6_spike_cudavulkan.json`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use hds_llama::cluster::{Cluster, Device, InstanceSpec, Metrics};
use hds_llama::error::{EngineError, Result};
use hds_llama::ffi::{model_kind, retention};
use hds_llama::vram::{NvmlProbe, VramProbe, VramSnapshot};
use hds_llama::Engine;

struct Args {
    engine_dir: Option<PathBuf>,
    model_chat: PathBuf,
    model_embed: PathBuf,
    n_ctx_chat: i32,
    n_ctx_embed: i32,
    rounds: usize,
    n_predict: i32,
    json: PathBuf,
    nvml_index: u32,
    /// Не поднимать chat-роль (ревизия T0: проверяем роль на Vulkan без CUDA).
    no_chat: bool,
    /// Не поднимать embed-роль (только chat на CUDA).
    no_embed: bool,
}

fn usage() -> String {
    "использование: a7_multiinstance_probe [--engine-dir DIR] [--model-chat GGUF] \
     [--model-embed GGUF] [--n-ctx-chat N] [--n-ctx-embed N] [--rounds N] \
     [--n-predict N] [--json FILE] [--nvml-index N] [--no-chat] [--no-embed]"
        .to_string()
}

fn default_chat_model() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(
        base.data_local_dir()
            .join("llama-runtime")
            .join("models")
            .join("chat")
            .join("Qwen3.5-9B-Q6_K.gguf"),
    )
}

fn default_embed_model() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(
        base.data_local_dir()
            .join("llama-runtime")
            .join("models")
            .join("embedding")
            .join("bge-m3-Q8_0.gguf"),
    )
}

fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut args = Args {
        engine_dir: None,
        model_chat: default_chat_model().unwrap_or_else(|| PathBuf::from("chat.gguf")),
        model_embed: default_embed_model().unwrap_or_else(|| PathBuf::from("bge-m3-Q8_0.gguf")),
        n_ctx_chat: 8192,
        n_ctx_embed: 4096,
        rounds: 2,
        n_predict: 32,
        json: PathBuf::from("tools/parity/out/m6_spike_cudavulkan.json"),
        nvml_index: 0,
        no_chat: false,
        no_embed: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> std::result::Result<String, String> {
            it.next()
                .ok_or_else(|| format!("после {name} ожидалось значение"))
        };
        match a.as_str() {
            "--engine-dir" => args.engine_dir = Some(PathBuf::from(take("--engine-dir")?)),
            "--model-chat" => args.model_chat = PathBuf::from(take("--model-chat")?),
            "--model-embed" => args.model_embed = PathBuf::from(take("--model-embed")?),
            "--n-ctx-chat" => {
                args.n_ctx_chat = take("--n-ctx-chat")?
                    .parse()
                    .map_err(|e| format!("--n-ctx-chat: {e}"))?
            }
            "--n-ctx-embed" => {
                args.n_ctx_embed = take("--n-ctx-embed")?
                    .parse()
                    .map_err(|e| format!("--n-ctx-embed: {e}"))?
            }
            "--rounds" => {
                args.rounds = take("--rounds")?
                    .parse()
                    .map_err(|e| format!("--rounds: {e}"))?
            }
            "--n-predict" => {
                args.n_predict = take("--n-predict")?
                    .parse()
                    .map_err(|e| format!("--n-predict: {e}"))?
            }
            "--json" => args.json = PathBuf::from(take("--json")?),
            "--nvml-index" => {
                args.nvml_index = take("--nvml-index")?
                    .parse()
                    .map_err(|e| format!("--nvml-index: {e}"))?
            }
            "--no-chat" => args.no_chat = true,
            "--no-embed" => args.no_embed = true,
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("неизвестный аргумент: {other}\n{}", usage())),
        }
    }
    Ok(args)
}
/// Снимок памяти устройства движком (для AMD UMA — единственный источник чисел).
#[derive(Debug, Clone, Serialize)]
struct EngineMem {
    name: String,
    backend: String,
    bridge_device_index: i32,
    free_mib: f64,
    total_mib: f64,
}

fn engine_mem(cluster: &Cluster) -> Result<Vec<EngineMem>> {
    Ok(cluster
        .devices()?
        .into_iter()
        .map(|d| EngineMem {
            name: d.name.clone(),
            backend: d.backend.clone(),
            bridge_device_index: d.bridge_device_index,
            free_mib: d.memory_free_mib(),
            total_mib: d.memory_total as f64 / (1024.0 * 1024.0),
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
struct InferStats {
    ok: bool,
    error: String,
    request_ms: f64,
    total_tokens_per_second: f64,
    decoded_tokens: i32,
    request_total_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
struct RoundStats {
    round: usize,
    chat_load_ms: u128,
    embed_load_ms: u128,
    chat_unload_ms: u128,
    embed_unload_ms: u128,
    chat1: InferStats,
    embed1: InferStats,
    chat2: InferStats,
    embed2: InferStats,
    engine_mem_after_load: Vec<EngineMem>,
    engine_mem_after_unload: Vec<EngineMem>,
    nvml_after_load: Option<VramSnapshot>,
    nvml_after_unload: Option<VramSnapshot>,
}

#[derive(Serialize)]
struct Report {
    generated: String,
    host: String,
    engine_dir: String,
    engine_lib: String,
    model_chat: String,
    model_embed: String,
    n_ctx_chat: i32,
    n_ctx_embed: i32,
    devices: Vec<Device>,
    cuda_csv: i32,
    vulkan_csv: i32,
    nvml_gpu: String,
    nvml_before: Option<VramSnapshot>,
    rounds: Vec<RoundStats>,
    conclusion: String,
}

fn infer_stats(ok: bool, error: &str, elapsed_ms: f64, m: &Metrics) -> InferStats {
    InferStats {
        ok,
        error: error.to_string(),
        request_ms: elapsed_ms,
        total_tokens_per_second: m.total_tokens_per_second,
        decoded_tokens: m.decoded_tokens,
        request_total_ms: m.request_total_ms,
    }
}

fn empty_stats() -> InferStats {
    InferStats {
        ok: false,
        error: String::new(),
        request_ms: 0.0,
        total_tokens_per_second: 0.0,
        decoded_tokens: 0,
        request_total_ms: 0.0,
    }
}

fn wait_loaded(cluster: &Cluster, id: hds_llama::ffi::InstanceId, secs: u64) -> Result<()> {
    let inst = cluster.wait_loaded(id, Duration::from_secs(secs), Duration::from_millis(300))?;
    if !inst.is_loaded() {
        return Err(EngineError::Other(format!(
            "состояние {} ({})",
            inst.state_name, inst.last_error
        )));
    }
    Ok(())
}

fn clean_ghost(cluster: &Cluster, name: &str) {
    if let Ok(Some(old)) = cluster.find_instance_by_name(name) {
        let _ = cluster.unload(old);
        let _ = cluster.remove_instance(old);
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("ошибка: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    args.model_chat = absolutize(&args.model_chat);
    args.model_embed = absolutize(&args.model_embed);
    args.json = absolutize(&args.json);
    for (label, m) in [("chat", &args.model_chat), ("embed", &args.model_embed)] {
        if !m.is_file() {
            return Err(EngineError::Other(format!(
                "модель {label} не найдена: {}",
                m.display()
            )));
        }
    }

    let engine = Engine::open(args.engine_dir.as_deref())?;
    println!("движок: {} ({})", engine.dir().display(), engine.lib_name());
    let _cwd = engine.activate()?;
    let cluster = engine.create_cluster()?;

    let devices = cluster.devices()?;
    for d in &devices {
        println!(
            "  device index={} backend={} name={} desc={} free={:.0} МиБ total={:.0} МиБ",
            d.bridge_device_index,
            d.backend,
            d.name,
            d.description_or_name(),
            d.memory_free_mib(),
            d.memory_total as f64 / (1024.0 * 1024.0)
        );
    }
    let cuda = devices
        .iter()
        .find(|d| d.backend.eq_ignore_ascii_case("CUDA"))
        .cloned()
        .ok_or_else(|| EngineError::Other("нет устройства backend=CUDA".into()))?;
    let vulkan = devices
        .iter()
        .find(|d| d.backend.eq_ignore_ascii_case("Vulkan"))
        .cloned()
        .ok_or_else(|| {
            EngineError::Other("нет устройства backend=Vulkan (сборка движка без Vulkan?)".into())
        })?;
    println!(
        "целевые карты: chat -> {} (csv={}), embed -> {} (csv={})",
        cuda.name, cuda.bridge_device_index, vulkan.name, vulkan.bridge_device_index
    );

    let nvml = match NvmlProbe::open(args.nvml_index) {
        Ok(p) => {
            println!("NVML: {} (индекс {})", p.name(), p.index());
            Some(p)
        }
        Err(e) => {
            eprintln!("[warn] NVML недоступен ({e}) — VRAM NVIDIA померить нечем");
            None
        }
    };
    let nvml_before = nvml.as_ref().and_then(|p| p.snapshot());

    let mut chat_spec = InstanceSpec::new("a7_chat", &args.model_chat.to_string_lossy());
    chat_spec.manual_devices_csv = Some(cuda.bridge_device_index.to_string());
    chat_spec.allow_cpu = Some(false);
    chat_spec.model_kind = Some(model_kind::TEXT);
    chat_spec.retention_mode = Some(retention::KEEP_LOADED);
    chat_spec.load_on_demand_grace_seconds = Some(300);
    chat_spec.n_ctx = Some(args.n_ctx_chat);
    chat_spec.n_gpu_layers = Some(-1);

    let mut embed_spec = InstanceSpec::new("a7_embed", &args.model_embed.to_string_lossy());
    embed_spec.manual_devices_csv = Some(vulkan.bridge_device_index.to_string());
    embed_spec.allow_cpu = Some(false);
    embed_spec.model_kind = Some(model_kind::EMBEDDINGS);
    // Без этого движок поднимет инстанс без `--embeddings` и вернёт 501
    // ("This server does not support embeddings") на каждый запрос.
    embed_spec.embedding = Some(true);
    embed_spec.retention_mode = Some(retention::KEEP_LOADED);
    embed_spec.load_on_demand_grace_seconds = Some(300);
    embed_spec.n_ctx = Some(args.n_ctx_embed);
    embed_spec.n_gpu_layers = Some(-1);

    clean_ghost(&cluster, "a7_chat");
    clean_ghost(&cluster, "a7_embed");

    if args.no_chat && args.no_embed {
        return Err(EngineError::Other(
            "--no-chat и --no-embed одновременно: нечего запускать".into(),
        ));
    }

    let chat_id: Option<hds_llama::ffi::InstanceId> = if args.no_chat {
        println!("chat-роль пропущена (--no-chat)");
        None
    } else {
        Some(cluster.create_instance(&chat_spec)?)
    };
    let embed_id: Option<hds_llama::ffi::InstanceId> = if args.no_embed {
        println!("embed-роль пропущена (--no-embed)");
        None
    } else {
        Some(cluster.create_instance(&embed_spec)?)
    };
    println!("инстансы созданы: chat={:?}, embed={:?}", chat_id, embed_id);
    let chat_prompt = "Кратко (одним предложением): что такое векторный поиск по документам?";
    let embed_texts = serde_json::json!({
        "input": [
            "Договор поставки оборудования: сроки, стоимость, ответственность сторон.",
            "Порядок приёмки работ и гарантийные обязательства подрядчика.",
            "Оплата производится этапами, приёмка — по акту, претензии в течение 10 дней."
        ],
        "encoding_format": "float"
    })
    .to_string();

    let mut rounds_out: Vec<RoundStats> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for round in 1..=args.rounds {
        println!("\n=== раунд {round} ===");
        let mut rs = RoundStats {
            round,
            chat_load_ms: 0,
            embed_load_ms: 0,
            chat_unload_ms: 0,
            embed_unload_ms: 0,
            chat1: empty_stats(),
            embed1: empty_stats(),
            chat2: empty_stats(),
            embed2: empty_stats(),
            engine_mem_after_load: Vec::new(),
            engine_mem_after_unload: Vec::new(),
            nvml_after_load: None,
            nvml_after_unload: None,
        };

        if let Some(cid) = chat_id {
            let t = Instant::now();
            cluster.load(cid)?;
            rs.chat_load_ms = t.elapsed().as_millis();
            if let Err(e) = wait_loaded(&cluster, cid, 600) {
                failures.push(format!("раунд {round}: загрузка chat: {e}"));
                let _ = cluster.unload(cid);
                break;
            }
            println!("chat LOADED за {} мс", rs.chat_load_ms);
        }

        if let Some(eid) = embed_id {
            let t = Instant::now();
            cluster.load(eid)?;
            rs.embed_load_ms = t.elapsed().as_millis();
            if let Err(e) = wait_loaded(&cluster, eid, 600) {
                failures.push(format!("раунд {round}: загрузка embed: {e}"));
                if let Some(cid) = chat_id {
                    let _ = cluster.unload(cid);
                }
                let _ = cluster.unload(eid);
                break;
            }
            println!(
                "embed LOADED за {} мс (Vulkan, {})",
                rs.embed_load_ms, vulkan.name
            );
        }

        rs.engine_mem_after_load = engine_mem(&cluster)?;
        rs.nvml_after_load = nvml.as_ref().and_then(|p| p.snapshot());
        if let (Some(a), Some(b)) = (rs.nvml_after_load, nvml_before) {
            println!(
                "NVML после загрузок: used={} МиБ (базовый {})",
                a.used_mib, b.used_mib
            );
        }
        for m in &rs.engine_mem_after_load {
            println!(
                "  {} ({}): free={:.0} МиБ / total={:.0} МиБ",
                m.name, m.backend, m.free_mib, m.total_mib
            );
        }

        // interleaved: chat → embed → chat → embed
        if let Some(cid) = chat_id {
            let t = Instant::now();
            let c1 = cluster.chat_complete(cid, chat_prompt, args.n_predict, 0.1, None)?;
            rs.chat1 = infer_stats(
                c1.ok,
                &c1.error,
                t.elapsed().as_millis() as f64,
                &c1.metrics,
            );
            if !c1.ok {
                failures.push(format!("раунд {round}: chat#1: {}", c1.error));
            } else {
                println!(
                    "chat#1 ok: «{}…» ({:.0} мс, {:.1} ток/с)",
                    c1.text.chars().take(60).collect::<String>(),
                    rs.chat1.request_ms,
                    c1.metrics.total_tokens_per_second
                );
            }
        }

        if let Some(eid) = embed_id {
            let t = Instant::now();
            let e1 = cluster.embeddings_json(eid, &embed_texts, true)?;
            rs.embed1 = infer_stats(
                e1.ok,
                &e1.error,
                t.elapsed().as_millis() as f64,
                &e1.metrics,
            );
            if let Err(e) = e1.ensure_ok() {
                failures.push(format!("раунд {round}: embed#1: {e}"));
            } else {
                println!("embed#1 ok ({:.0} мс)", rs.embed1.request_ms);
            }
        }

        if let Some(cid) = chat_id {
            let t = Instant::now();
            let c2 = cluster.chat_complete(cid, chat_prompt, args.n_predict, 0.1, None)?;
            rs.chat2 = infer_stats(
                c2.ok,
                &c2.error,
                t.elapsed().as_millis() as f64,
                &c2.metrics,
            );
            if !c2.ok {
                failures.push(format!("раунд {round}: chat#2: {}", c2.error));
            } else {
                println!(
                    "chat#2 ok ({:.0} мс, {:.1} ток/с)",
                    rs.chat2.request_ms, c2.metrics.total_tokens_per_second
                );
            }
        }

        if let Some(eid) = embed_id {
            let t = Instant::now();
            let e2 = cluster.embeddings_json(eid, &embed_texts, true)?;
            rs.embed2 = infer_stats(
                e2.ok,
                &e2.error,
                t.elapsed().as_millis() as f64,
                &e2.metrics,
            );
            if let Err(e) = e2.ensure_ok() {
                failures.push(format!("раунд {round}: embed#2: {e}"));
            } else {
                println!("embed#2 ok ({:.0} мс)", rs.embed2.request_ms);
            }
        }

        let t = Instant::now();
        if let Some(eid) = embed_id {
            let _ = cluster.unload(eid);
        }
        rs.embed_unload_ms = t.elapsed().as_millis();
        let t = Instant::now();
        if let Some(cid) = chat_id {
            let _ = cluster.unload(cid);
        }
        rs.chat_unload_ms = t.elapsed().as_millis();
        std::thread::sleep(Duration::from_millis(1500));
        rs.engine_mem_after_unload = engine_mem(&cluster)?;
        rs.nvml_after_unload = nvml.as_ref().and_then(|p| p.snapshot());
        println!(
            "unload: embed {} мс, chat {} мс; NVML вернулся к used={} МиБ",
            rs.embed_unload_ms,
            rs.chat_unload_ms,
            rs.nvml_after_unload
                .as_ref()
                .map(|s| s.used_mib)
                .unwrap_or(0)
        );

        rounds_out.push(rs);
    }

    if let Some(eid) = embed_id {
        let _ = cluster.remove_instance(eid);
    }
    if let Some(cid) = chat_id {
        let _ = cluster.remove_instance(cid);
    }
    let roles = match (args.no_chat, args.no_embed) {
        (false, false) => "chat(CUDA)+embed(Vulkan)",
        (true, false) => "embed(Vulkan)",
        (false, true) => "chat(CUDA)",
        (true, true) => "нет ролей",
    };
    let ok_rounds = rounds_out.len();
    let conclusion = if failures.is_empty() && ok_rounds == args.rounds {
        format!(
            "{roles} в одном процессе: {ok_rounds}/{0} раундов без ошибок",
            args.rounds
        )
    } else {
        format!("есть сбои: {}", failures.join("; "))
    };
    println!("\n{conclusion}");

    let report = Report {
        generated: format!(
            "unix={}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ),
        host: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into()),
        engine_dir: engine.dir().display().to_string(),
        engine_lib: engine.lib_name().to_string(),
        model_chat: args.model_chat.display().to_string(),
        model_embed: args.model_embed.display().to_string(),
        n_ctx_chat: args.n_ctx_chat,
        n_ctx_embed: args.n_ctx_embed,
        devices,
        cuda_csv: cuda.bridge_device_index,
        vulkan_csv: vulkan.bridge_device_index,
        nvml_gpu: nvml
            .as_ref()
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| "NVML недоступен".into()),
        nvml_before,
        rounds: rounds_out,
        conclusion,
    };
    if let Some(dir) = args.json.parent() {
        std::fs::create_dir_all(dir).map_err(|e| EngineError::Other(format!("{dir:?}: {e}")))?;
    }
    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| EngineError::Other(format!("сериализация отчёта: {e}")))?;
    std::fs::write(&args.json, json)
        .map_err(|e| EngineError::Other(format!("{}: {e}", args.json.display())))?;
    println!("отчёт: {}", args.json.display());
    if failures.is_empty() {
        Ok(())
    } else {
        Err(EngineError::Other(format!(
            "спайк завершился с {} сбоем(ами)",
            failures.len()
        )))
    }
}
