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
        let root = std::env::temp_dir().join("hds-llama-tests").join(format!(
            "{}-{}",
            std::process::id(),
            name
        ));
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
    assert_eq!(
        plans.len(),
        3,
        "chat + embedding + rerank (whisper без модели)"
    );
    assert!(plans.iter().all(|p| !p.is_failed()), "{plans:?}");

    let chat_p = ready(&plans, "chat");
    assert_eq!(chat_p.model_path, chat);
    assert_eq!(
        chat_p.spec.manual_devices_csv.as_deref(),
        Some("0"),
        "CUDA0"
    );
    assert_eq!(chat_p.spec.allow_cpu, Some(false));
    // Смена политики (30.10.2026): чат по умолчанию load_on_demand — не резидент.
    // Прежнее поведение возвращает явный `llm.chat.retention: keep_loaded`
    // (проверяется в `new_keys_win_over_legacy_and_gpu_layers_override`).
    assert_eq!(chat_p.spec.retention_mode, Some(retention::LOAD_ON_DEMAND));
    assert_eq!(chat_p.spec.model_kind, Some(model_kind::TEXT));
    assert_eq!(chat_p.spec.n_ctx, Some(32768));
    assert_eq!(
        chat_p.spec.n_gpu_layers,
        Some(99),
        "-ngl 99 из legacy extra_args"
    );
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
    assert_eq!(
        rr.spec.manual_devices_csv.as_deref(),
        Some("1"),
        "CPU-устройство"
    );
    assert_eq!(rr.spec.allow_cpu, Some(true));
    assert_eq!(
        rr.spec.n_gpu_layers, None,
        "для CPU-роли слои на GPU не задаём"
    );
    assert_eq!(rr.spec.reranking, Some(true));
    assert!(
        rr.notes.iter().any(|n| n.contains("на CPU")),
        "должно быть пояснение про CPU-роль: {:?}",
        rr.notes
    );

    // С нашим патчем движка KV-флаги из legacy `extra_args` больше не попадают в
    // предупреждения: они доезжают до llama.cpp через план инстанса.
    assert!(
        !cfg.warnings.iter().any(|w| w.contains("--cache-type-k")),
        "предупреждения про KV-флаги быть не должно: {:?}",
        cfg.warnings
    );
    assert_eq!(
        chat_p.spec.cache_type_k,
        Some(8),
        "q8_0 из extra_args → ggml_type в плане инстанса"
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
        chat.spec.retention_mode,
        Some(retention::KEEP_LOADED),
        "явный llm.chat.retention: keep_loaded сильнее нового дефолта load_on_demand"
    );
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
    assert_eq!(
        paths.models_for("chat"),
        env.runtime.join("models").join("chat")
    );
    assert_eq!(paths.bin, env.runtime.join("bin"));
    assert!(
        !paths.has_llama_server(),
        "llama-server в тестовом рантайме нет"
    );
    assert_eq!(
        hds_llama::runtime::RuntimePaths::llama_server_name(),
        if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        }
    );
}

/// Фейковые устройства как в факте T0 (с Vulkan-бэкендом): ловушка компата —
/// `index=2` ускорителя — это AMD iGPU, а не NVIDIA.
fn fake_devices_with_vulkan() -> Vec<Device> {
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
            memory_free: 31_000_000_000,
            memory_total: 34_300_000_000,
            backend: "Vulkan".to_string(),
            name: "Vulkan0".to_string(),
            description: "AMD Radeon(TM) Graphics".to_string(),
        },
        Device {
            bridge_device_index: 2,
            device_type: 1,
            memory_free: 12_024_475_648,
            memory_total: 12_930_293_760,
            backend: "Vulkan".to_string(),
            name: "Vulkan1".to_string(),
            description: "NVIDIA GeForce RTX 3060".to_string(),
        },
        Device {
            bridge_device_index: 3,
            device_type: 0,
            memory_free: 44_040_192_000,
            memory_total: 67_108_864_000,
            backend: "CPU".to_string(),
            name: "CPU".to_string(),
            description: "AMD Ryzen 7 5700G".to_string(),
        },
    ]
}

fn failed(plans: &[registry::RolePlan], role: &str) -> String {
    match plans.iter().find(|p| p.role() == role).expect(role) {
        registry::RolePlan::Ready(p) => panic!("роль {role} не должна планироваться: {p:?}"),
        registry::RolePlan::Failed { error, .. } => error.clone(),
    }
}

