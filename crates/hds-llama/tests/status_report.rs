//! A4 шаг 2: отчёт `llm-host status` — роли/состояния, бюджет VRAM, пауза,
//! «NVML − baseline» и машинночитаемый JSON (поля из §A4, их читает UI).

use std::collections::BTreeMap;
use std::path::PathBuf;

use hds_llama::config;
use hds_llama::ffi::{model_kind, retention, state};
use hds_llama::pause::HeartbeatState;
use hds_llama::registry;
use hds_llama::status::{StatusInput, StatusReport};
use hds_llama::vram::{VramSnapshot, VramSource};
use hds_llama::{Device, Instance};

/// Временное окружение: конфиг + общий рантайм с моделью чата.
struct Env {
    root: PathBuf,
    runtime: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir()
            .join("hds-llama-tests")
            .join(format!("status-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&root);
        let runtime = root.join("llama-runtime");
        std::fs::create_dir_all(runtime.join("models").join("chat")).expect("runtime");
        std::fs::write(runtime.join("models").join("chat").join("qwen.gguf"), b"GGUF")
            .expect("model");
        Env { root, runtime }
    }

    fn config(&self, yaml: &str) -> config::LlmHostConfig {
        let p = self.root.join("config.yaml");
        std::fs::write(&p, yaml).expect("config");
        config::load(&p).expect("config load")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn devices() -> Vec<Device> {
    vec![
        Device {
            bridge_device_index: 0,
            device_type: 1,
            memory_free: 3_500_000_000,
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

/// Загруженный инстанс чата — как его показал бы `list_instances`.
fn chat_instance(runtime: &PathBuf) -> Instance {
    Instance {
        id: 1,
        name: "chat".to_string(),
        model_path: runtime
            .join("models")
            .join("chat")
            .join("qwen.gguf")
            .display()
            .to_string(),
        state: state::SERVING,
        state_name: state::name(state::SERVING).to_string(),
        retention_mode: retention::KEEP_LOADED,
        load_on_demand_grace_seconds: 0,
        model_kind: model_kind::TEXT,
        active_request_count: 1,
        queued_request_count: 2,
        last_error: String::new(),
    }
}

const CONFIG: &str = r#"
llm_server:
  host: "127.0.0.1"
  parallel: 1
  chat:
    port: 8010
    model: "shared:chat"
    ctx_per_slot: 32768
  embedding:
    port: 8011
    model: "shared:embedding"
    ctx_per_slot: 8192
gpu:
  policy: query_priority
  reserve_mb: 1024
  priorities: { chat: 100, embedding: 40, rerank: 30, whisper: 20 }
"#;

/// Отчёт собирается из снимков: роли из плана, состояние — из кластера,
/// занятость — «NVML − baseline», пауза и heartbeat — из файлов-сигналов.
#[test]
fn report_merges_plan_cluster_and_vram() {
    let env = Env::new("merge");
    let cfg = env.config(CONFIG);
    let planned = registry::plan(&cfg, &env.runtime, &devices());
    let instances = vec![chat_instance(&env.runtime)];
    let mut needs = BTreeMap::new();
    needs.insert("chat".to_string(), 11_564u64);
    needs.insert("embedding".to_string(), 636u64);

    let vram = VramSnapshot {
        total_mib: 12_288,
        used_mib: 8_620,
        free_mib: 3_508,
    };
    let heartbeat = HeartbeatState {
        fresh: true,
        paused: true,
        age_secs: 3,
        seen: 120,
        processed: 118,
        errors: 0,
        chunks: 4_567,
        ..Default::default()
    };
    let report = StatusReport::build(StatusInput {
        config: &cfg,
        runtime_root: &env.runtime,
        engine_dir: None,
        devices: &devices(),
        instances: &instances,
        vram: Some(vram),
        vram_source: VramSource::Nvml,
        baseline_used_mib: Some(1_320),
        paused: true,
        pause_file: &env.root.join("index.pause"),
        heartbeat: Some(heartbeat),
        planned: &planned,
        needs: &needs,
        decision: None,
        forecast: None,
    });

    // роли: чат живой (SERVING), embedding в плане, но в этом процессе не поднят
    let chat = report
        .roles
        .iter()
        .find(|r| r.role == "chat")
        .expect("chat");
    assert_eq!(chat.state, "SERVING");
    assert!(chat.running);
    assert_eq!(chat.retention, "keep");
    assert_eq!(chat.n_ctx, 32_768);
    assert_eq!(chat.need_mib, 11_564);
    assert_eq!(chat.base_url, "http://127.0.0.1:8010/v1");
    assert_eq!(chat.devices_csv, "0", "первый GPU = bridge-индекс 0 (A1)");
    assert_eq!(chat.active_requests, 1);
    assert_eq!(chat.queued_requests, 2);
    assert_eq!(
        chat.vram_measured_mib,
        Some(7_300),
        "вся дельта NVML — на чат (единственная загруженная роль)"
    );

    let embedding = report
        .roles
        .iter()
        .find(|r| r.role == "embedding")
        .expect("embedding");
    assert!(!embedding.running, "инстанс не поднят в этом процессе");
    assert_eq!(embedding.state, "—");

    // бюджет и наша занятость
    assert_eq!(report.effective_free_mib, Some(3_508));
    assert_eq!(report.used_by_us_mib, Some(8_620 - 1_320));
    assert_eq!(report.vram_source, "nvml");

    // пауза и heartbeat
    assert!(report.paused);
    assert!(report.heartbeat.as_ref().unwrap().paused);

    // человекочитаемые строки — с цифрами (то, что увидит заказчик)
    let text = report.lines().join("\n");
    for expected in [
        "VRAM (nvml)",
        "индексация: пауза ДА",
        "11564 МиБ",
        "замер≈7300 МиБ",
        "наша занятость (NVML − baseline 1320 МиБ): 7300 МиБ",
        "диспетчер VRAM: gpu.policy query_priority",
    ] {
        assert!(text.contains(expected), "нет строки «{expected}»:\n{text}");
    }

    // JSON: поля, которые читает UI (§A4)
    let json = report.json();
    assert_eq!(json["gpu"]["policy"], "query_priority");
    assert_eq!(json["gpu"]["priorities"]["chat"], 100);
    assert_eq!(json["index"]["paused"], true);
    assert_eq!(json["index"]["heartbeat"]["chunks"], 4_567);
    assert_eq!(json["vram"]["free_mib"], 3_508);
    assert_eq!(json["roles"][0]["role"], "chat");
    assert_eq!(json["roles"][0]["state"], "SERVING");
    assert_eq!(json["roles"][0]["active_request_count"], 1);
    assert_eq!(json["roles"][0]["vram_measured_mib"], 7_300);
    assert_eq!(json["effective_free_mib"], 3_508);
}

/// Сузили бюджет (`vram_budget_mb`) и указали чужих потребителей (`external_vram_mb`):
/// «к использованию» уменьшается, и это видно в отчёте (критерий A-7).
#[test]
fn budget_cap_and_external_usage_are_visible() {
    let env = Env::new("cap");
    // отдельный конфиг: cap 4000 МиБ, вычет 500 МиБ на чужих, источник — движок (R29)
    let cfg = env.config(
        r#"
gpu:
  vram_budget_mb: 4000
  external_vram_mb: 500
  vram_source: engine
llm_server:
  chat:
    model: "shared:chat"
"#,
    );
    let vram = VramSnapshot {
        total_mib: 12_288,
        used_mib: 8_780,
        free_mib: 3_508,
    };
    let needs = BTreeMap::new();
    let report = StatusReport::build(StatusInput {
        config: &cfg,
        runtime_root: &env.runtime,
        engine_dir: None,
        devices: &[],
        instances: &[],
        vram: Some(vram),
        vram_source: cfg.gpu.vram_source,
        baseline_used_mib: None,
        paused: false,
        pause_file: &env.root.join("index.pause"),
        heartbeat: None,
        planned: &[],
        needs: &needs,
        decision: None,
        forecast: None,
    });

    assert_eq!(
        report.effective_free_mib,
        Some(3_508 - 500),
        "cap 4000 не режет (свободно 3508), но вычет на чужих — вычитается"
    );
    assert_eq!(report.vram_budget_mb, Some(4_000));
    assert_eq!(report.external_vram_mb, 500);
    assert_eq!(report.used_by_us_mib, None, "без baseline наша занятость неизвестна");
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("list_devices` недостоверны")),
        "предупреждение R29 про engine-источник обязано быть: {:?}",
        report.warnings
    );
    assert!(
        report.lines().join("\n").contains("cap 4000 МиБ"),
        "отчёт обязан печатать бюджет: {}",
        report.lines().join("\n")
    );
}
