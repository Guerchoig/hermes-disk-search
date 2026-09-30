//! A1: первый тест выбора устройства (`PLAN_W2_LLM_HOST.md` §4 A1, §9 «осталось
//! выяснить» п.1; факт W0 — `tools/parity/SPIKES.md` §14.3).
//!
//! Один и тот же embedding-инстанс создаётся несколькими способами, для каждого
//! замеряются NVML (источник истины, R29) и скорость инференса:
//!
//! 1. `none` — устройство не задано (ожидание: CPU-only на Windows, R32);
//! 2. `csv_bridge_index` — `manual_devices_csv = "<bridge_device_index CUDA0>"`;
//! 3. `csv_bridge_index_plus1` — воспроизведение провала W0 (`--gpu 1` не помог);
//! 4. `csv_device_name` — `manual_devices_csv = "CUDA0"` (имя вместо индекса);
//! 5. `csv_bridge_index_strict` — как 2, но `allow_cpu = 0`: если устройство не
//!    принято, загрузка обязана упасть (проверка, что мы не «свалились» на CPU).
//!
//! Результат — `tools/parity/out/w2_a1_device.json` + таблица в stdout.
//!
//! Запуск (из корня репозитория):
//! `cargo run -p hds-llama --release --bin a1_device_probe -- [--engine-dir DIR]
//!  [--model GGUF] [--texts 8] [--n-ctx 8192] [--json FILE] [--variants a,b]`

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use hds_llama::cluster::{Cluster, Device, InstanceSpec, Metrics};
use hds_llama::device::accelerators;
use hds_llama::error::{EngineError, Result};
use hds_llama::ffi::{model_kind, retention};
use hds_llama::vram::{NvmlProbe, VramProbe, VramSampler, VramSnapshot};
use hds_llama::Engine;

/// Параметры запуска (ручной разбор — без лишних зависимостей в первом шаге).
struct Args {
    engine_dir: Option<PathBuf>,
    model: PathBuf,
    texts: usize,
    text_chars: usize,
    n_ctx: i32,
    wait_secs: u64,
    json: PathBuf,
    variants: Vec<String>,
    nvml_index: u32,
    /// `--no-cwd-fix`: не переключать текущий каталог на каталог движка
    /// (воспроизведение R32/пустого `list_devices` для отчёта).
    no_cwd_fix: bool,
}

const ALL_VARIANTS: &[&str] = &[
    "none",
    "csv_bridge_index",
    "csv_bridge_index_plus1",
    "csv_device_name",
    "csv_bridge_index_strict",
];

fn usage() -> String {
    format!(
        "использование: a1_device_probe [--engine-dir DIR] [--model GGUF] [--texts N] \
         [--text-chars N] [--n-ctx N] [--wait-sec N] [--json FILE] [--nvml-index N] \
         [--variants {}]",
        ALL_VARIANTS.join(",")
    )
}

/// Модель по умолчанию: bge-m3 из общего рантайма (`%LOCALAPPDATA%\llama-runtime`).
fn default_embedding_model() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    let p = base
        .data_local_dir()
        .join("llama-runtime")
        .join("models")
        .join("embedding")
        .join("bge-m3-Q8_0.gguf");
    Some(p)
}