/// M6 T1 (§3.1): раскладка `gpu.devices` ведёт роли на назначенные карты,
/// backend карты виден в плане/статусе (`device_backend`, `summary`).
#[test]
fn layout_assigns_roles_to_cards_and_reports_backend() {
    let env = Env::new("m6layout");
    let yaml = r#"
gpu:
  devices:
    - { index: 1, roles: [chat] }
    - { index: 2, roles: [embedding] }
llm_server:
  chat: { port: 8010, model: "shared:chat" }
  embedding: { port: 8011, model: "shared:embedding" }
"#;
    env.model("chat", "chat.gguf");
    env.model("embedding", "bge.gguf");
    let cfg_path = env.write_config("m6.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices_with_vulkan());

    let chat = ready(&plans, "chat");
    assert_eq!(
        chat.spec.manual_devices_csv.as_deref(),
        Some("0"),
        "карта 1 = первый ускоритель = CUDA0 (bridge 0)"
    );
    assert_eq!(chat.device_backend.as_deref(), Some("CUDA"));
    let embedding = ready(&plans, "embedding");
    assert_eq!(
        embedding.spec.manual_devices_csv.as_deref(),
        Some("1"),
        "карта 2 = второй ускоритель = Vulkan0 (AMD iGPU, bridge 1) — ловушка компата"
    );
    assert_eq!(
        embedding.device_backend.as_deref(),
        Some("Vulkan"),
        "backend карты обязан быть виден в плане/статусе (§3.1)"
    );
    assert!(
        embedding.summary().contains("backend=Vulkan"),
        "summary печатает backend: {}",
        embedding.summary()
    );
}

/// M6 T1 (§3.5): «одна роль — ровно одна карта» — двойное назначение
/// (llm_server.<role>.device и gpu.devices одновременно) — fail-fast с числами.
#[test]
fn double_assignment_fails_with_numbers() {
    let env = Env::new("m6double");
    let yaml = r#"
gpu:
  devices:
    - { index: 1, roles: [chat, embedding] }
llm_server:
  chat: { port: 8010, model: "shared:chat" }
  embedding: { port: 8011, model: "shared:embedding", device: 2 }
"#;
    env.model("chat", "chat.gguf");
    env.model("embedding", "bge.gguf");
    let cfg_path = env.write_config("m6d.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices_with_vulkan());
    let err = failed(&plans, "embedding");
    assert!(err.contains("более чем на одну карту"), "{err}");
    assert!(err.contains("llm_server.embedding.device=2"), "{err}");
    assert!(err.contains("карта 1"), "{err}");
    // Ошибка одной роли не блокирует остальные (паритет Python-версии).
    let _ = ready(&plans, "chat");
}

/// M6 T1 (§3.1): карта из раскладки, которой нет среди ускорителей — fail-fast
/// с числом ускорителей и именем роли (не молча, как в грабле «Vulkan не видно»).
#[test]
fn missing_card_fails_with_role_and_numbers() {
    let env = Env::new("m6missing");
    let yaml = r#"
gpu:
  devices:
    - { index: 3, roles: [chat] }
llm_server:
  chat: { port: 8010, model: "shared:chat" }
"#;
    env.model("chat", "chat.gguf");
    let cfg_path = env.write_config("m6m.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    // Только один ускоритель (CUDA0) — карты 3 нет.
    let devices = vec![fake_devices_with_vulkan()[0].clone()];
    let plans = registry::plan(&cfg, &env.runtime, &devices);
    let err = failed(&plans, "chat");
    assert!(err.contains("роль 'chat'"), "{err}");
    assert!(err.contains("ускорителей найдено 1"), "{err}");
    assert!(err.contains("llm-host devices"), "{err}");
}

/// M6 T2 (§3.1/§3.5): bridge-индекс карты роли виден в плане (ключ бюджета VRAM
/// и свидетельство раскладки в статусе).
#[test]
fn plan_reports_bridge_device_index() {
    let env = Env::new("m6bridge");
    let yaml = r#"
gpu:
  devices:
    - { index: 2, roles: [embedding] }
llm_server:
  embedding: { port: 8011, model: "shared:embedding" }
"#;
    env.model("embedding", "bge.gguf");
    let cfg_path = env.write_config("m6b.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices_with_vulkan());
    let emb = ready(&plans, "embedding");
    assert_eq!(
        emb.bridge_device_index,
        Some(1),
        "карта 2 = Vulkan0 (bridge 1)"
    );
    assert!(emb.summary().contains("card=1"), "{}", emb.summary());
}

/// M6 T2 (§3.5/§3.6): чат-реплики — одинаковые роли (`chat`, `chat-2`) со своими
/// портами и слотами; допускаются на одной карте (делят её VRAM), маршрутизация
/// по ним — внутри хоста (§3.6).
#[test]
fn chat_replica_shares_card_and_keeps_slots() {
    let env = Env::new("m6replica");
    let yaml = r#"
llm_server:
  parallel: 5
  chat: { port: 8010, model: "shared:chat" }
  chat-2: { port: 8030, model: "shared:chat" }
"#;
    env.model("chat", "chat.gguf");
    let cfg_path = env.write_config("m6r.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    assert!(cfg.errors.is_empty(), "{:?}", cfg.errors);
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices());
    let chat = ready(&plans, "chat");
    let chat2 = ready(&plans, "chat-2");
    assert_eq!(
        chat2.model_path, chat.model_path,
        "реплики — одна модель (§3.5)"
    );
    assert_eq!(
        chat2.spec.manual_devices_csv, chat.spec.manual_devices_csv,
        "реплика на той же карте (можно и на другой — раскладкой)"
    );
    assert_eq!(chat2.port, 8030, "свой порт реплики");
    assert_eq!(chat2.spec.n_parallel, Some(5), "слоты реплики — как у чата");
    assert_eq!(chat2.bridge_device_index, chat.bridge_device_index);
}

/// M6 T2 (§3.5): `whisper` — не более одного экземпляра на карту: два на одной
/// карте — fail-fast с числами (карта и роли названы).
#[test]
fn whisper_twice_on_one_card_fails() {
    let env = Env::new("m6whisper");
    let yaml = r#"
gpu:
  devices:
    - { index: 1, roles: [whisper, whisper-2] }
index:
  whisper_model_path: "shared:whisper"
llm_server:
  chat: { port: 8010, model: "shared:chat" }
"#;
    env.model("chat", "chat.gguf");
    env.model("whisper", "ggml-turbo.gguf");
    let cfg_path = env.write_config("m6w.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices());
    let err = failed(&plans, "whisper-2");
    assert!(err.contains("карту 0"), "число карты: {err}");
    assert!(err.contains("§3.5"), "правило: {err}");
    assert!(
        err.contains("whisper-2") && err.contains("whisper"),
        "{err}"
    );
}

/// M6 T2 (§3.5): индексация — ровно один экземпляр на машину; реплика индексной
/// роли — ошибка контракта конфига (`errors`), роль не создаётся.
#[test]
fn index_role_replica_is_rejected() {
    let env = Env::new("m6idxrepl");
    let yaml = r#"
llm:
  embedding-2: { model: "shared:embedding" }
llm_server:
  chat: { port: 8010, model: "shared:chat" }
"#;
    env.model("chat", "chat.gguf");
    let cfg_path = env.write_config("m6i.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let note = cfg.contract_note().expect("ошибка контракта");
    assert!(
        note.contains("embedding-2") && note.contains("§3.5"),
        "{note}"
    );
    assert!(cfg.role("embedding-2").is_none(), "роль не создаётся");
}

/// M6 T1: явная карта из раскладки печатается в notes плана (диагностика статуса).
#[test]
fn layout_note_explains_explicit_card() {
    let env = Env::new("m6note");
    let yaml = r#"
gpu:
  devices:
    - { index: 2, roles: [embedding] }
llm_server:
  embedding: { port: 8011, model: "shared:embedding" }
"#;
    env.model("embedding", "bge.gguf");
    let cfg_path = env.write_config("m6n.yaml", yaml);
    let cfg = config::load(&cfg_path).expect("config");
    let plans = registry::plan(&cfg, &env.runtime, &fake_devices_with_vulkan());
    let emb = ready(&plans, "embedding");
    assert!(
        emb.notes.iter().any(|n| n.contains("явная карта 2")),
        "notes: {:?}",
        emb.notes
    );
}
