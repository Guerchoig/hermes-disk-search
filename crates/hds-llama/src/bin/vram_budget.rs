//! A4 (шаг 1): бюджет VRAM по ролям — «модель + KV» против реальной свободной
//! памяти (NVML), без загрузки моделей и без авто-деградации.
//!
//! Показывает по каждой роли: размер GGUF, оценку KV (f16 и q8_0), полную
//! потребность, свободную VRAM и вердикт «влезает / не хватает» с точными
//! цифрами — это то, что `hdsw check` и UI будут показывать пользователю.
//!
//! Важно (A4): у cluster API **нет** ручки типа KV-кэша (`kv_unified=1`,
//! `no_kv_offload=0`, `cache-type-*` не в `instance_params`), поэтому честная
//! оценка для W2 — **f16**; вариант q8_0 печатается для сравнения с прежним
//! llama-server (`--cache-type-k/v q8_0`).
//!
//! Запуск:
//! `cargo run -p hds-llama --release --bin vram_budget -- [--config FILE]
//!  [--runtime DIR] [--engine-dir DIR] [--json FILE] [--reserve-mb N]`

use std::path::{Path, PathBuf};

use hds_llama::budget::{check_fit, estimate_need_mib, kv_cache_mib};
use hds_llama::config;
use hds_llama::error::{EngineError, Result};
use hds_llama::gguf::{read_meta, KvBits};
use hds_llama::registry::{self, RolePlan};
use hds_llama::runtime::RuntimePaths;
use hds_llama::vram::NvmlProbe;
use hds_llama::{Engine, VramProbe};

struct Args {
    config: PathBuf,
    runtime: Option<PathBuf>,
    engine_dir: Option<PathBuf>,
    json: Option<PathBuf>,
    reserve_mb: Option<u64>,
    nvml_index: u32,
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
        json: None,
        reserve_mb: None,
        nvml_index: 0,
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
            "--json" => args.json = Some(PathBuf::from(take("--json")?)),
            "--reserve-mb" => {
                args.reserve_mb = Some(
                    take("--reserve-mb")?
                        .parse()
                        .map_err(|e| format!("--reserve-mb: {e}"))?,
                )
            }
            "--nvml-index" => {
                args.nvml_index = take("--nvml-index")?
                    .parse()
                    .map_err(|e| format!("--nvml-index: {e}"))?
            }
            "--help" | "-h" => {
                return Err(
                    "использование: vram_budget [--config FILE] [--runtime DIR] \
                            [--engine-dir DIR] [--json FILE] [--reserve-mb N]"
                        .to_string(),
                )
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

fn run() -> Result<()> {
    let mut args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    if let Some(p) = &args.json {
        args.json = Some(absolutize(p));
    }

    let cfg = config::load(&args.config)?;
    let reserve = args.reserve_mb.unwrap_or(cfg.gpu.reserve_mb);
    let paths = match &args.runtime {
        Some(r) => RuntimePaths::new(r.clone()),
        None => RuntimePaths::from_env()?,
    };

    let engine = Engine::open(args.engine_dir.as_deref())?;
    let _cwd = engine.activate()?;
    let cluster = engine.create_cluster()?;
    let devices = cluster.devices()?;
    let planned = registry::plan(&cfg, &paths.root, &devices);

    // Свободная VRAM: NVML — источник истины (R29)
    let probe = NvmlProbe::open(args.nvml_index).ok();
    let free_mib = probe
        .as_ref()
        .and_then(|p| p.snapshot())
        .map(|s| s.free_mib);
    println!(
        "NVML: {} | свободно {} МиБ | резерв {} МиБ | gpu.device_index={}",
        probe
            .as_ref()
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| "недоступен".to_string()),
        free_mib
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".to_string()),
        reserve,
        cfg.gpu.device_index
    );
    if probe.is_none() {
        println!("[warn] без NVML вердикт «влезает/не влезает» недостоверен (R29)");
    }

    let mut rows = Vec::new();
    let mut chat_ok = true;
    for p in &planned {
        let RolePlan::Ready(inst) = p else {
            if let RolePlan::Failed { role, error } = p {
                println!("{role:<9} НЕ СПЛАНИРОВАНА: {error}");
            }
            continue;
        };
        let file_mib = std::fs::metadata(&inst.model_path)
            .map(|m| m.len() / (1024 * 1024))
            .unwrap_or(0);
        let meta = match read_meta(&inst.model_path) {
            Ok(m) => m,
            Err(e) => {
                println!(
                    "{:<9} метаданные GGUF не прочитаны ({e}) — оценка только по размеру файла",
                    inst.role
                );
                continue;
            }
        };
        let n_ctx = inst.spec.n_ctx.unwrap_or(0) as i64;
        let n_parallel = cfg.parallel as i64;
        let kv_f16 = kv_cache_mib(&meta, n_ctx, n_parallel, KvBits::F16);
        let kv_q8 = kv_cache_mib(&meta, n_ctx, n_parallel, KvBits::Q8_0);
        // честная оценка W2 — f16: у cluster API нет ручки типа KV (см. модуль)
        let need = estimate_need_mib(&meta, file_mib, n_ctx, n_parallel, KvBits::F16);
        let fit = check_fit(free_mib, need, reserve);
        if inst.role == "chat" && !fit.is_ok() {
            chat_ok = false;
        }
        println!(
            "{:<9} файл {:>5} МиБ | слоёв {:>3} (KV {:>2}) | голов KV {:>3} | head_dim {:>4} | n_ctx {:>6} | \
             KV f16 {:>6.0} / q8_0 {:>6.0} МиБ | нужно {:>5} МиБ -> {}",
            inst.role,
            file_mib,
            meta.block_count,
            meta.kv_layer_count(),
            meta.head_count_kv,
            meta.head_dim(),
            n_ctx,
            kv_f16,
            kv_q8,
            need,
            fit.message()
        );
        rows.push(serde_json::json!({
            "role": inst.role,
            "model": inst.model_path.display().to_string(),
            "file_mib": file_mib,
            "block_count": meta.block_count,
            "kv_layer_count": meta.kv_layer_count(),
            "full_attention_interval": meta.full_attention_interval,
            "head_count_kv": meta.head_count_kv,
            "head_dim": meta.head_dim(),
            "n_ctx": n_ctx,
            "kv_f16_mib": kv_f16.round(),
            "kv_q8_0_mib": kv_q8.round(),
            "need_mib": need,
            "verdict": if fit.is_ok() { "fits" } else { "not_enough" },
            "message": fit.message(),
        }));
    }

    if let Some(json) = &args.json {
        let report = serde_json::json!({
            "config": cfg.path.display().to_string(),
            "nvml_gpu": probe.as_ref().map(|p| p.name().to_string()),
            "free_mib": free_mib,
            "reserve_mib": reserve,
            "kv_type_assumed": "f16 (у cluster API нет ручки типа KV)",
            "roles": rows,
        });
        if let Some(dir) = json.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| EngineError::Other(format!("json: {e}")))?;
        std::fs::write(json, text)
            .map_err(|e| EngineError::Other(format!("{}: {e}", json.display())))?;
        println!("\nотчёт: {}", json.display());
    }

    if !chat_ok {
        println!(
            "\nвнимание: чат-модель не влезает при текущей занятой VRAM — \
                  выгрузите индексные роли (A4) или смените модель в UI; \
                  авто-деградации нет (решение заказчика 29.09.2026)"
        );
    }
    Ok(())
}
