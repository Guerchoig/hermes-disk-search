//! Аудит метаданных GGUF: полный дамп KV-пар модели + сводка по KV-кэшу.
//!
//! Зачем: замер фактического KV движка (решение заказчика 30.09.2026) должен
//! опираться на факты из файла модели — в частности, **все ли слои держат KV**
//! (у гибридных моделей часть слоёв линейная/без KV) и какие `key_length`/
//! `value_length`. Дамп показывает и «подозрительные» ключи (`ssm`, `window`,
//! `rope`, `attention`), а сводка сразу считает KV на токен и на `n_ctx`.
//!
//! Запуск из корня репозитория:
//! `cargo run -p hds-llama --release --bin gguf_dump -- <модель.gguf>
//!  [--filter SUBSTR] [--json FILE]`
//! Без пути — берёт модель роли из конфига (`--role chat`) через общий рантайм:
//! `cargo run -p hds-llama --release --bin gguf_dump -- --role chat`

use std::path::{Path, PathBuf};

use hds_llama::budget::kv_cache_mib;
use hds_llama::config;
use hds_llama::error::{EngineError, Result};
use hds_llama::gguf::{kv_type_name, read_all, read_meta, KvBits};
use hds_llama::runtime::RuntimePaths;

struct Args {
    model: Option<PathBuf>,
    role: Option<String>,
    config: PathBuf,
    runtime: Option<PathBuf>,
    filter: Option<String>,
    json: Option<PathBuf>,
    n_ctx: i64,
    n_parallel: i64,
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
        model: None,
        role: None,
        config: std::env::var("HDS_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| repo_root().join("config.yaml")),
        runtime: None,
        filter: None,
        json: None,
        n_ctx: 32768,
        n_parallel: 1,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut take = |name: &str| -> std::result::Result<String, String> {
            it.next()
                .ok_or_else(|| format!("после {name} ожидалось значение"))
        };
        match a.as_str() {
            "--role" => args.role = Some(take("--role")?),
            "--config" => args.config = PathBuf::from(take("--config")?),
            "--runtime" => args.runtime = Some(PathBuf::from(take("--runtime")?)),
            "--filter" => args.filter = Some(take("--filter")?),
            "--json" => args.json = Some(PathBuf::from(take("--json")?)),
            "--n-ctx" => {
                args.n_ctx = take("--n-ctx")?
                    .parse()
                    .map_err(|e| format!("--n-ctx: {e}"))?
            }
            "--n-parallel" => {
                args.n_parallel = take("--n-parallel")?
                    .parse()
                    .map_err(|e| format!("--n-parallel: {e}"))?
            }
            "--help" | "-h" => {
                return Err("использование: gguf_dump <модель.gguf> | --role chat \
                            [--config FILE] [--runtime DIR] [--filter SUBSTR] [--json FILE] \
                            [--n-ctx N] [--n-parallel N]"
                    .to_string())
            }
            other if other.starts_with("--") => {
                return Err(format!("неизвестный аргумент: {other}"))
            }
            other => args.model = Some(PathBuf::from(other)),
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

/// Ключи, которые говорят о структуре внимания (и о возможной «гибридности»).
fn is_kv_relevant(name: &str) -> bool {
    [
        "attention",
        "block_count",
        "head",
        "key_length",
        "value_length",
        "embedding_length",
        "sliding_window",
        "rope",
        "ssm",
        "linear",
        "recurrent",
        "context_length",
        "expert",
    ]
    .iter()
    .any(|p| name.contains(p))
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

    // путь к модели: явный или роль из конфига через общий рантайм
    let model = match (&args.model, &args.role) {
        (Some(m), _) => m.clone(),
        (None, Some(role)) => {
            let cfg = config::load(&args.config)?;
            let rc = cfg
                .role(role)
                .ok_or_else(|| EngineError::Other(format!("роль '{role}' не описана в конфиге")))?;
            let paths = match &args.runtime {
                Some(r) => RuntimePaths::new(r.clone()),
                None => RuntimePaths::from_env()?,
            };
            hds_llama::runtime::resolve_model_checked(&paths.root, &rc.model_spec, role)?
        }
        (None, None) => {
            eprintln!("укажите файл GGUF или --role <chat|embedding|rerank>");
            std::process::exit(2);
        }
    };
    let size_mib = std::fs::metadata(&model)
        .map(|m| m.len() >> 20)
        .unwrap_or(0);
    println!("модель: {} ({} МиБ)", model.display(), size_mib);

    let all = read_all(&model)?;
    let meta = read_meta(&model)?;
    println!(
        "архитектура: {}, слоёв {}, голов {} (KV {}), embedding {}, head_dim {}, causal {:?}",
        meta.architecture,
        meta.block_count,
        meta.head_count,
        meta.head_count_kv,
        meta.embedding_length,
        meta.head_dim(),
        meta.causal
    );

    // --- ключи о структуре внимания: где искать доказательства вместо догадок ---
    println!("\nключи о структуре внимания:");
    for kv in all.iter().filter(|kv| is_kv_relevant(&kv.name)) {
        println!(
            "  {:<52} {:<8} {}",
            kv.name,
            kv_type_name(kv.type_id),
            kv.value.describe()
        );
    }

    summarize(&meta, &all, &model, size_mib, &args, json_path.as_deref())?;
    print_all(&all, args.filter.as_deref());
    Ok(())
}

/// Сводка «KV на токен и на n_ctx» + запись `--json` (вынесена, чтобы не мешать дампу).
fn summarize(
    meta: &hds_llama::gguf::GgufMeta,
    all: &[hds_llama::gguf::GgufKv],
    model: &Path,
    size_mib: u64,
    args: &Args,
    json_path: Option<&Path>,
) -> Result<()> {
    let bits = KvBits::F16;
    let per_token_kib = meta.block_count as f64
        * meta.head_count_kv as f64
        * (meta.key_length.unwrap_or(meta.head_dim()) as f64
            + meta.value_length.unwrap_or(meta.head_dim()) as f64)
        * bits.bytes_per_element()
        / 1024.0;
    let kv_ctx = kv_cache_mib(meta, args.n_ctx, args.n_parallel, bits);
    let kv_16k = kv_cache_mib(meta, 16384, args.n_parallel, bits);
    let kv_q8 = kv_cache_mib(meta, args.n_ctx, args.n_parallel, KvBits::Q8_0);
    let has_hybrid = all.iter().any(|kv| {
        kv.name.contains("ssm") || kv.name.contains("linear") || kv.name.contains("recurrent")
    });
    println!("\nсводка KV (формула по метаданным, f16 = 2 байта/элемент):");
    println!(
        "  слоёв {} × голов KV {} × (key {} + value {}) × 2 Б = {per_token_kib:.1} КиБ/токен",
        meta.block_count,
        meta.head_count_kv,
        meta.key_length.unwrap_or(meta.head_dim()),
        meta.value_length.unwrap_or(meta.head_dim())
    );
    println!(
        "  n_ctx {} × n_parallel {}: f16 {kv_ctx:.0} МиБ, q8_0 {kv_q8:.0} МиБ; n_ctx 16384: f16 {kv_16k:.0} МиБ",
        args.n_ctx, args.n_parallel
    );
    println!(
        "  гибридные признаки (ssm/linear/recurrent): {}",
        if has_hybrid {
            "ЕСТЬ — часть слоёв может не держать KV, формула завышает"
        } else {
            "нет — считаем, что KV держат все слои"
        }
    );

    if let Some(json_path) = json_path {
        let report = serde_json::json!({
            "model": model.display().to_string(),
            "file_mib": size_mib,
            "architecture": meta.architecture,
            "block_count": meta.block_count,
            "head_count": meta.head_count,
            "head_count_kv": meta.head_count_kv,
            "embedding_length": meta.embedding_length,
            "head_dim": meta.head_dim(),
            "key_length": meta.key_length,
            "value_length": meta.value_length,
            "causal": meta.causal,
            "has_kv_cache": meta.has_kv_cache(),
            "kv_kib_per_token_f16": per_token_kib,
            "n_ctx": args.n_ctx,
            "n_parallel": args.n_parallel,
            "kv_f16_mib": kv_ctx.round(),
            "kv_q8_0_mib": kv_q8.round(),
            "kv_f16_mib_at_16384": kv_16k.round(),
            "hybrid_markers": has_hybrid,
            "metadata": all.iter().map(|kv| {
                serde_json::json!({ "name": kv.name, "type": kv_type_name(kv.type_id),
                                    "value": kv.value.describe() })
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
    }
    Ok(())
}

/// Полный дамп KV-пар (по фильтру, если задан).
fn print_all(all: &[hds_llama::gguf::GgufKv], filter: Option<&str>) {
    println!("\nвсе KV-пары ({}):", all.len());
    for kv in all {
        if let Some(f) = filter {
            if !kv.name.contains(f) {
                continue;
            }
        }
        println!(
            "  {:<52} {:<8} {}",
            kv.name,
            kv_type_name(kv.type_id),
            kv.value.describe()
        );
    }
}
