//! A4 шаг 2: решения диспетчера VRAM — порядок вытеснения по `gpu.priorities`,
//! пауза индексации, отчёт о нехватке **без авто-деградации** (§8.6.2, ARB-1…ARB-5).
//!
//! Тесты чистые: без движка, GPU и файловой системы — проверяются решения, а не
//! вызовы кластера (`dispatch::apply` для этого требует живой `Cluster`).

use hds_llama::config::{GpuConfig, GpuPolicy};
use hds_llama::dispatch::{
    eviction_order, idle_evictions, plan_indexing, plan_query, Action, Demand, InstanceUse, Verdict,
};
use hds_llama::ffi::state;

/// Конфиг по умолчанию: приоритеты `chat 100, embedding 40, rerank 30, whisper 20`,
/// `gpu.reserve_mb = 1024`, `gpu.evict_idle_sec = 600`, `gpu.policy = query_priority`.
fn gpu() -> GpuConfig {
    GpuConfig::default()
}

/// Кто именно выгружается (в порядке действий плана).
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

/// Базовый набор: все четыре роли загружены и не заняты.
fn all_roles() -> Vec<InstanceUse> {
    vec![
        InstanceUse::new("chat", "chat", 5000).keep_loaded(),
        InstanceUse::new("embedding", "embedding", 600),
        InstanceUse::new("rerank", "rerank", 500),
        InstanceUse::new("whisper", "whisper", 400),
    ]
}

/// Порядок вытеснения — по возрастанию приоритета: whisper → rerank → embedding,
/// роль, под которую просят память (чат), не попадает в список.
#[test]
fn eviction_order_follows_priorities() {
    let order = eviction_order(&gpu(), &all_roles(), "chat");
    let names: Vec<&str> = order.iter().map(|i| i.role.as_str()).collect();
    assert_eq!(names, vec!["whisper", "rerank", "embedding"]);
}

/// Занятые запросом и чужие (не наши) инстансы не вытесняются никогда.
#[test]
fn busy_and_foreign_instances_are_never_evicted() {
    let instances = vec![
        InstanceUse::new("embedding", "embedding", 600).busy(1, 0),
        InstanceUse::new("whisper", "whisper", 400).foreign(),
        InstanceUse::new("rerank", "rerank", 500),
        InstanceUse::new("chat", "chat", 5000).keep_loaded(),
    ];
    let names: Vec<String> = eviction_order(&gpu(), &instances, "chat")
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, vec!["rerank"], "занятый embedding и чужой whisper — не трогаем");
}

/// Выгруженный инстанс освобождать нечего.
#[test]
fn unloaded_instances_are_skipped() {
    let instances = vec![
        InstanceUse::new("whisper", "whisper", 400).with_state(state::UNLOADED),
        InstanceUse::new("rerank", "rerank", 500).with_state(state::GRACE),
    ];
    let names: Vec<String> = eviction_order(&gpu(), &instances, "chat")
        .iter()
        .map(|i| i.name.clone())
        .collect();
    assert_eq!(names, vec!["rerank"], "GRACE ещё держит VRAM, UNLOADED — нет");
}

/// Хватает сразу: ни паузы, ни вытеснения — ARB-3 (не трогаем индексные роли «на всякий»).
#[test]
fn query_fits_without_any_action() {
    let plan = plan_query(&gpu(), Some(12_000), &Demand::new("chat", 5_000), &all_roles());
    assert_eq!(plan.verdict, Verdict::Fits);
    assert!(!has_pause(&plan), "пауза не нужна: {:?}", plan.actions);
    assert!(unload_names(&plan).is_empty());
    assert_eq!(plan.total_mib(), 6_024, "нужно 5000 + резерв 1024");
    assert!(
        plan.actions
            .iter()
            .any(|a| matches!(a, Action::EnsureLoaded { role } if role == "chat")),
        "роль запроса должна быть загружена"
    );
}

