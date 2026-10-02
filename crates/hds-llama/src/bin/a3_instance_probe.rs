//! A3: проверка кросс-процессной адресации инстансов (`PLAN_W2_LLM_HOST.md` §4 A3).
//!
//! Вопрос, который решает архитектуру A5: если инстанс создан в процессе A,
//! видит ли его процесс B (через свой `list_instances`/`find_instance_by_name`)
//! и можно ли по нему вызвать `embeddings`? План заранее предусматривает два
//! пути: (а) прямое обращение клиентов к чужим инстансам, (б) гарантированный
//! фасад `:8010–8012`. Этот пробик даёт ответ замером, а не предположением.
//!
//! Запуск (двумя процессами, из корня репозитория):
//! ```powershell
//! # процесс A: поднять инстанс и держать его
//! cargo run -p hds-llama --release --bin a3_instance_probe -- --hold 60
//! # процесс B (пока A держит): посмотреть и обратиться по имени
//! cargo run -p hds-llama --release --bin a3_instance_probe -- --list
//! cargo run -p hds-llama --release --bin a3_instance_probe -- --call a3_hold
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_llama::cluster::InstanceSpec;
use hds_llama::error::{EngineError, Result};
use hds_llama::ffi::{model_kind, retention};
use hds_llama::Engine;

struct Args {
    engine_dir: Option<PathBuf>,
    model: PathBuf,
    hold: Option<u64>,
    list: bool,
    call: Option<String>,
    name: String,
    texts: usize,
}

/// Абсолютный путь до переключения cwd на каталог движка (`Engine::activate`).
fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn default_embedding_model() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| EngineError::Other("неизвестен каталог данных пользователя".into()))?;
    Ok(base
        .data_local_dir()
        .join("llama-runtime")
        .join("models")
        .join("embedding")
        .join("bge-m3-Q8_0.gguf"))
}

fn parse_args() -> std::result::Result<Args, String> {
    let mut args = Args {
        engine_dir: None,
        model: default_embedding_model().unwrap_or_else(|_| PathBuf::from("bge-m3-Q8_0.gguf")),
        hold: None,
        list: false,
        call: None,
        name: "a3_hold".to_string(),
        texts: 2,
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
            "--name" => args.name = take("--name")?,
            "--hold" => {
                args.hold = Some(
                    take("--hold")?
                        .trim_end_matches('s')
                        .parse()
                        .map_err(|e| format!("--hold: {e}"))?,
                )
            }
            "--list" => args.list = true,
            "--call" => args.call = Some(take("--call")?),
            "--texts" => {
                args.texts = take("--texts")?
                    .parse()
                    .map_err(|e| format!("--texts: {e}"))?
            }
            "--help" | "-h" => {
                return Err(
                    "использование: a3_instance_probe (--hold SEC | --list | --call NAME) \
                            [--name NAME] [--model GGUF] [--engine-dir DIR]"
                        .to_string(),
                )
            }
            other => return Err(format!("неизвестный аргумент: {other}")),
        }
    }
    if args.hold.is_none() && !args.list && args.call.is_none() {
        return Err("нужен один из режимов: --hold SEC | --list | --call NAME".to_string());
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
    args.model = absolutize(&args.model);

    let engine = Engine::open(args.engine_dir.as_deref())?;
    println!(
        "[pid {}] движок: {}",
        std::process::id(),
        engine.dir().display()
    );
    let _cwd = engine.activate()?;
    let cluster = engine.create_cluster()?;
    let devices = cluster.devices()?;
    println!("[pid {}] устройств: {}", std::process::id(), devices.len());

    // --- режим --list: что видит ЭТОТ процесс ---
    if args.list {
        let instances = cluster.instances()?;
        println!(
            "[pid {}] list_instances: {}",
            std::process::id(),
            instances.len()
        );
        for i in &instances {
            println!(
                "  id={} name={} state={} kind={} model={}",
                i.id, i.name, i.state_name, i.model_kind, i.model_path
            );
        }
        // и что вернёт поиск по имени инстанса, созданного другим процессом
        match cluster.find_instance_by_name(&args.name) {
            Ok(Some(id)) => println!("  find_instance_by_name({}) -> id={id}", args.name),
            Ok(None) => println!(
                "  find_instance_by_name({}) -> НЕ НАЙДЕН в этом процессе",
                args.name
            ),
            Err(e) => println!("  find_instance_by_name({}) -> ошибка: {e}", args.name),
        }
        Ok(())
    } else if let Some(name) = args.call {
        // --- режим --call: попытка обратиться к инстансу «чужого» процесса ---
        match cluster.find_instance_by_name(&name)? {
            None => {
                println!(
                    "[pid {}] инстанс '{name}' НЕ НАЙДЕН в этом процессе → кросс-процессная \
                     адресация через cluster API недоступна",
                    std::process::id()
                );
                Ok(())
            }
            Some(id) => {
                let body = serde_json::json!({
                    "input": vec!["проверка кросс-процессного вызова"; args.texts],
                    "encoding_format": "float"
                })
                .to_string();
                let out = cluster.embeddings_json(id, &body, true)?;
                match out.ensure_ok() {
                    Ok(()) => {
                        println!(
                            "[pid {}] embeddings по чужому инстансу '{name}' (id={id}) — ok, \
                             {:.0} мс",
                            std::process::id(),
                            out.metrics.request_total_ms
                        );
                    }
                    Err(e) => {
                        println!("[pid {}] embeddings вернул ошибку: {e}", std::process::id())
                    }
                }
                Ok(())
            }
        }
    } else {
        // --- режим --hold: поднять инстанс и держать его указанное число секунд ---
        let hold = args.hold.unwrap_or(30);
        let name = args.name.clone();
        // «призрак» с тем же именем от прошлого прогона — снять
        if let Ok(Some(old)) = cluster.find_instance_by_name(&name) {
            let _ = cluster.unload(old);
            let _ = cluster.remove_instance(old);
        }
        let mut spec = InstanceSpec::new(&name, &args.model.to_string_lossy());
        spec.embedding = Some(true);
        spec.model_kind = Some(model_kind::EMBEDDINGS);
        spec.retention_mode = Some(retention::KEEP_LOADED);
        spec.load_on_demand_grace_seconds = Some(600);
        spec.n_ctx = Some(8192);
        let id = cluster.create_instance(&spec)?;
        cluster.load(id)?;
        let inst = cluster.wait_loaded(id, Duration::from_secs(180), Duration::from_millis(300))?;
        println!(
            "[pid {}] инстанс '{name}' id={id} создан и загружен (state={})",
            std::process::id(),
            inst.state_name
        );
        println!(
            "[pid {}] держу {hold} с — в другом окне запустите: a3_instance_probe --list",
            std::process::id()
        );
        std::thread::sleep(Duration::from_secs(hold));
        let _ = cluster.unload(id);
        let _ = cluster.remove_instance(id);
        println!("[pid {}] инстанс снят, выход", std::process::id());
        Ok(())
    }
}
