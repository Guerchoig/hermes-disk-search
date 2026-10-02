//! A5 (шаг 0): разведка контракта чата у cluster API — **замером**, а не по догадкам.
//!
//! Два вопроса, от которых зависит фасад `:8010–8012`:
//! 1. **`prompt` в `chat_complete` — это готовый шаблонный текст или «сырое» продолжение?**
//!    Если движок сам применяет chat-шаблон модели, фасад может передавать как есть;
//!    если нет — фасад обязан рендерить шаблон сам (иначе ответы будут мусором).
//! 2. **`reasoning = off` через cluster-инстанс** (в W0 проверялось только bridge-API,
//!    план §9.5 помечал это как открытый вопрос): приходит ли ответ без блоков
//!    размышлений (`thinking`), а для видимых размышлений достаточно ли
//!    `reasoning = on, format = none`.
//!
//! По умолчанию инстанс поднимается **на CPU** (`--ngl 0`), чтобы замер не отбирал
//! VRAM у штатных ролей Python-версии.
//!
//! Запуск из корня репозитория:
//! `cargo run -p hds-llama --release --bin chat_probe -- [--role chat]
//!  [--ngl 0] [--n-ctx 4096] [--json FILE]`

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_llama::cluster::InstanceSpec;
use hds_llama::config;
use hds_llama::error::{EngineError, Result};
use hds_llama::ffi::{model_kind, retention};
use hds_llama::runtime::RuntimePaths;
use hds_llama::Engine;

