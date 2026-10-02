//! Замер **фактического** KV-кэша движка (решение заказчика 30.09.2026 по
//! `llm.chat.n_ctx`): грузим модель кластерным инстансом и мерим рост занятой
//! VRAM по NVML (источник истины, R29), а не по формуле из метаданных.
//!
//! Метод — **дифференциальный по `n_ctx`**: тот же `n_gpu_layers`, два разных
//! `n_ctx`; разница занятой VRAM = KV, выросший на разницу токенов. Так из замера
//! выпадают веса, compute-буферы и контекст CUDA (они одинаковы), а остаётся
//! чистая «цена токена» KV. Это позволяет измерить KV даже там, где полный офлоад
//! не влезает в свободную VRAM (машина занята ролями Python-версии).
//!
//! Дополнительно печатается формула из метаданных (с учётом
//! `full_attention_interval` у гибридных моделей) и вывод «сколько будет при
//! полном офлоаде» с проверкой «влезает ли в 12 ГБ».
//!
//! Запуск из корня репозитория:
//! ```powershell
//! # дифференциальный замер: 4096 против 32768 при офлоаде 8 слоёв (≈2 ГБ VRAM)
//! cargo run -p hds-llama --release --bin kv_probe -- --role chat --ngl 8 --n-ctx 4096,32768
//! # полный офлоад (нужна свободная VRAM ~8,6 ГБ)
//! cargo run -p hds-llama --release --bin kv_probe -- --role chat --n-ctx 32768
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hds_llama::budget::kv_cache_mib;
use hds_llama::cluster::InstanceSpec;
use hds_llama::config;
use hds_llama::error::{EngineError, Result};
use hds_llama::ffi::{model_kind, retention};
use hds_llama::gguf::{read_meta, GgufMeta, KvBits};
use hds_llama::runtime::RuntimePaths;
use hds_llama::vram::NvmlProbe;
use hds_llama::{Cluster, Engine, VramProbe};

