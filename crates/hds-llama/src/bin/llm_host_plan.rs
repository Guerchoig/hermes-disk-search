//! A2: план инстансов `llm-host` по конфигу — без загрузки моделей.
//!
//! Показывает, что именно будет поднято: роль → файл модели (после разрешения
//! `shared:<role>` в общем рантайме), устройство (`manual_devices_csv`),
//! `n_ctx`/`n_gpu_layers`/`allow_cpu`, retention, порт и замечания о переносе
//! legacy-ключей. Это «сухой прогон» перед A4 (диспетчер VRAM) и основа для
//! `hdsw llm-host status`.
//!
//! Запуск из корня репозитория:
//! `cargo run -p hds-llama --release --bin llm_host_plan -- [--config FILE]
//!  [--runtime DIR] [--engine-dir DIR] [--json FILE]`

use std::path::{Path, PathBuf};

use hds_llama::config;
use hds_llama::device::describe_devices;
use hds_llama::error::{EngineError, Result};
use hds_llama::registry::{self, PlannedInstance};
use hds_llama::runtime::RuntimePaths;
use hds_llama::Engine;

struct Args {
    config: PathBuf,
    runtime: Option<PathBuf>,
    engine_dir: Option<PathBuf>,
    json: Option<PathBuf>,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// Абсолютный путь **до** переключения текущего каталога на каталог движка:
/// после `Engine::activate()` относительный путь указывал бы внутрь `Engine`
/// (побочный эффект `EngineCwd`, найденный на первом же прогоне A2).
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
            "--help" | "-h" => {
                return Err("использование: llm_host_plan [--config FILE] [--runtime DIR] \
                            [--engine-dir DIR] [--json FILE]"
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

fn run() -> Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };

    let cfg = config::load(&args.config)?;
    let json_path = args.json.as_ref().map(|p| absolutize(p));
    println!("конфиг: {}", cfg.path.display());
    println!(
        "режим llm: {} | политика модели: {} | host {} | parallel {} | autostart {}",
        cfg.mode.as_str(),
        cfg.model_policy,
        cfg.host,
        cfg.parallel,
        cfg.autostart
    );
    println!(
        "gpu: device_index={} n_gpu_layers={}{} reserve={} МиБ evict_idle={} с budget={}",
        cfg.gpu.device_index,
        cfg.gpu.n_gpu_layers,
        if cfg.gpu.n_gpu_layers_set { "" } else { " (дефолт)" },
        cfg.gpu.reserve_mb,
        cfg.gpu.evict_idle_sec,
        cfg.gpu
            .vram_budget_mb
            .map(|v| v.to_string())
            .unwrap_or_else(|| "не задан".into())
    );
    for w in &cfg.warnings {
        println!("  [warn] {w}");
    }

    let paths = match &args.runtime {
        Some(r) => RuntimePaths::new(r.clone()),
        None => RuntimePaths::from_env()?,
    };
    println!(
        "рантайм: {} (bin: {}, есть llama-server: {})",
        paths.root.display(),
        paths.bin.display(),
        paths.has_llama_server()
    );

    // Устройства нужны для маппинга gpu.device_index → manual_devices_csv
    let engine = Engine::open(args.engine_dir.as_deref())?;
    println!("движок: {} ({})", engine.dir().display(), engine.lib_name());
    let _cwd = engine.activate()?;
    let cluster = engine.create_cluster()?;
    let devices = cluster.devices()?;
    println!("устройства: {}", describe_devices(&devices));
    if devices.is_empty() {
        return Err(EngineError::Other(
            "движок не вернул ни одного устройства — план без маппинга устройства бессмыслен"
                .to_string(),
        ));
    }

    let planned = registry::plan(&cfg, &paths.root, &devices);
    println!("\nплан инстансов ({}):", planned.len());
    let mut failed = 0usize;
    for p in &planned {
        match p {
            registry::RolePlan::Ready(inst) => {
                println!("  {}", inst.summary());
                for n in &inst.notes {
                    println!("    ^ {n}");
                }
            }
            registry::RolePlan::Failed { role, error } => {
                failed += 1;
                println!("  {role:<9} НЕ СПЛАНИРОВАНА: {error}");
            }
        }
    }
    save_json(json_path.as_ref(), &cfg, &paths, &engine, &devices, &planned)?;
    if failed > 0 {
        println!(
            "\nролей без модели: {failed} — положите файлы в общий рантайм \
             (installers/ensure_llama_runtime.ps1) или укажите путь в конфиге"
        );
        std::process::exit(1);
    }
    Ok(())
}

/// Записать машинночитаемый план (для приёмки W2 и будущего `status --json`).
fn save_json(
    json_path: Option<&PathBuf>,
    cfg: &config::LlmHostConfig,
    paths: &RuntimePaths,
    engine: &Engine,
    devices: &[hds_llama::Device],
    planned: &[registry::RolePlan],
) -> Result<()> {
    let Some(json_path) = json_path else {
        return Ok(());
    };
    let report = serde_json::json!({
        "config": cfg.path.display().to_string(),
        "mode": cfg.mode.as_str(),
        "model_policy": cfg.model_policy,
        "runtime_root": paths.root.display().to_string(),
        "engine_dir": engine.dir().display().to_string(),
        "gpu": {
            "device_index": cfg.gpu.device_index,
            "n_gpu_layers": cfg.gpu.n_gpu_layers,
            "reserve_mb": cfg.gpu.reserve_mb,
            "evict_idle_sec": cfg.gpu.evict_idle_sec,
            "priorities": cfg.gpu.priorities,
        },
        "devices": devices,
        "warnings": cfg.warnings,
        "instances": planned.iter().filter_map(|p| p.ready()).map(|p: &PlannedInstance| serde_json::json!({
            "role": p.role,
            "port": p.port,
            "model_spec": p.model_spec,
            "model_path": p.model_path.display().to_string(),
            "devices_csv": p.spec.manual_devices_csv,
            "model_kind": p.spec.model_kind,
            "n_ctx": p.spec.n_ctx,
            "n_gpu_layers": p.spec.n_gpu_layers,
            "retention": p.spec.retention_mode,
            "allow_cpu": p.spec.allow_cpu,
            "notes": p.notes,
        })).collect::<Vec<_>>(),
        "failed": planned.iter().filter_map(|p| match p {
            registry::RolePlan::Failed { role, error } => Some(serde_json::json!({
                "role": role, "error": error,
            })),
            registry::RolePlan::Ready(_) => None,
        }).collect::<Vec<_>>(),
    });
    if let Some(dir) = json_path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let text = serde_json::to_string_pretty(&report)
        .map_err(|e| EngineError::Other(format!("json: {e}")))?;
    std::fs::write(json_path, text)
        .map_err(|e| EngineError::Other(format!("{}: {e}", json_path.display())))?;
    println!("\nотчёт: {}", json_path.display());
    Ok(())
}

