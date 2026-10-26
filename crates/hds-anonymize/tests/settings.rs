//! Настройки `anonymize.*` → аргументы воркера (§2.6): дефолты, notices,
//! абсолютные пути, профиль.

use hds_anonymize::settings::Settings;
use hds_core::config::Config;

fn cfg(yaml: &str) -> Config {
    serde_yaml::from_str(yaml).expect("тестовый конфиг")
}

#[test]
fn defaults_match_plan() {
    let s = Settings::from_config(&Config::Null);
    assert_eq!(s.output_dir, "data/anonymized");
    assert_eq!(s.db_path, "data/anonymize.db");
    assert_eq!(s.model, "knowledgator/gliner-pii-large-v1.0");
    assert_eq!(s.threshold, 0.3, "порог 0.3 — НЕ 0.5 (§2.6)");
    assert_eq!(s.max_input_chars, 1500);
    assert_eq!(s.chunk_overlap, 200);
    assert_eq!(s.timeout_sec, 300);
    assert!(s.natasha);
    assert_eq!(s.max_file_mb, 200);
    assert_eq!(s.gpu, "auto");
    assert_eq!(s.request_timeout_sec, 600);
    assert_eq!(s.idle_timeout_sec, 60);
    assert!(s.notices.is_empty());
}

#[test]
fn worker_args_for_carries_device_and_gpu_limit() {
    let s = Settings::from_config(&Config::Null);
    let root = std::env::temp_dir();
    // CPU (P2-совместимость): без лимита
    let cpu = s.worker_args(&root);
    let val = |args: &[String], name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    assert_eq!(val(&cpu, "--device").as_deref(), Some("cpu"));
    assert_eq!(val(&cpu, "--gpu-mem-limit-mb"), None);
    // GPU (P3, lease): --device cuda + лимит ORT из lease
    let gpu = s.worker_args_for(&root, "cuda", 4096);
    assert_eq!(val(&gpu, "--device").as_deref(), Some("cuda"));
    assert_eq!(val(&gpu, "--gpu-mem-limit-mb").as_deref(), Some("4096"));
    // лимит 0 — флаг не ставится (размер аренды контролирует ORT)
    let no_limit = s.worker_args_for(&root, "cuda", 0);
    assert_eq!(val(&no_limit, "--gpu-mem-limit-mb"), None);
}

#[test]
fn gpu_settings_have_defaults() {
    let s = Settings::from_config(&Config::Null);
    assert_eq!(s.gpu, "auto");
    assert_eq!(s.llm_host_url, "http://127.0.0.1:8010");
    assert_eq!(s.gpu_reserve_mb, 2048);
    assert_eq!(s.lease_timeout_sec, 120);
}

#[test]
fn values_are_carried_to_worker_args() {
    let s = Settings::from_config(&cfg(
        "anonymize:\n  threshold: 0.35\n  natasha: false\n  timeout_sec: 120\n",
    ));
    let root = std::env::temp_dir();
    let args = s.worker_args(&root);
    let val = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    assert_eq!(val("--threshold").as_deref(), Some("0.35"));
    assert_eq!(val("--natasha").as_deref(), Some("0"));
    assert_eq!(val("--timeout").as_deref(), Some("120"));
    assert_eq!(val("--device").as_deref(), Some("cpu"), "P2: только CPU");
    // абсолютные пути уходят воркеру
    assert!(val("--db").unwrap().contains("anonymize.db"));
    assert!(val("--models-dir").unwrap().ends_with("anonymize"));
}

#[test]
fn bad_threshold_and_chars_become_notices() {
    let s = Settings::from_config(&cfg(
        "anonymize:\n  threshold: 1.5\n  max_input_chars: 10\n",
    ));
    assert_eq!(s.threshold, 0.3, "некорректный порог → дефолт");
    assert_eq!(s.max_input_chars, 1500);
    assert!(s.notices.len() >= 2, "{:?}", s.notices);
}

#[test]
fn relative_paths_resolve_against_root() {
    let s = Settings::from_config(&Config::Null);
    let root = Path::new("D:/proj");
    assert!(s.db_abs(root).starts_with(root));
    assert!(s.output_dir_abs(root).starts_with(root));
    let abs = s.db_abs(root);
    assert!(abs.ends_with("data/anonymize.db") || abs.ends_with("data\\anonymize.db"));
}

use std::path::Path;