/// Корень репозитория (крейт лежит в `crates/hds-llama`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// Абсолютный путь **до** переключения текущего каталога на каталог движка
/// (`Engine::activate()` меняет cwd — относительные пути после этого ведут внутрь
/// каталога движка; найдено при первом прогоне A2).
fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut args = Args {
        engine_dir: None,
        model: default_embedding_model().unwrap_or_else(|| PathBuf::from("bge-m3-Q8_0.gguf")),
        texts: 8,
        text_chars: 1200,
        n_ctx: 8192,
        wait_secs: 240,
        json: repo_root().join("tools/parity/out/w2_a1_device.json"),
        variants: ALL_VARIANTS.iter().map(|s| s.to_string()).collect(),
        nvml_index: 0,
        no_cwd_fix: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> std::result::Result<String, String> {
            it.next()
                .ok_or_else(|| format!("после {name} ожидалось значение"))
        };
        match a.as_str() {
            "--engine-dir" => args.engine_dir = Some(PathBuf::from(take("--engine-dir")?)),
            "--model" => args.model = PathBuf::from(take("--model")?),
            "--texts" => {
                args.texts = take("--texts")?
                    .parse()
                    .map_err(|e| format!("--texts: {e}"))?
            }
            "--text-chars" => {
                args.text_chars = take("--text-chars")?
                    .parse()
                    .map_err(|e| format!("--text-chars: {e}"))?
            }
            "--n-ctx" => {
                args.n_ctx = take("--n-ctx")?
                    .parse()
                    .map_err(|e| format!("--n-ctx: {e}"))?
            }
            "--wait-sec" => {
                args.wait_secs = take("--wait-sec")?
                    .parse()
                    .map_err(|e| format!("--wait-sec: {e}"))?
            }
            "--nvml-index" => {
                args.nvml_index = take("--nvml-index")?
                    .parse()
                    .map_err(|e| format!("--nvml-index: {e}"))?
            }
            "--json" => args.json = PathBuf::from(take("--json")?),
            "--no-cwd-fix" => args.no_cwd_fix = true,
            "--variants" => {
                args.variants = take("--variants")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("неизвестный аргумент: {other}\n{}", usage())),
        }
    }
    for v in &args.variants {
        if !ALL_VARIANTS.contains(&v.as_str()) {
            return Err(format!("неизвестный вариант '{v}'\n{}", usage()));
        }
    }
    Ok(args)
}

/// Итог одного способа выбора устройства.
#[derive(Debug, Clone, Serialize)]
struct VariantResult {
    label: String,
    manual_devices_csv: Option<String>,
    allow_cpu: Option<bool>,
    created: bool,
    instance_id: Option<i64>,
    error: String,
    state: String,
    last_error: String,
    nvml_base: Option<VramSnapshot>,
    nvml_peak: Option<VramSnapshot>,
    nvml_after_unload: Option<VramSnapshot>,
    vram_delta_mib: Option<i64>,
    embed_ok: bool,
    embed_error: String,
    metrics: Option<Metrics>,
    load_ms: u128,
    embed_ms: u128,
    /// `GPU` / `CPU` / `failed_create` / `failed_load` / `failed_infer`
    verdict: String,
}

/// Полный отчёт прогона (пишется в `tools/parity/out/w2_a1_device.json`).
#[derive(Debug, Clone, Serialize)]
struct Report {
    generated: String,
    host: String,
    engine_dir: String,
    engine_lib: String,
    model: String,
    model_mb: u64,
    n_ctx: i32,
    nvml_gpu: String,
    nvml_before: Option<VramSnapshot>,
    devices: Vec<Device>,
    /// `memory_free` движка против NVML — перепроверка факта R29 на этом прогоне.
    engine_memory_free_note: String,
    /// Что делали с текущим каталогом процесса (движок грузит ggml-бэкенды
    /// относительно `.` — находка A1).
    cwd_note: String,
    variants: Vec<VariantResult>,
    conclusion: String,
}

/// Способ адресации устройства для варианта: `(manual_devices_csv, allow_cpu)`.
fn variant_device(label: &str, gpu: &Device) -> (Option<String>, Option<bool>) {
    match label {
        "none" => (None, None),
        "csv_bridge_index" => (Some(gpu.bridge_device_index.to_string()), None),
        "csv_bridge_index_plus1" => (Some((gpu.bridge_device_index + 1).to_string()), None),
        "csv_device_name" => (Some(gpu.name.clone()), None),
        "csv_bridge_index_strict" => (Some(gpu.bridge_device_index.to_string()), Some(false)),
        _ => (None, None),
    }
}

/// Детерминированный набор текстов «как чанки»: длина важна для метрики токенов.
fn sample_texts(n: usize, chars: usize) -> Vec<String> {
    const BASE: &str = "Договор поставки оборудования: сроки, стоимость, ответственность сторон, \
порядок приёмки и гарантийные обязательства. Оплата производится этапами, приёмка — по акту. ";
    (0..n)
        .map(|i| {
            let mut s = format!("[раздел {}] ", i + 1);
            let mut len = s.chars().count();
            while len < chars {
                s.push_str(BASE);
                len = s.chars().count();
            }
            s
        })
        .collect()
}

/// Тело запроса эмбеддингов в формате `/v1/embeddings` (как у llama-server :8011).
fn embeddings_body(texts: &[String]) -> String {
    serde_json::json!({ "input": texts, "encoding_format": "float" }).to_string()
}

