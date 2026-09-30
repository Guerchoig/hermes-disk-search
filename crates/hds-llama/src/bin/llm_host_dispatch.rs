//! A4 (шаг 2): прогон решения диспетчера VRAM на живом движке.
//!
//! Считает потребность роли «модель + KV», спрашивает арбитр (`plan_query`/`plan_indexing`)
//! и по `--apply` выполняет действия: ставит `index.pause`, выгружает роли по
//! `gpu.priorities`, грузит запрошенную роль, затем снимает паузу. Все замеры —
//! NVML (до/после), потому что `memory_free` движка недостоверен (R29).
//!
//! Это «живая» проверка решений A4 шага 2 до появления резидентного `llm-host`
//! (A5/A6) и основа будущего `tools/parity/arb_scenarios.py`.
//!
//! Запуск из корня репозитория:
//! ```powershell
//! # расчёт влезает/не влезает (ничего не меняя)
//! cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat
//! # с выполнением плана (пауза → выгрузка → загрузка → снятие паузы)
//! cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat --apply
//! # искусственно сузить бюджет (критерий A-7: точные цифры нехватки)
//! cargo run -p hds-llama --release --bin llm_host_dispatch -- --role chat --budget-mb 6000
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hds_llama::budget::{estimate_need_mib, kv_cache_mib};
use hds_llama::config;
use hds_llama::device::describe_devices;
use hds_llama::dispatch::{
    apply, instance_use, plan_indexing, plan_query, Demand, InstanceUse, Plan,
};
use hds_llama::error::{EngineError, Result};
use hds_llama::gguf::{read_meta, KvBits};
use hds_llama::pause::IndexPause;
use hds_llama::runtime::RuntimePaths;
use hds_llama::vram::NvmlProbe;
use hds_llama::{Cluster, Engine, VramProbe};

struct Args {
    config: PathBuf,
    runtime: Option<PathBuf>,
    engine_dir: Option<PathBuf>,
    role: String,
    need_mib: Option<u64>,
    kind: String,
    apply: bool,
    budget_mb: Option<u64>,
    pause_dir: PathBuf,
    json: Option<PathBuf>,
    load_timeout: u64,
}

fn repo_root() -> PathBuf {
    clean(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(".."),
    )
}

fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Нормализовать путь (убрать `..`/`.`) — без обращения к ФС и без `\\?\`-префикса.
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
        role: "chat".to_string(),
        need_mib: None,
        kind: "query".to_string(),
        apply: false,
        budget_mb: None,
        pause_dir: repo_root(),
        json: None,
        load_timeout: 240,
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
            "--role" => args.role = take("--role")?,
            "--kind" => args.kind = take("--kind")?,
            "--pause-dir" => args.pause_dir = PathBuf::from(take("--pause-dir")?),
            "--json" => args.json = Some(PathBuf::from(take("--json")?)),
            "--need-mib" => {
                args.need_mib = Some(
                    take("--need-mib")?
                        .parse()
                        .map_err(|e| format!("--need-mib: {e}"))?,
                )
            }
            "--budget-mb" => {
                args.budget_mb = Some(
                    take("--budget-mb")?
                        .parse()
                        .map_err(|e| format!("--budget-mb: {e}"))?,
                )
            }
            "--load-timeout" => {
                args.load_timeout = take("--load-timeout")?
                    .parse()
                    .map_err(|e| format!("--load-timeout: {e}"))?
            }
            "--apply" => args.apply = true,
            "--help" | "-h" => {
                return Err("использование: llm_host_dispatch --role chat|embedding|rerank|whisper \
                            [--kind query|indexing] [--need-mib N] [--apply] [--budget-mb N] \
                            [--config FILE] [--runtime DIR] [--engine-dir DIR] [--pause-dir DIR] \
                            [--json FILE] [--load-timeout SEC]"
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

/// Оценка «модель + KV» для роли: то, что арбитр считает потребностью.
fn need_for_role(
    cfg: &config::LlmHostConfig,
    root: &Path,
    role: &str,
) -> Result<(PathBuf, u64, u64)> {
    let rc = cfg
        .role(role)
        .ok_or_else(|| EngineError::Other(format!("роль '{role}' не описана в конфиге")))?;
    let path = hds_llama::runtime::resolve_model_checked(root, &rc.model_spec, role)?;
    let file_mib = std::fs::metadata(&path).map(|m| m.len() >> 20).unwrap_or(0);
    let meta = read_meta(&path)?;
    let n_ctx = rc.n_ctx.max(0) as i64;
    let kv = kv_cache_mib(&meta, n_ctx, cfg.parallel.max(1) as i64, KvBits::F16);
    let need = estimate_need_mib(&meta, file_mib, n_ctx, cfg.parallel.max(1) as i64, KvBits::F16);
    println!(
        "роль '{role}': файл {file_mib} МиБ, слоёв {}, голов KV {}, n_ctx {n_ctx}, KV f16 {kv:.0} МиБ \
         → потребность {need} МиБ",
        meta.block_count,
        meta.head_count_kv
    );
    Ok((path, need, file_mib))
}

/// Снимок инстансов для арбитра: сколько освободит каждый (оценка «модель + KV»).
fn snapshot_uses(
    cluster: &Cluster,
    cfg: &config::LlmHostConfig,
    root: &Path,
) -> Result<Vec<InstanceUse>> {
    let mut needs: BTreeMap<String, u64> = BTreeMap::new();
    let mut uses = Vec::new();
    for inst in cluster.instances()? {
        let need = match needs.get(&inst.name) {
            Some(v) => *v,
            None => {
                let v = match need_for_role(cfg, root, &inst.name) {
                    Ok((_, need, _)) => need,
                    Err(_) => 0,
                };
                needs.insert(inst.name.clone(), v);
                v
            }
        };
        uses.push(instance_use(&inst, &inst.name.clone(), need, 0));
    }
    Ok(uses)
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
    let mut cfg = config::load(&args.config)?;
    if let Some(cap) = args.budget_mb {
        // ручка критерия A-7: искусственно сузить бюджет и увидеть точные цифры нехватки
        cfg.gpu.vram_budget_mb = Some(cap);
        println!("искусственное сужение бюджета: gpu.vram_budget_mb = {cap}");
    }
    let paths = match &args.runtime {
        Some(r) => RuntimePaths::new(r.clone()),
        None => RuntimePaths::from_env()?,
    };

    let probe = match NvmlProbe::open(0) {
        Ok(p) => Some(p),
        Err(e) => {
            println!("NVML недоступен ({e}) — решение будет без гарантий (verdict unknown)");
            None
        }
    };
    let before = probe.as_ref().and_then(|p| p.snapshot());
    if let (Some(p), Some(v)) = (probe.as_ref(), before) {
        println!(
            "NVML {}: занято {} / всего {} МиБ, свободно {} МиБ",
            p.name(),
            v.used_mib,
            v.total_mib,
            v.free_mib
        );
    }

    let (model, need, file_mib) = need_for_role(&cfg, &paths.root, &args.role)?;
    let need = args.need_mib.unwrap_or(need);
    if args.need_mib.is_some() {
        println!("потребность задана вручную: {need} МиБ");
    }
    let demand = Demand::new(&args.role, need);

    let engine = Engine::open(args.engine_dir.as_deref())?;
    let _cwd = engine.activate()?; // cwd движка обязателен (грабли A1/A2)
    let cluster = engine.create_cluster()?;
    let devices = cluster.devices()?;
    println!("движок: {} ({})", engine.dir().display(), engine.lib_name());
    println!("устройства: {}", describe_devices(&devices));

    let uses = snapshot_uses(&cluster, &cfg, &paths.root)?;
    println!(
        "инстансы в этом процессе ({}): {}",
        uses.len(),
        if uses.is_empty() {
            "нет (создаёт llm-host при старте — A6)".to_string()
        } else {
            uses.iter()
                .map(|u| format!(
                    "{}[{}/{}, {} МиБ, простой {} с]",
                    u.name, u.role, u.retention_label(), u.vram_mib, u.idle_secs
                ))
                .collect::<Vec<_>>()
                .join("; ")
        }
    );

    let pause = Arc::new(IndexPause::new(args.pause_dir.clone()));
    println!(
        "index.pause: {} ({})",
        if pause.is_paused() { "стоит" } else { "нет" },
        pause.path().display()
    );

    let plan: Plan = if args.kind == "indexing" {
        plan_indexing(&cfg.gpu, before.map(|v| v.free_mib), &demand, &uses)
    } else {
        plan_query(&cfg.gpu, before.map(|v| v.free_mib), &demand, &uses)
    };
    println!("\nрешение диспетчера:");
    for line in plan.lines() {
        println!("  {line}");
    }

    let mut log = Vec::new();
    if args.apply {
        println!("\nвыполняю действия:");
        log = apply(&cluster, &pause, &plan);
        for line in &log {
            println!("  {line}");
        }
        // ждём загрузку запрошенной роли (если её инстанс существует) и мерим NVML
        if let Some(id) = cluster.find_instance_by_name(&args.role)? {
            match cluster.wait_loaded(
                id,
                Duration::from_secs(args.load_timeout),
                Duration::from_millis(500),
            ) {
                Ok(inst) => println!("  роль '{}': состояние {}", args.role, inst.state_name),
                Err(e) => println!("  роль '{}': {e}", args.role),
            }
        }
        if pause.depth() > 0 {
            // запрос «завершён» — снимаем **нашу** паузу (ARB-2); чужую не трогаем
            let resumed = pause.resume().unwrap_or(false);
            println!(
                "  пауза снята: {resumed} (сейчас пауза: {})",
                pause.is_paused()
            );
        }
    }

    let after = probe.as_ref().and_then(|p| p.snapshot());
    if let (Some(b), Some(a)) = (before, after) {
        println!(
            "\nNVML после: занято {} МиБ (дельта {}), свободно {} МиБ (дельта {})",
            a.used_mib,
            a.used_mib as i64 - b.used_mib as i64,
            a.free_mib,
            a.free_mib as i64 - b.free_mib as i64
        );
    }

    if let Some(json_path) = &json_path {
        save_json(
            json_path, &cfg, &args, &plan, &log, &devices, &uses, before, after, file_mib, need,
            &model,
        )?;
    }
    print!("\n");
    // код возврата как у проверки приёмки: не влезает → 1 (само решение — это отчёт, не деградация)
    if !plan.verdict.is_ok() {
        std::process::exit(1);
    }
    Ok(())
}

/// Машинночитаемый отчёт проверки (основа будущего `tools/parity/arb_scenarios.py`).
#[allow(clippy::too_many_arguments)]
fn save_json(
    path: &Path,
    cfg: &config::LlmHostConfig,
    args: &Args,
    plan: &Plan,
    log: &[String],
    devices: &[hds_llama::Device],
    uses: &[InstanceUse],
    before: Option<hds_llama::VramSnapshot>,
    after: Option<hds_llama::VramSnapshot>,
    file_mib: u64,
    need_mib: u64,
    model: &Path,
) -> Result<()> {
    let report = serde_json::json!({
        "config": cfg.path.display().to_string(),
        "role": args.role,
        "kind": args.kind,
        "model": model.display().to_string(),
        "file_mib": file_mib,
        "need_mib": need_mib,
        "free_mib_before": before.map(|v| v.free_mib),
        "used_mib_before": before.map(|v| v.used_mib),
        "used_mib_after": after.map(|v| v.used_mib),
        "verdict": plan.verdict.as_str(),
        "freed_mib": plan.freed_mib,
        "actions": plan.actions.iter().map(|a| a.describe()).collect::<Vec<_>>(),
        "notes": plan.notes,
        "applied": args.apply,
        "apply_log": log,
        "gpu": {
            "policy": cfg.gpu.policy.as_str(),
            "reserve_mb": cfg.gpu.reserve_mb,
            "budget_mb": cfg.gpu.vram_budget_mb,
            "external_vram_mb": cfg.gpu.external_vram_mb,
            "evict_idle_sec": cfg.gpu.evict_idle_sec,
            "priorities": cfg.gpu.priorities,
        },
        "devices": devices,
        "instances": uses.iter().map(|u| serde_json::json!({
            "name": u.name, "role": u.role, "vram_mib": u.vram_mib,
            "state": u.state, "retention": u.retention_label(),
            "active": u.active_requests, "idle_secs": u.idle_secs,
            "grace_seconds": u.grace_seconds,
        })).collect::<Vec<_>>(),
    });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let text = serde_json::to_string_pretty(&report)
        .map_err(|e| EngineError::Other(format!("json: {e}")))?;
    std::fs::write(path, text).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    println!("\nотчёт: {}", path.display());
    Ok(())
}