struct Args {
    config: PathBuf,
    runtime: Option<PathBuf>,
    engine_dir: Option<PathBuf>,
    role: String,
    ngl: i32,
    n_ctx: Vec<i64>,
    json: Option<PathBuf>,
    load_timeout: u64,
    settle_secs: u64,
    nvml_index: u32,
    min_margin_mib: u64,
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
        role: "chat".to_string(),
        ngl: -1,
        n_ctx: vec![4096, 32768],
        json: None,
        load_timeout: 300,
        settle_secs: 2,
        nvml_index: 0,
        min_margin_mib: 256,
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
            "--json" => args.json = Some(PathBuf::from(take("--json")?)),
            "--n-ctx" => {
                args.n_ctx = take("--n-ctx")?
                    .split(',')
                    .map(|s| s.trim().parse::<i64>().map_err(|e| format!("--n-ctx: {e}")))
                    .collect::<std::result::Result<Vec<_>, _>>()?
            }
            "--ngl" => args.ngl = take("--ngl")?.parse().map_err(|e| format!("--ngl: {e}"))?,
            "--load-timeout" => {
                args.load_timeout = take("--load-timeout")?
                    .parse()
                    .map_err(|e| format!("--load-timeout: {e}"))?
            }
            "--settle" => {
                args.settle_secs = take("--settle")?
                    .parse()
                    .map_err(|e| format!("--settle: {e}"))?
            }
            "--min-margin-mib" => {
                args.min_margin_mib = take("--min-margin-mib")?
                    .parse()
                    .map_err(|e| format!("--min-margin-mib: {e}"))?
            }
            "--nvml-index" => {
                args.nvml_index = take("--nvml-index")?
                    .parse()
                    .map_err(|e| format!("--nvml-index: {e}"))?
            }
            "--help" | "-h" => {
                return Err(
                    "использование: kv_probe --role chat [--ngl N] [--n-ctx A[,B]] \
                            [--config FILE] [--runtime DIR] [--engine-dir DIR] [--json FILE] \
                            [--load-timeout SEC] [--settle SEC] [--nvml-index N]"
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
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    let json_path = args.json.as_ref().map(|p| absolutize(p));
    let cfg = config::load(&args.config)?;
    let rc = cfg
        .role(&args.role)
        .ok_or_else(|| EngineError::Other(format!("роль '{}' не описана в конфиге", args.role)))?;
    let paths = match &args.runtime {
        Some(r) => RuntimePaths::new(r.clone()),
        None => RuntimePaths::from_env()?,
    };
    let model = hds_llama::runtime::resolve_model_checked(&paths.root, &rc.model_spec, &args.role)?;
    let file_mib = std::fs::metadata(&model)
        .map(|m| m.len() >> 20)
        .unwrap_or(0);
    let meta = read_meta(&model)?;
    println!(
        "модель: {} ({file_mib} МиБ)\nроль '{}': слоёв {}, голов KV {}, key/value {} / {}, \
         full_attention_interval {:?} → KV держат {} слоёв из {}",
        model.display(),
        args.role,
        meta.block_count,
        meta.head_count_kv,
        meta.key_length.unwrap_or(meta.head_dim()),
        meta.value_length.unwrap_or(meta.head_dim()),
        meta.full_attention_interval,
        meta.kv_layer_count(),
        meta.block_count
    );

    let probe = NvmlProbe::open(args.nvml_index)?;
    // базовая линия: спокойная занятость ДО наших загрузок (роли Python-версии и пр.)
    std::thread::sleep(Duration::from_secs(1));
    let base = probe.snapshot().ok_or_else(|| {
        EngineError::Other("NVML не отдал память GPU — замер без него бессмыслен (R29)".into())
    })?;
    println!(
        "NVML {}: базовая занятость {} / {} МиБ, свободно {} МиБ",
        probe.name(),
        base.used_mib,
        base.total_mib,
        base.free_mib
    );

    let engine = Engine::open(args.engine_dir.as_deref())?;
    let _cwd = engine.activate()?; // cwd движка обязателен (грабли A1/A2)
    let cluster = engine.create_cluster()?;
    println!("движок: {}\n", engine.dir().display());

    let mut rows: Vec<Row> = Vec::new();
    for n_ctx in &args.n_ctx {
        println!("замер: n_ctx {n_ctx}, n_gpu_layers {}", args.ngl);
        let name = format!("kvprobe_{n_ctx}_{}", std::process::id());
        if let Some(row) = measure(
            &cluster,
            &probe,
            &name,
            &model,
            &meta,
            *n_ctx,
            args.ngl,
            base.used_mib,
            &args,
            file_mib,
        )? {
            println!(
                "  рост VRAM {:>6} МиБ (пик {:>6} МиБ), формула KV {:>6} МиБ на {} KV-слоях",
                row.delta_mib, row.peak_mib, row.kv_formula_mib, row.kv_layers
            );
            for n in &row.notes {
                println!("  ^ {n}");
            }
            rows.push(row);
        }
    }
    if rows.is_empty() {
        eprintln!("\nни один замер не выполнен — не хватает свободной VRAM (см. пропуски выше)");
        std::process::exit(1);
    }
    analyze(
        &meta,
        &rows,
        file_mib,
        &cfg,
        &base,
        json_path.as_deref(),
        &model,
    )
}

/// Дифференциальный анализ замеров + вывод для полного офлоада и `--json`.
#[allow(clippy::too_many_arguments)]
fn analyze(
    meta: &GgufMeta,
    rows: &[Row],
    file_mib: u64,
    cfg: &config::LlmHostConfig,
    base: &hds_llama::vram::VramSnapshot,
    json_path: Option<&Path>,
    model: &Path,
) -> Result<()> {
    // цена токена KV из разницы двух n_ctx при одном n_gpu_layers.
    // ВАЖНО: NVML здесь машинный (не по процессу — на Windows per-process VRAM нет),
    // поэтому при почти полной карте WDDM вытесняет чужие буферы, и разница может
    // «не вырасти» (наблюдено: 32768 против 4096 дали −32 МиБ). Порог шума — 64 МиБ;
    // ниже него KV берём из данных самого движка (он печатает
    // `llama_kv_cache: size = … MiB (… cells, N layers)` в свой лог).
    const NOISE_MIB: i64 = 64;
    let per_token_kib: Option<f64> = {
        let mut it = rows.iter().filter(|r| r.kv_layers > 0);
        match (it.next(), it.next()) {
            (Some(a), Some(b)) => {
                let (small, big) = if a.n_ctx < b.n_ctx { (a, b) } else { (b, a) };
                let d_ctx = (big.n_ctx - small.n_ctx) as f64;
                let d_mib = big.delta_mib - small.delta_mib;
                if d_mib.abs() < NOISE_MIB {
                    println!(
                        "\nдифференциальный замер: Δn_ctx {d_ctx:.0} → ΔVRAM {d_mib} МиБ — \
                         ниже шума NVML ({NOISE_MIB} МиБ), при почти полной карте WDDM вытесняет \
                         чужие буферы"
                    );
                    println!(
                        "  фактический KV берём из данных движка: строка `llama_kv_cache: size = …` \
                         в его логе (наш прогноз — formula_mib в отчёте)"
                    );
                    None
                } else {
                    let kib = d_mib as f64 * 1024.0 / d_ctx / big.kv_layers as f64;
                    println!(
                        "\nдифференциальный замер: Δn_ctx {d_ctx:.0} токенов, ΔVRAM {d_mib} МиБ \
                         на {} KV-слоях → {kib:.2} КиБ/токен/слой",
                        big.kv_layers
                    );
                    Some(kib)
                }
            }
            _ => {
                println!(
                    "\nдифференциальный замер невозможен: нужны две удачные загрузки с разным n_ctx"
                );
                None
            }
        }
    };
    let formula_kib = meta.head_count_kv as f64
        * (meta.key_length.unwrap_or(meta.head_dim()) as f64
            + meta.value_length.unwrap_or(meta.head_dim()) as f64)
        * KvBits::F16.bytes_per_element()
        / 1024.0;
    if let Some(kib) = per_token_kib {
        println!(
            "  формула llama.cpp даёт {formula_kib:.2} КиБ/токен/слой → расхождение {:+.1} %",
            (kib - formula_kib) / formula_kib * 100.0
        );
    }

    // вывод для полного офлоада и вердикт по ёмкости карты
    let n_ctx_max = rows.iter().map(|r| r.n_ctx).max().unwrap_or(32768);
    let kv_full_formula = kv_cache_mib(meta, n_ctx_max, 1, KvBits::F16);
    let kv_full_measured = per_token_kib
        .map(|k| k * meta.kv_layer_count() as f64 * n_ctx_max as f64 / 1024.0)
        .unwrap_or(kv_full_formula);
    let need_full = file_mib as f64 + kv_full_measured + (file_mib as f64 * 0.05);
    println!(
        "\nэквивалент полного офлоада при n_ctx {n_ctx_max}: KV ≈ {kv_full_measured:.0} МиБ \
         (формула {kv_full_formula:.0} МиБ), всего нужно ≈ {need_full:.0} МиБ при файле {file_mib} МиБ"
    );
    let fits = need_full + cfg.gpu.reserve_mb as f64 <= base.total_mib as f64;
    println!(
        "  вердикт для карты {} МиБ (резерв {} МиБ): {}",
        base.total_mib,
        cfg.gpu.reserve_mb,
        if fits {
            "влезает"
        } else {
            "НЕ влезает"
        }
    );
    println!(
        "  сверх модели+KV движок держит ещё два фиксированных (не зависящих от n_ctx) блока — \
         их видно в его логе: `llama_memory_recurrent: size = …` (SSM-состояние гибридных слоёв) \
         и `sched_reserve: CUDA0 compute buffer size = …` (зависит от n_batch/n_ubatch)"
    );

    if let Some(json_path) = json_path {
        let report = serde_json::json!({
            "model": model.display().to_string(),
            "file_mib": file_mib,
            "architecture": meta.architecture,
            "block_count": meta.block_count,
            "kv_layer_count": meta.kv_layer_count(),
            "full_attention_interval": meta.full_attention_interval,
            "head_count_kv": meta.head_count_kv,
            "key_length": meta.key_length.unwrap_or(meta.head_dim()),
            "value_length": meta.value_length.unwrap_or(meta.head_dim()),
            "baseline_used_mib": base.used_mib,
            "total_mib": base.total_mib,
            "rows": rows.iter().map(|r| serde_json::json!({
                "n_gpu_layers": r.n_gpu_layers,
                "n_ctx": r.n_ctx,
                "delta_mib": r.delta_mib,
                "peak_mib": r.peak_mib,
                "free_mib": r.free_mib,
                "kv_layers": r.kv_layers,
                "kv_formula_mib": r.kv_formula_mib,
                "notes": r.notes,
            })).collect::<Vec<_>>(),
            "kv_kib_per_token_per_layer_measured": per_token_kib,
            "kv_kib_per_token_per_layer_formula": formula_kib,
            "kv_full_mib_formula": kv_full_formula.round(),
            "kv_full_mib_measured": kv_full_measured.round(),
            "need_full_mib": need_full.round(),
            "n_ctx_max": n_ctx_max,
            "fits_total_card": fits,
        });
        if let Some(dir) = json_path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let text = serde_json::to_string_pretty(&report)
            .map_err(|e| EngineError::Other(format!("json: {e}")))?;
        std::fs::write(json_path, text)
            .map_err(|e| EngineError::Other(format!("{}: {e}", json_path.display())))?;
        println!("\nотчёт: {}", json_path.display());
    }
    Ok(())
}

/// Одна измеренная конфигурация (ngl × n_ctx).
#[derive(Debug, Clone)]
struct Row {
    n_gpu_layers: i32,
    n_ctx: i64,
    /// Рост занятой VRAM относительно базовой линии (веса + KV + буферы).
    delta_mib: i64,
    peak_mib: u64,
    free_mib: u64,
    /// Формульная оценка KV для этой конфигурации (МиБ).
    kv_formula_mib: u64,
    /// Сколько слоёв держат KV среди офлоаднутых.
    kv_layers: u32,
    /// Инференс по факту: движок ответил на пробный эмбеддинг/чат? (не используется)
    notes: Vec<String>,
}

/// Загрузить модель с заданными `n_ctx`/`n_gpu_layers` и замерить рост VRAM.
#[allow(clippy::too_many_arguments)]
fn measure(
    cluster: &Cluster,
    probe: &NvmlProbe,
    name: &str,
    model: &Path,
    meta: &GgufMeta,
    n_ctx: i64,
    n_gpu_layers: i32,
    baseline_used: u64,
    args: &Args,
    file_mib: u64,
) -> Result<Option<Row>> {
    // оценка «сколько попросит» — только чтобы отказаться заранее, если не влезет.
    // KV на GPU: только у офлоаднутых слоёв (у гибридных моделей их доля считается
    // по `full_attention_interval`); полная формула печатается отдельно для сравнения.
    let kv_formula = kv_cache_mib(meta, n_ctx, 1, KvBits::F16);
    let kv_layers_all = meta.kv_layer_count().max(1) as f64;
    let kv_layers_gpu = if n_gpu_layers < 0 {
        kv_layers_all
    } else {
        meta.kv_layer_count_within(n_gpu_layers.max(0) as u32) as f64
    };
    let kv_offloaded = kv_formula * kv_layers_gpu / kv_layers_all;
    let weights_share = if n_gpu_layers < 0 {
        file_mib as f64
    } else {
        file_mib as f64 * n_gpu_layers as f64 / meta.block_count.max(1) as f64
    };
    let need = (weights_share + kv_offloaded) as u64 + 512;
    let free = probe.snapshot().map(|v| v.free_mib).unwrap_or(0);
    if free < need + args.min_margin_mib {
        println!(
            "пропуск n_ctx {n_ctx} / ngl {n_gpu_layers}: нужно ≈{need} МиБ \
             (веса ≈{weights_share:.0} + KV на GPU ≈{kv_offloaded:.0} + буферы 512) \
             + запас {}, свободно {free} МиБ",
            args.min_margin_mib
        );
        return Ok(None);
    }

    // «призрак» с тем же именем от прошлого прогона — снять
    if let Ok(Some(old)) = cluster.find_instance_by_name(name) {
        let _ = cluster.unload(old);
        let _ = cluster.remove_instance(old);
    }
    let mut spec = InstanceSpec::new(name, &model.to_string_lossy());
    spec.model_kind = Some(model_kind::TEXT);
    spec.retention_mode = Some(retention::KEEP_LOADED);
    spec.n_ctx = Some(n_ctx as i32);
    if n_gpu_layers >= 0 {
        spec.n_gpu_layers = Some(n_gpu_layers);
        // частичный офлоад: без allow_cpu движок может отказать
        spec.allow_cpu = Some(true);
    }
    let id = cluster.create_instance(&spec)?;
    let sampler = hds_llama::vram::VramSampler::start(
        Arc::new(NvmlProbe::open(args.nvml_index)?),
        Duration::from_millis(200),
    );
    let load = cluster.load(id);
    if let Err(e) = load {
        let peak = sampler.stop().used_mib;
        println!("  загрузка не удалась (n_ctx {n_ctx}, ngl {n_gpu_layers}): {e} (пик {peak} МиБ)");
        let _ = cluster.remove_instance(id);
        return Ok(None);
    }
    let inst = cluster.wait_loaded(
        id,
        Duration::from_secs(args.load_timeout),
        Duration::from_millis(500),
    )?;
    std::thread::sleep(Duration::from_secs(args.settle_secs));
    let peak = sampler.stop().used_mib;
    let snap = probe.snapshot();
    let used = snap.map(|v| v.used_mib).unwrap_or(0);
    let free_now = snap.map(|v| v.free_mib).unwrap_or(0);
    let mut notes = vec![format!("состояние после загрузки: {}", inst.state_name)];
    if !inst.last_error.is_empty() {
        notes.push(format!("last_error: {}", inst.last_error));
    }

    // выгружаем и ждём возврата VRAM к базовой линии
    let _ = cluster.unload(id);
    let _ = cluster.remove_instance(id);
    let mut waited = 0;
    while waited < 30 {
        let now = probe.snapshot().map(|v| v.used_mib).unwrap_or(0);
        if now <= baseline_used + 64 {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
        waited += 1;
    }
    let restored = probe.snapshot().map(|v| v.used_mib).unwrap_or(0);
    if restored > baseline_used + 64 {
        notes.push(format!(
            "после выгрузки занято {restored} МиБ против базовой линии {baseline_used} МиБ"
        ));
    }
    Ok(Some(Row {
        n_gpu_layers,
        n_ctx,
        delta_mib: used as i64 - baseline_used as i64,
        peak_mib: peak,
        free_mib: free_now,
        kv_formula_mib: kv_formula.round() as u64,
        kv_layers: meta.kv_layer_count_within(if n_gpu_layers < 0 {
            meta.block_count
        } else {
            n_gpu_layers.max(0) as u32
        }),
        notes,
    }))
}