/// Отсутствие NVML не должно ломать прогон: отчёт пишется без цифр VRAM.
fn nvml_snapshot(probe: &Option<NvmlProbe>) -> Option<VramSnapshot> {
    probe.as_ref().and_then(|p| p.snapshot())
}

/// Свой NVML-инстанс для сэмплера (NVML допускает повторную инициализацию —
/// внутри у драйвера счётчик ссылок).
fn open_sampler(index: u32) -> Option<VramSampler> {
    match NvmlProbe::open(index) {
        Ok(p) => {
            let probe: Arc<dyn VramProbe> = Arc::new(p);
            Some(VramSampler::start(probe, Duration::from_millis(200)))
        }
        Err(_) => None,
    }
}

/// Параметры инстанса под вариант: embedding-модель, полный офлоад, KEEP_LOADED.
fn build_spec(
    label: &str,
    csv: Option<String>,
    allow_cpu: Option<bool>,
    args: &Args,
) -> InstanceSpec {
    let mut spec = InstanceSpec::new(
        &format!("a1_probe_{label}"),
        &args.model.to_string_lossy(),
    );
    spec.manual_devices_csv = csv;
    spec.allow_cpu = allow_cpu;
    spec.embedding = Some(true);
    spec.reranking = Some(false);
    spec.model_kind = Some(model_kind::EMBEDDINGS);
    // KEEP_LOADED: инстанс не выгрузится сам, пока мы не померим VRAM.
    spec.retention_mode = Some(retention::KEEP_LOADED);
    spec.load_on_demand_grace_seconds = Some(300);
    spec.n_ctx = Some(args.n_ctx);
    spec.n_gpu_layers = Some(-1); // дефолт движка: полный офлоад
    spec
}

