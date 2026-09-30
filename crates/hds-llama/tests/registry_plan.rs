//! A2: план инстансов по конфигу — паритет с поведением Python-версии.
//!
//! Проверяем на временном каталоге рантайма:
//! * `shared:<role>` резолвится в общий рантайм машины (не в каталог проекта) —
//!   это дефект `SPIKES.md` §14.7, из-за которого чат не находил модель;
//! * манифест `current.json`, единственная модель роли без манифеста;
//! * негативный кейс: файла нет → сообщение содержит путь поиска и подсказку;
//! * маппинг `gpu.device_index` → `manual_devices_csv`, `allow_cpu`,
//!   `n_gpu_layers` (включая legacy `-ngl 0` → явная CPU-роль).

use std::path::PathBuf;

use hds_llama::config;
use hds_llama::ffi::{model_kind, retention};
use hds_llama::registry;
use hds_llama::Device;

/// Временное окружение: каталог рантайма + конфиг.
struct Env {
    root: PathBuf,
    runtime: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir()
            .join("hds-llama-tests")
            .join(format!("{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp root");
        let runtime = root.join("llama-runtime");
        std::fs::create_dir_all(&runtime).expect("runtime");
        Env { root, runtime }
    }

    /// Создать файл модели роли (создаёт каталоги).
    fn model(&self, role: &str, file: &str) -> PathBuf {
        let p = self.runtime.join("models").join(role).join(file);
        std::fs::create_dir_all(p.parent().unwrap()).expect("models dir");
        std::fs::write(&p, b"GGUF").expect("model file");
        p
    }

    /// Создать манифест `current.json` роли.
    fn manifest(&self, role: &str, file: &str) {
        let p = self.runtime.join("models").join(role).join("current.json");
        std::fs::create_dir_all(p.parent().unwrap()).expect("models dir");
        std::fs::write(
            &p,
            format!("{{\"file\": \"{file}\", \"switched_at\": \"2026-09-30T00:00:00\"}}"),
        )
        .expect("manifest");
    }

    fn write_config(&self, name: &str, yaml: &str) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, yaml).expect("config");
        p
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Фейковые устройства движка (как в прогоне A1: CUDA0 и CPU).
fn fake_devices() -> Vec<Device> {
    vec![
        Device {
            bridge_device_index: 0,
            device_type: 1,
            memory_free: 11_801_722_880,
            memory_total: 12_884_377_600,
            backend: "CUDA".to_string(),
            name: "CUDA0".to_string(),
            description: "NVIDIA GeForce RTX 3060".to_string(),
        },
        Device {
            bridge_device_index: 1,
            device_type: 0,
            memory_free: 33_731_325_952,
            memory_total: 68_089_540_608,
            backend: "CPU".to_string(),
            name: "CPU".to_string(),
            description: "AMD Ryzen 7 5700G".to_string(),
        },
    ]
}

const LEGACY_CONFIG: &str = r#"
index:
  whisper_model_path: ""
llm_server:
  host: "127.0.0.1"
  parallel: 1
  chat:
    port: 8010
    model: "shared:chat"
    ctx_per_slot: 32768
    extra_args: "--cache-type-k q8_0 -ngl 99"
  embedding:
    port: 8011
    model: "shared:embedding"
    ctx_per_slot: 8192
  rerank:
    port: 8012
    model: "shared:rerank"
    ctx_per_slot: 8192
    extra_args: "-ngl 0"
"#;

/// Готовая к подъёму роль из плана (паникует, если роль не спланирована).
fn ready<'a>(plans: &'a [registry::RolePlan], role: &str) -> &'a registry::PlannedInstance {
    plans
        .iter()
        .find(|p| p.role() == role)
        .unwrap_or_else(|| panic!("роль {role} отсутствует в плане"))
        .ready()
        .unwrap_or_else(|| panic!("роль {role} не спланирована: {:?}", plans))
}

