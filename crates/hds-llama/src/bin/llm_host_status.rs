//! A4 (шаг 2): `hdsw llm-host status` — наблюдаемость диспетчера VRAM.
//!
//! Показывает по факту запуска: устройства движка, свободную VRAM (NVML — источник
//! истины, R29), бюджет (`gpu.reserve_mb`/cap/вычет), состояние `index.pause` и
//! heartbeat индексации, роли с их состоянием/контекстом/потребностью и
//! последнее решение диспетчера. Плюс `--json` — те же поля для UI/MCP.
//!
//! Запуск из корня репозитория:
//! `cargo run -p hds-llama --release --bin llm_host_status -- [--config FILE]
//!  [--runtime DIR] [--engine-dir DIR] [--json FILE] [--baseline-used-mib N]
//!  [--pause-dir DIR] [--no-engine]`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hds_llama::budget::estimate_need_mib;
use hds_llama::config;
use hds_llama::dispatch::{instance_use, plan_query, Demand, InstanceUse};
use hds_llama::error::{EngineError, Result};
use hds_llama::ffi::state;
use hds_llama::gguf::{read_meta, KvBits};
use hds_llama::pause::{read_heartbeat, IndexPause};
use hds_llama::registry;
use hds_llama::runtime::RuntimePaths;
use hds_llama::status::{StatusInput, StatusReport};
use hds_llama::vram::NvmlProbe;
use hds_llama::{Engine, Instance, VramProbe};

struct Args {
    config: PathBuf,
    runtime: Option<PathBuf>,
    engine_dir: Option<PathBuf>,
    json: Option<PathBuf>,
    baseline_used_mib: Option<u64>,
    pause_dir: PathBuf,
    no_engine: bool,
    nvml_index: u32,
}

fn repo_root() -> PathBuf {
    clean(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(".."),
    )
}