/// Не хватает немного — выгружаем самый низкий приоритет (whisper), ставим паузу.
#[test]
fn query_evicts_lowest_priority_first_and_pauses_index() {
    let plan = plan_query(&gpu(), Some(3_000), &Demand::new("chat", 2_000), &all_roles());
    assert_eq!(plan.verdict, Verdict::FitsAfterEviction);
    assert!(has_pause(&plan), "при нехватке индексация обязана встать на паузу");
    assert_eq!(unload_names(&plan), vec!["whisper"], "хватает одного самого младшего");
    assert_eq!(plan.freed_mib, 400);
    let pause_reason = plan
        .actions
        .iter()
        .find_map(|a| match a {
            Action::PauseIndex { reason } => Some(reason.clone()),
            _ => None,
        })
        .expect("причина паузы");
    assert!(
        pause_reason.contains("3024") && pause_reason.contains("3000"),
        "в причине паузы — точные цифры: {pause_reason}"
    );
}

/// Вытесняем столько, сколько нужно: хватает двух младших, embedding не трогаем.
#[test]
fn query_stops_evicting_when_enough() {
    let plan = plan_query(&gpu(), Some(2_000), &Demand::new("chat", 1_400), &all_roles());
    assert_eq!(plan.verdict, Verdict::FitsAfterEviction);
    assert_eq!(
        unload_names(&plan),
        vec!["whisper", "rerank"],
        "нужно освободить 424 МиБ: whisper (400) не хватает, добавляем rerank (500)"
    );
    assert_eq!(plan.freed_mib, 900);
}

/// A-7/ARB-4: не влезает даже после вытеснения — точные цифры в отчёте, и **никакой**
/// авто-деградации (квант/n_ctx/n_gpu_layers не меняются).
#[test]
fn shortage_is_reported_without_auto_degradation() {
    let mut cfg = gpu();
    cfg.vram_budget_mb = Some(6_000); // искусственное сужение бюджета (критерий A-7)
    let plan = plan_query(&cfg, Some(12_000), &Demand::new("chat", 11_564), &all_roles());

    assert_eq!(plan.verdict, Verdict::NotEnough { short_mib: 5_088 });
    assert_eq!(plan.free_mib, Some(6_000), "cap сузил замер свободной VRAM");
    assert_eq!(plan.need_mib, 11_564, "потребность не «уточняется» вниз");
    assert_eq!(plan.freed_mib, 1_500, "вытеснили всё, что можно");
    let msg = plan.shortage().expect("отчёт о нехватке обязателен");
    for number in ["11564", "12588", "6000", "1500", "5088"] {
        assert!(msg.contains(number), "в отчёте нет цифры {number}: {msg}");
    }
    assert!(
        msg.contains("llm.model_policy: fixed"),
        "нужно прямо сказать, что деградации нет: {msg}"
    );
    // конфиг не менялся — деградации нет ни в решении, ни в параметрах
    assert_eq!(cfg.n_gpu_layers, -1);
    assert_eq!(cfg.reserve_mb, 1024);
}

/// Без замера VRAM диспетчер не гадает: сообщает и не вытесняет.
#[test]
fn missing_vram_measurement_is_reported_not_guessed() {
    let plan = plan_query(&gpu(), None, &Demand::new("chat", 5_000), &all_roles());
    assert_eq!(plan.verdict, Verdict::Unknown);
    assert!(unload_names(&plan).is_empty());
    assert!(!has_pause(&plan));
    assert!(
        plan.notes.iter().any(|n| n.contains("нет замера")),
        "пояснение обязано быть: {:?}",
        plan.notes
    );
}

/// `gpu.policy: manual` — только отчёт, никаких автоматических действий.
#[test]
fn manual_policy_only_reports() {
    let mut cfg = gpu();
    cfg.policy = GpuPolicy::Manual;
    let plan = plan_query(&cfg, Some(1_000), &Demand::new("chat", 5_000), &all_roles());
    assert_eq!(plan.verdict, Verdict::NotEnough { short_mib: 5_024 });
    assert!(unload_names(&plan).is_empty());
    assert!(!has_pause(&plan));
    assert!(plan.shortage().unwrap().contains("manual"));
}