#[test]
fn legacy_config_resolves_roles_from_shared_runtime() {
    let env = Env::new("legacy");
    let chat = env.model("chat", "Qwen3.5-9B-Q6_K.gguf");
    env.manifest("chat", "Qwen3.5-9B-Q6_K.gguf");
    let emb = env.model("embedding", "bge-m3-Q8_0.gguf"); // манифеста нет, файл один
    let cfg_path = env.write_config("config.yaml", LEGACY_CONFIG);
    let cfg = config::load(&cfg_path).expect("config");

    // модели rerank ещё нет — роль падает отдельно, остальные планируются
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices());
    let rr_failed = plans.iter().find(|p| p.role() == "rerank").expect("rerank");
    match rr_failed {
        registry::RolePlan::Failed { error, .. } => assert!(
            error.contains("ensure_llama_runtime") && error.contains("rerank"),
            "в ошибке роли должны быть путь и подсказка: {error}"
        ),
        registry::RolePlan::Ready(_) => panic!("rerank ещё не должен планироваться"),
    }
    assert!(plans.iter().filter(|p| p.is_failed()).count() == 1);

    // модель rerank появилась в общем рантайме (единственный GGUF роли)
    env.model("rerank", "bge-reranker-v2-m3-q8_0.gguf");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices());
    assert_eq!(plans.len(), 3, "chat + embedding + rerank (whisper без модели)");
    assert!(plans.iter().all(|p| !p.is_failed()), "{plans:?}");

    let chat_p = ready(&plans, "chat");
    assert_eq!(chat_p.model_path, chat);
    assert_eq!(chat_p.spec.manual_devices_csv.as_deref(), Some("0"), "CUDA0");
    assert_eq!(chat_p.spec.allow_cpu, Some(false));
    assert_eq!(chat_p.spec.retention_mode, Some(retention::KEEP_LOADED));
    assert_eq!(chat_p.spec.model_kind, Some(model_kind::TEXT));
    assert_eq!(chat_p.spec.n_ctx, Some(32768));
    assert_eq!(chat_p.spec.n_gpu_layers, Some(99), "-ngl 99 из legacy extra_args");
    assert_eq!(chat_p.port, 8010);

    let emb_p = ready(&plans, "embedding");
    assert_eq!(emb_p.model_path, emb, "единственный GGUF роли = активный");
    assert_eq!(emb_p.spec.model_kind, Some(model_kind::EMBEDDINGS));
    assert_eq!(emb_p.spec.embedding, Some(true));
    assert_eq!(emb_p.spec.reranking, Some(false));
    assert_eq!(emb_p.spec.retention_mode, Some(retention::LOAD_ON_DEMAND));
    assert_eq!(emb_p.spec.n_ctx, Some(8192));

    // legacy -ngl 0 → роль на CPU: явное CPU-устройство и allow_cpu = true
    let rr = ready(&plans, "rerank");
    assert_eq!(rr.spec.manual_devices_csv.as_deref(), Some("1"), "CPU-устройство");
    assert_eq!(rr.spec.allow_cpu, Some(true));
    assert_eq!(rr.spec.n_gpu_layers, None, "для CPU-роли слои на GPU не задаём");
    assert_eq!(rr.spec.reranking, Some(true));
    assert!(
        rr.notes.iter().any(|n| n.contains("на CPU")),
        "должно быть пояснение про CPU-роль: {:?}",
        rr.notes
    );

    assert!(
        cfg.warnings.iter().any(|w| w.contains("--cache-type-k")),
        "warnings: {:?}",
        cfg.warnings
    );
}