/// Абсолютный путь до переключения cwd на каталог движка (`Engine::activate`).
fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Нормализовать путь (убрать `..`/`.`) — без обращения к ФС и без `\\?\`-префикса,
/// который добавляет `canonicalize`: в логах и отчётах он только мешает.
fn clean(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut args = Args {
        config: std::env::var("HDS_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root().join("config.yaml")),
        runtime: None,
        engine_dir: None,
        json: None,
        baseline_used_mib: None,
        pause_dir: repo_root(),
        no_engine: false,
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
            "--pause-dir" => args.pause_dir = PathBuf::from(take("--pause-dir")?),
            "--baseline-used-mib" => {
                args.baseline_used_mib = Some(
                    take("--baseline-used-mib")?
                        .parse()
                        .map_err(|e| format!("--baseline-used-mib: {e}"))?,
                )
            }
            "--nvml-index" => {
                args.nvml_index = take("--nvml-index")?
                    .parse()
                    .map_err(|e| format!("--nvml-index: {e}"))?
            }
            "--no-engine" => args.no_engine = true,
            "--help" | "-h" => {
                return Err("использование: llm_host_status [--config FILE] [--runtime DIR] \
                            [--engine-dir DIR] [--json FILE] [--baseline-used-mib N] \
                            [--pause-dir DIR] [--nvml-index N] [--no-engine]"
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

/// Оценка «модель + KV» по ролям — без движка (только файлы GGUF из рантайма).
fn role_needs(cfg: &config::LlmHostConfig, root: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for rc in &cfg.roles {
        let Ok(path) = hds_llama::runtime::resolve_model_checked(root, &rc.model_spec, &rc.role)
        else {
            continue;
        };
        let file_mib = std::fs::metadata(&path).map(|m| m.len() >> 20).unwrap_or(0);
        let need = match read_meta(&path) {
            Ok(meta) => estimate_need_mib(
                &meta,
                file_mib,
                rc.n_ctx.max(0) as i64,
                cfg.parallel.max(1) as i64,
                // честная оценка W2 — f16: у cluster API нет ручки типа KV (A4 шаг 1)
                KvBits::F16,
            ),
            Err(_) => file_mib + (file_mib as f64 * 0.05).ceil() as u64,
        };
        out.insert(rc.role.clone(), need);
    }
    out
}

/// Диагностика по роли: сколько освободит выгрузка (нужно арбитру и статусу).
fn instance_uses(
    instances: &[Instance],
    needs: &BTreeMap<String, u64>,
) -> Vec<InstanceUse> {
    instances
        .iter()
        .map(|i| {
            let need = needs.get(&i.name).copied().unwrap_or(0);
            // простой ведёт резидентный llm-host; в разовом прогоне считаем «только что» (0)
            instance_use(i, &i.name.clone(), need, 0)
        })
        .collect()
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

    let paths = match &args.runtime {
        Some(r) => RuntimePaths::new(r.clone()),
        None => RuntimePaths::from_env()?,
    };

    // --- NVML: источник истины по свободной VRAM (R29) ---
    let probe = if cfg.gpu.vram_source.is_trusted() {
        match NvmlProbe::open(args.nvml_index) {
            Ok(p) => Some(p),
            Err(e) => {
                println!("NVML недоступен ({e}) — бюджет VRAM не проверяется");
                None
            }
        }
    } else {
        println!(
            "gpu.vram_source = {}: цифры движка недостоверны (R29), полагаемся на них только \
             как на подсказку",
            cfg.gpu.vram_source.as_config_key()
        );
        None
    };
    let vram = probe.as_ref().and_then(|p| p.snapshot());

    // --- движок: устройства и инстансы (можно пропустить: --no-engine) ---
    let mut engine_dir = None;
    let mut devices = Vec::new();
    let mut instances = Vec::new();
    if !args.no_engine {
        match Engine::open(args.engine_dir.as_deref()) {
            Ok(engine) => {
                engine_dir = Some(engine.dir().to_path_buf());
                let _cwd = engine.activate()?; // cwd движка обязателен (грабли A1/A2)
                let cluster = engine.create_cluster()?;
                devices = cluster.devices()?;
                instances = cluster.instances()?;
            }
            Err(e) => println!("движок недоступен: {e}"),
        }
    }

    let needs = role_needs(&cfg, &paths.root);
    let planned = if devices.is_empty() {
        Vec::new()
    } else {
        registry::plan(&cfg, &paths.root, &devices)
    };

    // --- пауза индексации и heartbeat ---
    let pause = IndexPause::new(args.pause_dir.clone());
    let heartbeat = read_heartbeat(&args.pause_dir);

    // Прогноз диспетчера: что будет, если запрос чат-роли придёт прямо сейчас
    // (разовому `status` важно показать решение; резидентный `llm-host` передаст
    // сюда своё фактическое последнее решение).
    let uses = instance_uses(&instances, &needs);
    let forecast = cfg.role("chat").map(|_| {
        plan_query(
            &cfg.gpu,
            vram.map(|v| v.free_mib),
            &Demand::new("chat", needs.get("chat").copied().unwrap_or(0)),
            &uses,
        )
    });

    let report = StatusReport::build(StatusInput {
        config: &cfg,
        runtime_root: &paths.root,
        engine_dir: engine_dir.as_deref(),
        devices: &devices,
        instances: &instances,
        vram,
        vram_source: cfg.gpu.vram_source,
        baseline_used_mib: args.baseline_used_mib,
        paused: pause.is_paused(),
        pause_file: pause.path(),
        heartbeat,
        planned: &planned,
        needs: &needs,
        decision: None,
        forecast: forecast.as_ref().map(|p| ("chat", p)),
    });

    for line in report.lines() {
        println!("{line}");
    }
    println!(
        "пауза индексации: файл {} — {}, вложенность {}",
        pause.path().display(),
        if pause.is_paused() { "стоит" } else { "нет" },
        pause.depth()
    );
    if !instances.is_empty() {
        println!(
            "инстансы кластера: {}",
            uses.iter()
                .map(|i| format!(
                    "{}[{}/{}, активных {}]",
                    i.name,
                    state::name(i.state),
                    i.retention_label(),
                    i.active_requests
                ))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    if let Some(json_path) = &json_path {
        if let Some(dir) = json_path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let text = serde_json::to_string_pretty(&report.json())
            .map_err(|e| EngineError::Other(format!("json: {e}")))?;
        std::fs::write(json_path, text)
            .map_err(|e| EngineError::Other(format!("{}: {e}", json_path.display())))?;
        println!("\nотчёт: {}", json_path.display());
    }

    // код возврата: есть сломанные роли/предупреждения о модели — 1 (для автопроверок)
    let broken = report.roles.iter().any(|r| {
        r.state_code == Some(state::FAILED)
            || r.notes.iter().any(|n| n.contains("не спланирована"))
    });
    if broken {
        std::process::exit(1);
    }
    Ok(())
}