/// Прогон одного варианта: создать → загрузить → эмбеддинги → выгрузить → удалить.
fn run_variant(
    cluster: &Cluster,
    label: &str,
    gpu: &Device,
    args: &Args,
    texts: &[String],
    nvml: &Option<NvmlProbe>,
) -> VariantResult {
    let (csv, allow_cpu) = variant_device(label, gpu);
    let spec = build_spec(label, csv.clone(), allow_cpu, args);
    let mut res = VariantResult {
        label: label.to_string(),
        manual_devices_csv: csv,
        allow_cpu,
        created: false,
        instance_id: None,
        error: String::new(),
        state: String::new(),
        last_error: String::new(),
        nvml_base: None,
        nvml_peak: None,
        nvml_after_unload: None,
        vram_delta_mib: None,
        embed_ok: false,
        embed_error: String::new(),
        metrics: None,
        load_ms: 0,
        embed_ms: 0,
        verdict: String::new(),
    };

    // «Призрак» с тем же именем (после падения прошлого прогона) — снять.
    if let Ok(Some(old)) = cluster.find_instance_by_name(&spec.name) {
        let _ = cluster.unload(old);
        let _ = cluster.remove_instance(old);
    }

    let id = match cluster.create_instance(&spec) {
        Ok(id) => {
            res.created = true;
            res.instance_id = Some(id);
            id
        }
        Err(e) => {
            res.error = format!("create: {e}");
            res.verdict = "failed_create".to_string();
            return res;
        }
    };

    let base = nvml_snapshot(nvml);
    res.nvml_base = base;
    let sampler = open_sampler(args.nvml_index);

    let mut failure: Option<(&'static str, String)> = None;

    let t0 = Instant::now();
    if let Err(e) = cluster.load(id) {
        failure = Some(("failed_load", format!("load: {e}")));
    }
    res.load_ms = t0.elapsed().as_millis();

    if failure.is_none() {
        match cluster.wait_loaded(
            id,
            Duration::from_secs(args.wait_secs),
            Duration::from_millis(300),
        ) {
            Ok(inst) => {
                res.state = inst.state_name.clone();
                if !inst.last_error.is_empty() {
                    res.last_error = inst.last_error.clone();
                }
                if !inst.is_loaded() {
                    failure = Some((
                        "failed_load",
                        format!("состояние {} ({})", inst.state_name, inst.last_error),
                    ));
                }
            }
            Err(e) => failure = Some(("failed_load", format!("wait: {e}"))),
        }
    }

    if failure.is_none() {
        let body = embeddings_body(texts);
        let t1 = Instant::now();
        match cluster.embeddings_json(id, &body, true) {
            Ok(out) => {
                res.metrics = Some(out.metrics);
                match out.ensure_ok() {
                    Ok(()) => res.embed_ok = true,
                    Err(e) => res.embed_error = e.to_string(),
                }
            }
            Err(e) => res.embed_error = e.to_string(),
        }
        res.embed_ms = t1.elapsed().as_millis();
        if !res.embed_ok {
            failure = Some(("failed_infer", res.embed_error.clone()));
        }
    }

    // Уборка: выгрузить (освободить VRAM), снять пик, удалить инстанс.
    let _ = cluster.unload(id);
    std::thread::sleep(Duration::from_millis(700)); // дать движку отпустить память
    res.nvml_after_unload = nvml_snapshot(nvml);
    res.nvml_peak = sampler.map(|s| s.stop());
    let _ = cluster.remove_instance(id);
    res.vram_delta_mib = match (res.nvml_peak, base) {
        (Some(p), Some(b)) => Some(p.used_delta(&b)),
        _ => None,
    };

    res.verdict = match failure {
        Some((verdict, msg)) => {
            if res.error.is_empty() {
                res.error = msg;
            }
            verdict.to_string()
        }
        None if res.vram_delta_mib.unwrap_or(0) >= 200 => "GPU".to_string(),
        None => "CPU".to_string(),
    };
    res
}

/// Строка отчёта по варианту (для журнала и глазами).
fn print_variant(r: &VariantResult) {
    let peak = r
        .nvml_peak
        .map(|p| format!("{} МиБ (+{})", p.used_mib, r.vram_delta_mib.unwrap_or(0)))
        .unwrap_or_else(|| "н/д".to_string());
    let tokens = r
        .metrics
        .map(|m| {
            if m.prompt_tokens > 0 {
                format!("{:.0} ток/с", m.prompt_tokens_per_second)
            } else {
                // у embeddings движок не заполняет счётчики токенов — показываем время запроса
                format!("req {:.0} мс", m.request_total_ms)
            }
        })
        .unwrap_or_else(|| "—".to_string());
    let tail = if r.error.is_empty() {
        String::new()
    } else {
        format!("\n    ошибка: {}", r.error)
    };
    println!(
        "{:<24} dev={:<12} allow_cpu={:<8} -> {:<13} state={:<8} peak={:<20} embed={:<6} {:<10} \
         load={} мс embed={} мс{}",
        r.label,
        r.manual_devices_csv.as_deref().unwrap_or("—"),
        r.allow_cpu
            .map(|v| v.to_string())
            .unwrap_or_else(|| "движок".into()),
        r.verdict,
        if r.state.is_empty() { "—" } else { &r.state },
        peak,
        r.embed_ok,
        tokens,
        r.load_ms,
        r.embed_ms,
        tail
    );
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
    // Относительные пути — сразу в абсолютные: после `Engine::activate()` текущий
    // каталог процесса — каталог движка (побочный эффект `EngineCwd`).
    args.model = absolutize(&args.model);
    args.json = absolutize(&args.json);
    if !args.model.is_file() {
        return Err(EngineError::Other(format!(
            "модель не найдена: {} (укажите --model; по умолчанию берётся \
             %LOCALAPPDATA%\\llama-runtime\\models\\embedding\\bge-m3-Q8_0.gguf)",
            args.model.display()
        )));
    }

    let engine = Engine::open(args.engine_dir.as_deref())?;
    println!("движок: {} ({})", engine.dir().display(), engine.lib_name());
    for note in engine.load_notes() {
        println!("  путь поиска DLL: {note}");
    }
    // Движок грузит ggml-бэкенды относительно ТЕКУЩЕГО каталога (находка A1):
    // без переключения list_devices пуст, а инференс уходит на CPU (R32).
    // Переключаем ДО первого обращения к кластеру — как это делает llm-host.
    let cwd_guard = if args.no_cwd_fix {
        None
    } else {
        Some(engine.activate()?)
    };
    let cwd_note = match &cwd_guard {
        Some(g) => format!(
            "текущий каталог переключён на каталог движка: {} (иначе ggml-бэкенды не грузятся)",
            g.dir().display()
        ),
        None => format!(
            "текущий каталог НЕ переключён (--no-cwd-fix), он равен {} — ожидаем пустой список устройств",
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "?".to_string())
        ),
    };
    println!("{cwd_note}");

    let cluster = engine.create_cluster()?;

    let devices = cluster.devices()?;
    for d in &devices {
        println!(
            "  device index={} backend={} name={} free={:.0} МиБ total={:.0} МиБ",
            d.bridge_device_index,
            d.backend,
            d.name,
            d.memory_free_mib(),
            d.memory_total as f64 / (1024.0 * 1024.0)
        );
    }
    let gpu = accelerators(&devices)
        .into_iter()
        .next()
        .ok_or_else(|| {
            EngineError::Other("движок не вернул ни одного устройства-ускорителя".to_string())
        })?
        .clone();

    let nvml = match NvmlProbe::open(args.nvml_index) {
        Ok(p) => {
            println!("NVML: {} (индекс {})", p.name(), p.index());
            Some(p)
        }
        Err(e) => {
            eprintln!("[warn] NVML недоступен ({e}) — VRAM померить нечем, вердикт A1 неполный");
            None
        }
    };
    let nvml_before = nvml_snapshot(&nvml);

    // R29: сверяем `memory_free` движка с NVML прямо в этом прогоне.
    let engine_memory_free_note = match (nvml_before, nvml.is_some()) {
        (Some(b), true) => format!(
            "list_devices: free={:.0} МиБ, NVML: free={} МиБ, расхождение {:+.0} МиБ (R29)",
            gpu.memory_free_mib(),
            b.free_mib,
            gpu.memory_free_mib() - b.free_mib as f64
        ),
        _ => format!(
            "list_devices: free={:.0} МиБ (NVML недоступен)",
            gpu.memory_free_mib()
        ),
    };
    println!("{engine_memory_free_note}");

    let texts = sample_texts(args.texts, args.text_chars);
    println!(
        "текстов: {} (≈{} символов каждый), модель: {}",
        texts.len(),
        args.text_chars,
        args.model.display()
    );

    let mut variants: Vec<VariantResult> = Vec::new();
    for label in &args.variants {
        println!("\n=== вариант {label} ===");
        let r = run_variant(&cluster, label, &gpu, &args, &texts, &nvml);
        print_variant(&r);
        variants.push(r);
    }

    // Вывод: какой способ реально даёт GPU и как это ложится на gpu.device_index.
    let winner = variants
        .iter()
        .find(|v| v.verdict == "GPU" && v.label != "none")
        .or_else(|| variants.iter().find(|v| v.verdict == "GPU"));
    let conclusion = match winner {
        Some(w) => {
            let idx = hds_llama::device::config_index_for(&devices, gpu.bridge_device_index);
            format!(
                "рабочий вариант: '{}' ({}); gpu.device_index = {} (0 = CPU, 1 = первый GPU) \
                 -> manual_devices_csv = {:?}; рост VRAM {:?} МиБ",
                w.label,
                w.manual_devices_csv
                    .as_deref()
                    .unwrap_or("устройство не задано"),
                idx.map(|i| i.to_string())
                    .unwrap_or_else(|| "н/д".to_string()),
                w.manual_devices_csv,
                w.vram_delta_mib
            )
        }
        None => "ни один вариант не дал роста VRAM: устройство через cluster API не \
                 назначилось — разбирать manual_devices_csv/allow_cpu и (при неудаче) \
                 bridge-API с явным gpu/pooling_type (§11.5 п.1)"
            .to_string(),
    };
    println!("\n{conclusion}");

    let model_mb = std::fs::metadata(&args.model)
        .map(|m| m.len() / (1024 * 1024))
        .unwrap_or(0);
    let report = Report {
        generated: timestamp(),
        host: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".to_string()),
        engine_dir: engine.dir().display().to_string(),
        engine_lib: engine.lib_name().to_string(),
        model: args.model.display().to_string(),
        model_mb,
        n_ctx: args.n_ctx,
        nvml_gpu: nvml
            .as_ref()
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| "NVML недоступен".to_string()),
        nvml_before,
        devices,
        engine_memory_free_note,
        cwd_note,
        variants,
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
    Ok(())
}

/// Метка времени в отчёте (без внешних крейтов).
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix={now}")
}