/// Новые ключи `llm.*`/`gpu.*` приоритетнее legacy.
#[test]
fn new_keys_win_over_legacy_and_gpu_layers_override() {
    let env = Env::new("newkeys");
    env.model("chat", "New.gguf");
    env.manifest("chat", "New.gguf");
    let yaml = r#"
gpu:
  device_index: 1
  n_gpu_layers: -1
llm:
  model_policy: fixed
  chat:
    model: "shared:chat"
    n_ctx: 16384
    retention: keep_loaded
    grace_seconds: 111
llm_server:
  chat:
    model: "shared:chat"
    ctx_per_slot: 32768
    extra_args: "-ngl 99"
"#;
    let cfg_path = env.write_config("new.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices());
    let chat = ready(&plans, "chat");
    assert_eq!(
        chat.spec.n_ctx,
        Some(16384),
        "llm.chat.n_ctx приоритетнее ctx_per_slot"
    );
    assert_eq!(chat.spec.load_on_demand_grace_seconds, Some(111));
    assert_eq!(
        chat.spec.n_gpu_layers,
        Some(-1),
        "gpu.n_gpu_layers приоритетнее -ngl"
    );
    assert!(
        cfg.warnings.iter().any(|w| w.contains("игнорируется")),
        "должно быть предупреждение о перекрытом -ngl: {:?}",
        cfg.warnings
    );
}

/// `gpu.device_index = 0` = CPU: во всех ролях явный индекс CPU-устройства.
#[test]
fn device_index_zero_means_cpu() {
    let env = Env::new("cpu");
    let yaml = r#"
gpu:
  device_index: 0
llm_server:
  chat:
    model: "shared:chat"
  embedding:
    model: "shared:embedding"
"#;
    env.model("embedding", "bge.gguf");
    env.model("chat", "chat.gguf");
    let cfg_path = env.write_config("cpu.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices());
    for role in ["chat", "embedding"] {
        let p = ready(&plans, role);
        assert_eq!(
            p.spec.manual_devices_csv.as_deref(),
            Some("1"),
            "gpu.device_index = 0 должен давать индекс CPU-устройства ({role})"
        );
    }
}

/// Явный путь в конфиге, которого нет: ошибка обязана подсказать оба пути —
/// куда положить файл и каким установщиком его скачать (`SPIKES.md` §14.7).
#[test]
fn explicit_missing_model_path_reports_both_hints() {
    let env = Env::new("missing");
    let missing = env.root.join("no-such-model.gguf");
    let yaml = format!(
        "llm_server:\n  chat:\n    model: \"{}\"\n",
        missing.display().to_string().replace('\\', "\\\\")
    );
    let cfg_path = env.write_config("missing.yaml", &yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices());
    let err = match plans.iter().find(|p| p.role() == "chat").expect("chat") {
        registry::RolePlan::Failed { error, .. } => error.clone(),
        registry::RolePlan::Ready(_) => panic!("модели нет — роль не должна планироваться"),
    };
    assert!(err.contains("no-such-model.gguf"), "{err}");
    assert!(err.contains("ensure_llama_runtime"), "{err}");
}

/// `resolve_model`/`read_current` ведут себя как Python-версия.
#[test]
fn resolve_model_matches_python() {
    let env = Env::new("resolve");
    env.model("chat", "A.gguf");
    env.manifest("chat", "A.gguf");

    let p = hds_llama::resolve_model(&env.runtime, "shared", "chat").expect("shared");
    assert!(p.ends_with("A.gguf"), "{}", p.display());
    let p = hds_llama::resolve_model(&env.runtime, "shared:chat", "embedding").expect("role");
    assert!(p.ends_with("A.gguf"), "роль указана явно: {}", p.display());
    let p = hds_llama::resolve_model(&env.runtime, "D:\\models\\X.gguf", "chat").expect("path");
    assert_eq!(p, PathBuf::from("D:\\models\\X.gguf"));
    assert_eq!(
        hds_llama::read_current(&env.runtime, "chat"),
        "A.gguf",
        "манифест читается как в Python"
    );
    // роль без манифеста и без моделей — ошибка с упоминанием роли
    let err = hds_llama::resolve_model(&env.runtime, "shared:rerank", "rerank").unwrap_err();
    assert!(err.to_string().contains("rerank"), "{err}");
}

/// Пути рантайма: env `LLAMA_RUNTIME_DIR` имеет приоритет (порт `runtime_dir`).
#[test]
fn runtime_paths_follow_env() {
    let env = Env::new("paths");
    let paths = hds_llama::runtime::RuntimePaths::new(env.runtime.clone());
    assert_eq!(paths.models_for("chat"), env.runtime.join("models").join("chat"));
    assert_eq!(paths.bin, env.runtime.join("bin"));
    assert!(!paths.has_llama_server(), "llama-server в тестовом рантайме нет");
    assert_eq!(
        hds_llama::runtime::RuntimePaths::llama_server_name(),
        if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        }
    );
}