/// `gpu.policy: indexing_priority` — индексные роли не вытесняем, запрос получает отчёт.
#[test]
fn indexing_priority_protects_index_roles() {
    let mut cfg = gpu();
    cfg.policy = GpuPolicy::IndexingPriority;
    let plan = plan_query(&cfg, Some(1_000), &Demand::new("chat", 5_000), &all_roles());
    assert_eq!(plan.verdict, Verdict::NotEnough { short_mib: 5_024 });
    assert!(unload_names(&plan).is_empty(), "индексные роли protected");
    assert!(plan.shortage().unwrap().contains("indexing_priority"));
}

/// `gpu.pause_index_on_query: false` — паузу не ставим, но вытесняем по приоритетам.
#[test]
fn pause_can_be_disabled_explicitly() {
    let mut cfg = gpu();
    cfg.pause_index_on_query = false;
    let plan = plan_query(&cfg, Some(2_000), &Demand::new("chat", 1_400), &all_roles());
    assert!(!has_pause(&plan));
    assert_eq!(unload_names(&plan), vec!["whisper", "rerank"]);
    assert!(
        plan.notes.iter().any(|n| n.contains("pause_index_on_query = false")),
        "{:?}",
        plan.notes
    );
}

/// ARB-3: индексации при нехватке VRAM разрешено выгрузить чат (он вернётся сам).
#[test]
fn indexing_may_evict_chat_but_not_its_own_role() {
    let instances = vec![
        InstanceUse::new("chat", "chat", 7_000).keep_loaded().with_idle(30),
        InstanceUse::new("embedding", "embedding", 600),
        InstanceUse::new("whisper", "whisper", 400),
    ];
    // просим память под whisper: его роль не вытесняем, embedding — тоже (он нужен индексации)
    let plan = plan_indexing(&gpu(), Some(1_000), &Demand::new("whisper", 3_000), &instances);
    assert_eq!(plan.verdict, Verdict::FitsAfterEviction);
    assert_eq!(
        unload_names(&plan),
        vec!["chat"],
        "освобождаем резидента, а не index-роли"
    );
    assert!(!has_pause(&plan), "индексация — это и есть работа, паузу не ставим");
}

/// Простой: внешний предохранитель `gpu.evict_idle_sec` + grace роли (ARB-5).
#[test]
fn idle_evictions_respect_grace_and_fuse() {
    let instances = vec![
        InstanceUse::new("embedding", "embedding", 600).with_grace(300).with_idle(400),
        InstanceUse::new("rerank", "rerank", 500).with_idle(700),
        InstanceUse::new("chat", "chat", 5_000).keep_loaded().with_idle(300),
        InstanceUse::new("whisper", "whisper", 400).with_idle(10_000).busy(1, 0),
    ];
    let actions = idle_evictions(&gpu(), &instances);
    let names: Vec<String> = actions
        .iter()
        .filter_map(|a| match a {
            Action::Unload { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        names,
        vec!["embedding", "rerank"],
        "embedding по grace 300, rerank по фьюзу 600; чат (300 с) и занятый whisper — нет"
    );
}

/// `gpu.evict_idle_sec: 0` и `manual` полностью выключают вытеснение по простою.
#[test]
fn idle_fuse_can_be_disabled() {
    let instances = vec![InstanceUse::new("embedding", "embedding", 600).with_idle(99_999)];
    let mut cfg = gpu();
    cfg.evict_idle_sec = 0;
    assert!(idle_evictions(&cfg, &instances).is_empty(), "фьюз выключен нулём");

    let mut cfg = gpu();
    cfg.policy = GpuPolicy::Manual;
    assert!(idle_evictions(&cfg, &instances).is_empty(), "manual = ничего сами");
}