struct Args {
    config: PathBuf,
    runtime: Option<PathBuf>,
    engine_dir: Option<PathBuf>,
    role: String,
    ngl: i32,
    n_ctx: i64,
    n_predict: i32,
    json: Option<PathBuf>,
    load_timeout: u64,
    /// Одиночный произвольный prompt (режим разведки «применяет ли движок шаблон сам»).
    prompt: Option<String>,
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
        ngl: 0,
        n_ctx: 4096,
        n_predict: 48,
        json: None,
        load_timeout: 300,
        prompt: None,
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
            "--ngl" => args.ngl = take("--ngl")?.parse().map_err(|e| format!("--ngl: {e}"))?,
            "--n-ctx" => {
                args.n_ctx = take("--n-ctx")?
                    .parse()
                    .map_err(|e| format!("--n-ctx: {e}"))?
            }
            "--n-predict" => {
                args.n_predict = take("--n-predict")?
                    .parse()
                    .map_err(|e| format!("--n-predict: {e}"))?
            }
            "--load-timeout" => {
                args.load_timeout = take("--load-timeout")?
                    .parse()
                    .map_err(|e| format!("--load-timeout: {e}"))?
            }
            "--prompt" => args.prompt = Some(take("--prompt")?),
            "--help" | "-h" => {
                return Err(
                    "использование: chat_probe [--role chat] [--ngl 0] [--n-ctx 4096] \
                            [--n-predict 48] [--config FILE] [--runtime DIR] [--engine-dir DIR] \
                            [--json FILE]"
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

/// «Плоский» prompt без спецмаркеров: сначала выясняем, не применяет ли движок
/// шаблон чата сам (если да — фасаду достаточно «плоского» текста).
fn render_qwen(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

/// Вариант prompt со спецмаркерами ChatML (угловые скобки собираем из кодов —
/// литеральные последовательности ломаются инструментами, а движку нужны именно они).
fn render_qwen_chatml(system: &str, user: &str) -> String {
    let lt = '\u{3c}';
    let gt = '\u{3e}';
    let start = format!("{lt}|im_start|");
    let end = format!("{lt}|im_end|{gt}");
    format!("{start}system\n{system}{end}\n{start}user\n{user}{end}\n{start}assistant\n")
}

struct Row {
    name: String,
    prompt_kind: String,
    reasoning: String,
    text: String,
    ms: f64,
    decoded: i32,
    /// Токенов во входе: если движок применяет шаблон чата сам, их будет больше
    /// «сырого» текста (в этом и был вопрос разведки).
    prompt_tokens: i32,
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

    let engine = Engine::open(args.engine_dir.as_deref())?;
    let _cwd = engine.activate()?; // cwd движка обязателен (грабли A1/A2)
    let cluster = engine.create_cluster()?;
    println!("движок: {}", engine.dir().display());
    println!(
        "роль '{}' модель {} | n_ctx {} | n_gpu_layers {} (по умолчанию 0 = CPU, чтобы не есть VRAM)",
        args.role,
        model.display(),
        args.n_ctx,
        args.ngl
    );

    let name = format!("chatprobe_{}", std::process::id());
    if let Ok(Some(old)) = cluster.find_instance_by_name(&name) {
        let _ = cluster.unload(old);
        let _ = cluster.remove_instance(old);
    }
    let mut spec = InstanceSpec::new(&name, &model.to_string_lossy());
    spec.model_kind = Some(model_kind::TEXT);
    spec.retention_mode = Some(retention::KEEP_LOADED);
    spec.n_ctx = Some(args.n_ctx as i32);
    spec.n_gpu_layers = Some(args.ngl);
    spec.allow_cpu = Some(true);
    let id = cluster.create_instance(&spec)?;
    cluster.load(id)?;
    let inst = cluster.wait_loaded(
        id,
        Duration::from_secs(args.load_timeout),
        Duration::from_millis(500),
    )?;
    println!(
        "инстанс '{name}' id={id} загружен (state={})\n",
        inst.state_name
    );
    if let Some(prompt) = &args.prompt {
        // одиночный произвольный prompt: печатаем и текст, и число токенов промпта
        // (по нему видно, добавляет ли движок шаблон чата сам)
        let out = cluster.chat_complete(id, prompt, args.n_predict, 0.2, Some(("off", 0, None)))?;
        println!(
            "--- произвольный prompt ({} символов, reasoning=off)",
            prompt.chars().count()
        );
        println!("вход:  {:?}", prompt);
        println!("ответ: {}", indent(out.text.trim()));
        println!(
            "токенов промпта: {} (по нему видно, добавляет ли движок шаблон)",
            out.metrics.prompt_tokens
        );
    } else {
        probe_cases(&cluster, id, &args, &model, json_path.as_deref())?;
    }
    let _ = cluster.unload(id);
    let _ = cluster.remove_instance(id);
    Ok(())
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Прогон контрольных случаев и выводы по разведке контракта чата.
fn probe_cases(
    cluster: &hds_llama::Cluster,
    id: i64,
    args: &Args,
    model: &Path,
    json_path: Option<&Path>,
) -> Result<()> {
    let system = "Ты — ассистент поиска по файлам. Отвечай кратко.";
    let user = "Сколько будет 2+2? Ответь одним числом.";
    let flat = render_qwen(system, user);
    let templated = render_qwen_chatml(system, user);

    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, String, &str, Option<(&str, i32, Option<&str>)>)> = vec![
        ("flat", flat, "off", Some(("off", 0, None))),
        ("chatml", templated, "off", Some(("off", 0, None))),
        (
            "chatml",
            render_qwen_chatml(system, user),
            "on+none",
            Some(("on", -1, Some("none"))),
        ),
        ("chatml", render_qwen_chatml(system, user), "не задан", None),
    ];

    let mut rows = Vec::new();
    for (kind, prompt, reasoning_label, reasoning) in cases {
        let out = cluster.chat_complete(id, &prompt, args.n_predict, 0.2, reasoning)?;
        let text = out.text.trim().to_string();
        println!(
            "--- prompt={kind}, reasoning={reasoning_label}: ok={} токенов входа={} decoded={} {:.0} мс",
            out.ok, out.metrics.prompt_tokens, out.metrics.decoded_tokens, out.metrics.request_total_ms
        );
        println!("{}", indent(&text));
        if !out.error.is_empty() {
            println!("  ошибка движка: {}", out.error);
        }
        rows.push(Row {
            name: format!("{kind}/reasoning={reasoning_label}"),
            prompt_kind: kind.to_string(),
            reasoning: reasoning_label.to_string(),
            text,
            ms: out.metrics.request_total_ms,
            decoded: out.metrics.decoded_tokens,
            prompt_tokens: out.metrics.prompt_tokens,
        });
    }

    let short =
        |s: Option<&str>| -> String { s.unwrap_or("—").chars().take(90).collect::<String>() };
    let flat_answer = rows
        .iter()
        .find(|r| r.prompt_kind == "flat")
        .map(|r| r.text.clone());
    let tmpl_answer = rows
        .iter()
        .find(|r| r.prompt_kind == "chatml" && r.reasoning == "off")
        .map(|r| r.text.clone());
    let reasoning_off_has_thinking = tmpl_answer
        .as_deref()
        .map(|t| t.contains("think"))
        .unwrap_or(false);
    let templated_beats_flat = tmpl_answer
        .as_deref()
        .map(|t| t.contains('4'))
        .unwrap_or(false);

    println!("\nвыводы по контракту чата (для фасада A5):");
    println!("  flat-prompt   → {}", short(flat_answer.as_deref()));
    println!("  chatml-prompt → {}", short(tmpl_answer.as_deref()));
    println!(
        "  chatml содержит правильный ответ '4': {}",
        if templated_beats_flat {
            "да"
        } else {
            "нет"
        }
    );
    println!(
        "  reasoning=off через кластер: {} (блок размышлений {})",
        if reasoning_off_has_thinking {
            "НЕ ЧИСТО"
        } else {
            "ок"
        },
        if reasoning_off_has_thinking {
            "ЕСТЬ"
        } else {
            "отсутствует"
        }
    );

    if let Some(json_path) = json_path {
        let report = serde_json::json!({
            "model": model.display().to_string(),
            "role": args.role,
            "n_ctx": args.n_ctx,
            "n_gpu_layers": args.ngl,
            "cases": rows.iter().map(|r| serde_json::json!({
                "case": r.name, "prompt_kind": r.prompt_kind, "reasoning": r.reasoning,
                "text": r.text, "prompt_tokens": r.prompt_tokens,
                "decoded_tokens": r.decoded, "total_ms": r.ms,
            })).collect::<Vec<_>>(),
            "flat_prompt_answer": flat_answer,
            "chatml_prompt_answer": tmpl_answer,
            "chatml_has_answer": templated_beats_flat,
            "reasoning_off_has_thinking": reasoning_off_has_thinking,
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
