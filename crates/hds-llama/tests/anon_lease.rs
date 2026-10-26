//! P3 (PLAN_ANONYMIZER_SIDECAR.md §2.2): план входа в lease анонимизации —
//! `index.pause`, вытеснение всех незанятых инстансов (включая idle-chat),
//! занятые защищены, отчёт о нехватке без авто-деградации. Тесты чистые,
//! без движка и GPU.

use hds_llama::config::GpuConfig;
use hds_llama::dispatch::{plan_anon_enter, Action, InstanceUse, Verdict, ANON_ROLE};

fn unload_names(plan: &hds_llama::dispatch::Plan) -> Vec<String> {
    plan.actions
        .iter()
        .filter_map(|a| match a {
            Action::Unload { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn has_pause(plan: &hds_llama::dispatch::Plan) -> bool {
    plan.actions
        .iter()
        .any(|a| matches!(a, Action::PauseIndex { .. }))
}

/// P3: резерв NVML ~2 ГБ учитывается как потребность роли `anon`.
#[test]
fn anon_needs_reserve_in_budget() {
    let gpu = GpuConfig::default(); // reserve_mb = 1024
    let instances = vec![InstanceUse::new("chat", "chat", 5000).keep_loaded()];
    // свободно 4000, нужно 2048 + 1024 резерв → хватает, idle-chat вытеснять не надо
    let plan = plan_anon_enter(&gpu, Some(4000), &instances, 2048);
    assert_eq!(plan.verdict, Verdict::FitsAfterEviction);
    assert!(unload_names(&plan).is_empty(), "вытеснения не требуется");
    assert!(has_pause(&plan), "индексация ставится на паузу");
    // потребность роли — 'anon'
    assert!(plan
        .lines()
        .iter()
        .any(|l| l.contains(ANON_ROLE) || l.contains("анонимизац")));
}

#[test]
fn anon_evicts_idle_roles_low_priority_first() {
    let gpu = GpuConfig::default();
    let instances = vec![
        InstanceUse::new("chat", "chat", 5000).keep_loaded(),
        InstanceUse::new("embedding", "embedding", 600),
        InstanceUse::new("rerank", "rerank", 500),
        InstanceUse::new("whisper", "whisper", 400),
    ];
    // свободно 0, нужно 2048 + 1024: 600+500+400 = 1500 < 3072 → вытесняется и idle-chat
    let plan = plan_anon_enter(&gpu, Some(0), &instances, 2048);
    assert_eq!(plan.verdict, Verdict::FitsAfterEviction);
    let names = unload_names(&plan);
    assert_eq!(names, vec!["whisper", "rerank", "embedding", "chat"]);
    assert!(has_pause(&plan));
}

#[test]
fn anon_does_not_evict_busy_chat_reports_shortage() {
    let gpu = GpuConfig::default();
    let instances = vec![InstanceUse::new("chat", "chat", 5000)
        .keep_loaded()
        .busy(1, 0)];
    // занятый chat не вытесняется: 0 свободно, 0 освобождено → не хватает
    let plan = plan_anon_enter(&gpu, Some(0), &instances, 2048);
    assert!(matches!(plan.verdict, Verdict::NotEnough { .. }));
    assert!(unload_names(&plan).is_empty(), "занятые не трогаем");
    assert!(
        plan.shortage().is_some(),
        "отчёт о нехватке — вызывающий отдаст gpu_busy (503)"
    );
}

#[test]
fn anon_idle_chat_is_evicted_too() {
    let gpu = GpuConfig::default();
    let instances = vec![InstanceUse::new("chat", "chat", 5000).keep_loaded()];
    // свободно 0, нужно 2048+1024, chat 5000 idle → вытесняется (anon — единственный владелец)
    let plan = plan_anon_enter(&gpu, Some(0), &instances, 2048);
    assert_eq!(plan.verdict, Verdict::FitsAfterEviction);
    assert_eq!(unload_names(&plan), vec!["chat"]);
}

#[test]
fn anon_without_vram_measurement_still_pauses_and_evicts() {
    let gpu = GpuConfig::default();
    let instances = vec![InstanceUse::new("embedding", "embedding", 600)];
    let plan = plan_anon_enter(&gpu, None, &instances, 2048);
    assert_eq!(plan.verdict, Verdict::Unknown);
    assert!(has_pause(&plan));
    assert_eq!(unload_names(&plan), vec!["embedding"]);
}
