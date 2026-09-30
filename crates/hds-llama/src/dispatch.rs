//! Диспетчер VRAM (A4, шаг 2): вытеснение по `gpu.priorities`, пауза индексации
//! и отчёт о нехватке памяти — **без авто-деградации** (§8.6.2 основного плана,
//! решение заказчика 29.09.2026: `llm.model_policy: fixed`).
//!
//! Модуль разделён на две части намеренно:
//! * **решение** ([`plan_query`], [`plan_indexing`], [`idle_evictions`]) — чистые
//!   функции над снимком инстансов: их проверяют тесты без движка и GPU;
//! * **исполнение** ([`apply`]) — тонкий слой вызовов кластера/файла паузы, который
//!   будет использовать резидентный `llm-host` (A5/A6).
//!
//! Правила (событие → действия), из §8.6.2:
//! | Событие | Действия |
//! |---|---|
//! | запрос, свободной VRAM меньше потребности | `index.pause`, выгрузка по приоритетам `whisper → rerank → embedding`, при нехватке — **отчёт** (без деградации) |
//! | запрос завершён | пауза снимается, индексные роли вернутся сами (`LOAD_ON_DEMAND`) |
//! | индексация без запросов, VRAM не хватает | разрешено выгрузить `chat` (единственный владелец GPU) |
//! | простой роли | встроенный grace движка + внешний предохранитель `gpu.evict_idle_sec` |
//!
//! Замер свободной памяти — **NVML** ([`crate::vram`]); `memory_free` движка
//! недостоверен (R29, расхождение до +7,7 ГБ) и в решения не попадает.

use std::sync::Arc;

use crate::cluster::{Cluster, Instance};
use crate::config::{GpuConfig, GpuPolicy};
use crate::ffi::{retention, state};
use crate::pause::IndexPause;

/// Инстанс глазами арбитра: поля `list_instances` + учёт использования.
#[derive(Debug, Clone)]
pub struct InstanceUse {
    pub name: String,
    pub role: String,
    pub state: i32,
    pub retention_mode: i32,
    /// Сколько VRAM освободит выгрузка (оценка «модель + KV» или замер).
    pub vram_mib: u64,
    pub active_requests: i32,
    pub queued_requests: i32,
    /// Владеет ли инстансом HDS. Чужой инстанс не вытесняем: им может
    /// пользоваться `anonymizer_proxy` (§8.4).
    pub owned: bool,
    /// Сколько секунд инстанс без запросов (для `gpu.evict_idle_sec`).
    pub idle_secs: u64,
    /// Grace роли (`load_on_demand_grace_seconds`): движок выгружает сам.
    pub grace_seconds: i32,
}

impl InstanceUse {
    /// Загруженный инстанс арбитра: владелец — HDS, простоя не было.
    pub fn new(role: &str, name: &str, vram_mib: u64) -> InstanceUse {
        InstanceUse {
            name: name.to_string(),
            role: role.to_string(),
            state: state::LOADED,
            retention_mode: retention::LOAD_ON_DEMAND,
            vram_mib,
            active_requests: 0,
            queued_requests: 0,
            owned: true,
            idle_secs: 0,
            grace_seconds: 0,
        }
    }

    /// Роль удерживается резидентно (`chat`).
    pub fn keep_loaded(mut self) -> Self {
        self.retention_mode = retention::KEEP_LOADED;
        self
    }

    pub fn with_state(mut self, state: i32) -> Self {
        self.state = state;
        self
    }

    pub fn with_idle(mut self, idle_secs: u64) -> Self {
        self.idle_secs = idle_secs;
        self
    }

    pub fn with_grace(mut self, grace_seconds: i32) -> Self {
        self.grace_seconds = grace_seconds;
        self
    }

    /// Инстанс занят запросом — вытеснять нельзя.
    pub fn busy(mut self, active: i32, queued: i32) -> Self {
        self.active_requests = active;
        self.queued_requests = queued;
        self
    }

    /// Инстанс создан не нами (чужой владелец) — не трогаем.
    pub fn foreign(mut self) -> Self {
        self.owned = false;
        self
    }

    pub fn is_loaded(&self) -> bool {
        state::is_loaded(self.state)
    }

    /// Инстанс держит VRAM: `LOADED`/`SERVING`/`GRACE` — в grace модель ещё в памяти
    /// (движок выгрузит её сам по grace-таймауту, внешний предохранитель — раньше).
    pub fn holds_vram(&self) -> bool {
        self.is_loaded() || self.state == state::GRACE
    }

    pub fn is_busy(&self) -> bool {
        self.active_requests > 0 || self.queued_requests > 0
    }

    pub fn is_on_demand(&self) -> bool {
        self.retention_mode == retention::LOAD_ON_DEMAND
    }

    /// Строка `retention` для `status`.
    pub fn retention_label(&self) -> &'static str {
        if self.is_on_demand() {
            "on_demand"
        } else {
            "keep"
        }
    }
}

/// Требование на VRAM: роль и оценка «модель + KV» (`budget::estimate_need_mib`).
#[derive(Debug, Clone)]
pub struct Demand {
    pub role: String,
    pub need_mib: u64,
}

impl Demand {
    pub fn new(role: &str, need_mib: u64) -> Demand {
        Demand {
            role: role.to_string(),
            need_mib,
        }
    }
}

/// Кандидаты на вытеснение: владелец — HDS, инстанс держит VRAM, не занят запросом
/// и это не роль, под которую просят память.
fn evictable<'a>(instances: &'a [InstanceUse], protect_role: &str) -> Vec<&'a InstanceUse> {
    instances
        .iter()
        .filter(|i| i.owned && i.holds_vram() && !i.is_busy() && i.role != protect_role)
        .collect()
}

/// Порядок вытеснения **для запроса**: сначала роли с меньшим приоритетом
/// (`gpu.priorities`: whisper 20 → rerank 30 → embedding 40 → chat 100),
/// при равном приоритете — дольше простаивающие, затем более крупные.
///
/// Роль, которой нет в `gpu.priorities`, получает приоритет 0 и выгружается
/// первой — новая роль не должна незаметно выдавливать чат.
pub fn eviction_order(
    gpu: &GpuConfig,
    instances: &[InstanceUse],
    protect_role: &str,
) -> Vec<InstanceUse> {
    let mut out = evictable(instances, protect_role);
    out.sort_by_key(|i| {
        (
            gpu.priority_of(&i.role),
            std::cmp::Reverse(i.idle_secs),
            std::cmp::Reverse(i.vram_mib),
        )
    });
    out.into_iter().cloned().collect()
}

/// Порядок вытеснения **для индексации**: сначала «резидентные» роли
/// (`KEEP_LOADED`, то есть чат: он вернётся при следующем запросе), затем
/// index-роли по возрастанию приоритета. Роль, под которую просят память
/// (её использует сама индексация), не вытесняется.
pub fn eviction_order_for_indexing(
    gpu: &GpuConfig,
    instances: &[InstanceUse],
    protect_role: &str,
) -> Vec<InstanceUse> {
    let mut out = evictable(instances, protect_role);
    out.sort_by_key(|i| {
        (
            i.is_on_demand() as u8,
            gpu.priority_of(&i.role),
            std::cmp::Reverse(i.idle_secs),
        )
    });
    out.into_iter().cloned().collect()
}

/// Точный отчёт о нехватке (вместо молчаливой деградации).
fn shortage_message(
    gpu: &GpuConfig,
    demand: &Demand,
    free_mib: u64,
    freed_mib: u64,
    total_mib: u64,
    extra: &str,
) -> String {
    let short = total_mib.saturating_sub(free_mib + freed_mib);
    format!(
        "не хватает VRAM для роли '{role}': нужно {need} МиБ (модель+KV) + резерв {reserve} = {total} МиБ, \
         свободно {free_mib} МиБ, вытеснение освободило {freed_mib} МиБ — недостаёт {short} МиБ. \
         Авто-деградации нет (`llm.model_policy: fixed`): смените модель в UI или уменьшите \
         llm.{role}.n_ctx / квант вручную{extra}",
        role = demand.role,
        need = demand.need_mib,
        reserve = gpu.reserve_mb,
        total = total_mib,
    )
}

/// Решение по запросу (`ask`/`search`/внешний агент): ARB-1, ARB-2, ARB-4.
pub fn plan_query(
    gpu: &GpuConfig,
    free_mib: Option<u64>,
    demand: &Demand,
    instances: &[InstanceUse],
) -> Plan {
    let free = gpu.effective_free_mib(free_mib);
    let mut plan = Plan::new(free, demand.need_mib, gpu.reserve_mb);
    let total = plan.total_mib();

    // (а) замера нет — не гадаем: движок сам решит (возможна ошибка нехватки памяти)
    let Some(free_u) = free else {
        plan.verdict = Verdict::Unknown;
        plan.actions.push(Action::EnsureLoaded {
            role: demand.role.clone(),
        });
        plan.notes.push(
            "нет замера свободной VRAM (ни NVML, ни движок): бюджет не проверяем, \
             вытеснение и пауза не выполняются"
                .to_string(),
        );
        return plan;
    };

    // (б) влезает сразу — ничего не трогаем (ARB-3: индексные роли не выгружаем «на всякий»)
    if free_u >= total {
        plan.verdict = Verdict::Fits;
        plan.actions.push(Action::EnsureLoaded {
            role: demand.role.clone(),
        });
        plan.notes.push(format!(
            "VRAM достаточно: нужно {total} МиБ (модель+KV {} + резерв {}), свободно {free_u} МиБ",
            demand.need_mib, gpu.reserve_mb
        ));
        return plan;
    }

    match gpu.policy {
        GpuPolicy::Manual => {
            let msg = shortage_message(
                gpu,
                demand,
                free_u,
                0,
                total,
                "; gpu.policy: manual — вытеснение и пауза выключены",
            );
            plan.verdict = Verdict::NotEnough {
                short_mib: total - free_u,
            };
            plan.actions.push(Action::ReportShortage { message: msg.clone() });
            plan.notes.push(msg);
            plan.notes
                .push("роль не загружаем: решение за оператором (gpu.policy: manual)".to_string());
        }
        GpuPolicy::IndexingPriority => {
            let msg = shortage_message(
                gpu,
                demand,
                free_u,
                0,
                total,
                "; gpu.policy: indexing_priority — индексные роли не вытесняем",
            );
            plan.verdict = Verdict::NotEnough {
                short_mib: total - free_u,
            };
            plan.actions.push(Action::ReportShortage { message: msg.clone() });
            plan.notes.push(msg);
            plan.notes.push(
                "запрос ждёт: сейчас приоритет у индексации (gpu.policy: indexing_priority)"
                    .to_string(),
            );
        }
        GpuPolicy::QueryPriority => {
            if gpu.pause_index_on_query {
                plan.actions.push(Action::PauseIndex {
                    reason: format!(
                        "запрос роли '{}': нужно {total} МиБ, свободно {free_u} МиБ",
                        demand.role
                    ),
                });
            } else {
                plan.notes.push(
                    "gpu.pause_index_on_query = false: индексацию не останавливаем — \
                     она продолжит занимать VRAM до вытеснения её ролей движком"
                        .to_string(),
                );
            }
            let mut freed = 0u64;
            for inst in eviction_order(gpu, instances, &demand.role) {
                if free_u + freed >= total {
                    break;
                }
                plan.actions.push(Action::Unload {
                    name: inst.name.clone(),
                    role: inst.role.clone(),
                    vram_mib: inst.vram_mib,
                    why: format!(
                        "приоритет {} ({}), простой {} с, retention {}",
                        gpu.priority_of(&inst.role),
                        inst.role,
                        inst.idle_secs,
                        inst.retention_label()
                    ),
                });
                freed += inst.vram_mib;
            }
            plan.freed_mib = freed;
            plan.actions.push(Action::EnsureLoaded {
                role: demand.role.clone(),
            });
            if free_u + freed >= total {
                plan.verdict = Verdict::FitsAfterEviction;
                plan.notes.push(format!(
                    "после вытеснения хватает: освободили {freed} МиБ, доступно {} МиБ",
                    free_u + freed
                ));
                plan.notes.push(
                    "после ответа пауза снимается, индексные роли вернутся сами \
                     (LOAD_ON_DEMAND) — ARB-2"
                        .to_string(),
                );
            } else {
                let msg = shortage_message(gpu, demand, free_u, freed, total, "");
                plan.verdict = Verdict::NotEnough {
                    short_mib: total - (free_u + freed),
                };
                plan.actions.push(Action::ReportShortage { message: msg.clone() });
                plan.notes.push(msg);
                plan.notes.push(
                    "вытеснено всё, что можно (занятые и чужие инстансы не трогаем)".to_string(),
                );
            }
        }
    }
    plan
}

/// Действие арбитра. Список действий — это и есть «решение» для лога/`status`.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Поставить `index.pause` (на время запроса).
    PauseIndex { reason: String },
    /// Снять поставленную нами паузу (запрос завершён).
    ResumeIndex { reason: String },
    /// Выгрузить инстанс (`unload_instance`) — освободить VRAM.
    Unload {
        name: String,
        role: String,
        vram_mib: u64,
        why: String,
    },
    /// Обеспечить загрузку роли (движок грузит `LOAD_ON_DEMAND` сам,
    /// `KEEP_LOADED` — явным `load_instance`).
    EnsureLoaded { role: String },
    /// Точный отчёт о нехватке — вместо молчаливой деградации.
    ReportShortage { message: String },
}

impl Action {
    /// Строка для лога/`hdsw llm-host status`.
    pub fn describe(&self) -> String {
        match self {
            Action::PauseIndex { reason } => format!("пауза индексации: {reason}"),
            Action::ResumeIndex { reason } => format!("снять паузу индексации: {reason}"),
            Action::Unload {
                name,
                role,
                vram_mib,
                why,
            } => format!("выгрузить '{name}' (роль {role}, ≈{vram_mib} МиБ): {why}"),
            Action::EnsureLoaded { role } => format!("обеспечить загрузку роли '{role}'"),
            Action::ReportShortage { message } => format!("отчёт о нехватке VRAM: {message}"),
        }
    }
}

/// Итог решения — по нему `llm-host`/UI понимают, обслуживать запрос или нет.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Свободной VRAM достаточно сразу.
    Fits,
    /// Сначала вытеснили индексные роли — теперь достаточно.
    FitsAfterEviction,
    /// Не хватает: в действиях есть [`Action::ReportShortage`] с точными цифрами.
    NotEnough { short_mib: u64 },
    /// Замера свободной VRAM нет — решаем без гарантий (но это видно в отчёте).
    Unknown,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Fits => "fits",
            Verdict::FitsAfterEviction => "fits_after_eviction",
            Verdict::NotEnough { .. } => "not_enough",
            Verdict::Unknown => "unknown",
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Verdict::Fits | Verdict::FitsAfterEviction)
    }
}

impl Default for Verdict {
    fn default() -> Self {
        Verdict::Unknown
    }
}

/// План диспетчера: что сделали/сделаем и почему.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// Свободная VRAM, которой мы распоряжались (после cap и вычетов).
    pub free_mib: Option<u64>,
    /// Оценка потребности роли «модель + KV».
    pub need_mib: u64,
    /// Резерв, который не выедаем (`gpu.reserve_mb`).
    pub reserve_mib: u64,
    /// Сколько освободило вытеснение.
    pub freed_mib: u64,
    pub actions: Vec<Action>,
    pub verdict: Verdict,
    /// Пояснения для лога/`status` (в т.ч. «почему ничего не делали»).
    pub notes: Vec<String>,
}

impl Plan {
    pub fn new(free_mib: Option<u64>, need_mib: u64, reserve_mib: u64) -> Plan {
        Plan {
            free_mib,
            need_mib,
            reserve_mib,
            ..Default::default()
        }
    }

    /// Потребность вместе с резервом.
    pub fn total_mib(&self) -> u64 {
        self.need_mib + self.reserve_mib
    }

    /// Текст отчёта о нехватке (если он есть в действиях).
    pub fn shortage(&self) -> Option<&str> {
        self.actions.iter().find_map(|a| match a {
            Action::ReportShortage { message } => Some(message.as_str()),
            _ => None,
        })
    }

    /// Человекочитаемый список действий и пояснений (лог `llm-host`).
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let free = self
            .free_mib
            .map(|f| format!("{f} МиБ"))
            .unwrap_or_else(|| "нет замера".to_string());
        out.push(format!(
            "вердикт {}: нужно {} МиБ (модель+KV {} + резерв {}), свободно {free}, освобождено {} МиБ",
            self.verdict.as_str(),
            self.total_mib(),
            self.need_mib,
            self.reserve_mib,
            self.freed_mib
        ));
        out.extend(self.actions.iter().map(|a| a.describe()));
        out.extend(self.notes.iter().cloned());
        out
    }
}

/// Решение по индексации: ARB-3 — при нехватке VRAM индексации разрешено выгрузить
/// «резидентные» роли (чат), индексные роли не трогаем (они и есть работа).
pub fn plan_indexing(
    gpu: &GpuConfig,
    free_mib: Option<u64>,
    demand: &Demand,
    instances: &[InstanceUse],
) -> Plan {
    let free = gpu.effective_free_mib(free_mib);
    let mut plan = Plan::new(free, demand.need_mib, gpu.reserve_mb);
    let total = plan.total_mib();

    let Some(free_u) = free else {
        plan.verdict = Verdict::Unknown;
        plan.actions.push(Action::EnsureLoaded {
            role: demand.role.clone(),
        });
        plan.notes.push(
            "нет замера свободной VRAM: бюджет индексации не проверяем (NVML недоступен)"
                .to_string(),
        );
        return plan;
    };

    if free_u >= total {
        plan.verdict = Verdict::Fits;
        plan.actions.push(Action::EnsureLoaded {
            role: demand.role.clone(),
        });
        return plan;
    }

    if gpu.policy == GpuPolicy::Manual {
        let msg = shortage_message(
            gpu,
            demand,
            free_u,
            0,
            total,
            "; gpu.policy: manual — выгружать ничего не разрешено",
        );
        plan.verdict = Verdict::NotEnough {
            short_mib: total - free_u,
        };
        plan.actions.push(Action::ReportShortage { message: msg.clone() });
        plan.notes.push(msg);
        return plan;
    }

    let mut freed = 0u64;
    for inst in eviction_order_for_indexing(gpu, instances, &demand.role) {
        if free_u + freed >= total {
            break;
        }
        plan.actions.push(Action::Unload {
            name: inst.name.clone(),
            role: inst.role.clone(),
            vram_mib: inst.vram_mib,
            why: format!(
                "индексации нужно {total} МиБ; выгружаем резидентно удерживаемую роль \
                 (retention {}), простой {} с — вернётся при следующем запросе",
                inst.retention_label(),
                inst.idle_secs
            ),
        });
        freed += inst.vram_mib;
    }
    plan.freed_mib = freed;
    plan.actions.push(Action::EnsureLoaded {
        role: demand.role.clone(),
    });
    if free_u + freed >= total {
        plan.verdict = Verdict::FitsAfterEviction;
        plan.notes.push(
            "выгруженные роли загрузятся сами при первом запросе (LOAD_ON_DEMAND) — ARB-3"
                .to_string(),
        );
    } else {
        let msg = shortage_message(gpu, demand, free_u, freed, total, "");
        plan.verdict = Verdict::NotEnough {
            short_mib: total - (free_u + freed),
        };
        plan.actions.push(Action::ReportShortage { message: msg.clone() });
        plan.notes.push(msg);
        plan.notes.push(
            "индексация продолжится без этой роли (или на CPU), паузу не ставим".to_string(),
        );
    }
    plan
}

/// Порог простоя роли: `min(grace роли, gpu.evict_idle_sec)`.
///
/// Grace — механизм движка (он выгружает сам), `gpu.evict_idle_sec` — внешний
/// предохранитель на случай, если grace ещё не сработал (§8.6.2).
fn idle_threshold(gpu: &GpuConfig, inst: &InstanceUse) -> u64 {
    let grace = if inst.grace_seconds > 0 && inst.is_on_demand() {
        inst.grace_seconds as u64
    } else {
        u64::MAX
    };
    grace.min(gpu.evict_idle_sec)
}

/// Внешний предохранитель `gpu.evict_idle_sec`: кого выгрузить по простою.
///
/// Роль чата (`KEEP_LOADED`) тоже попадает под него — `llm-host` единственный
/// владелец GPU, и §8.6.2 прямо разрешает выгрузить чат, чтобы продолжить
/// индексацию; при следующем запросе он загрузится сам.
pub fn idle_evictions(gpu: &GpuConfig, instances: &[InstanceUse]) -> Vec<Action> {
    if gpu.policy == GpuPolicy::Manual || gpu.evict_idle_sec == 0 {
        return Vec::new();
    }
    instances
        .iter()
        .filter(|i| i.owned && i.holds_vram() && !i.is_busy())
        .filter_map(|i| {
            let threshold = idle_threshold(gpu, i);
            if i.idle_secs < threshold {
                return None;
            }
            Some(Action::Unload {
                name: i.name.clone(),
                role: i.role.clone(),
                vram_mib: i.vram_mib,
                why: format!(
                    "простой {} с ≥ порога {} с (grace роли {} с, gpu.evict_idle_sec {} с)",
                    i.idle_secs, threshold, i.grace_seconds, gpu.evict_idle_sec
                ),
            })
        })
        .collect()
}

/// Снимок инстанса кластера для арбитра.
///
/// * `role` — роль, которой служит инстанс (в `llm-host` имя инстанса равно роли, §4 A2);
/// * `vram_mib` — оценка «модель + KV» (`budget::estimate_need_mib`);
/// * `idle_secs` — простой (ведёт `llm-host`; сам кластер `last_used` не отдаёт);
/// * `owned = true` — инстансы видны только своему процессу (факт A3), значит созданы нами.
pub fn instance_use(inst: &Instance, role: &str, vram_mib: u64, idle_secs: u64) -> InstanceUse {
    InstanceUse {
        name: inst.name.clone(),
        role: role.to_string(),
        state: inst.state,
        retention_mode: inst.retention_mode,
        vram_mib,
        active_requests: inst.active_request_count,
        queued_requests: inst.queued_request_count,
        owned: true,
        idle_secs,
        grace_seconds: inst.load_on_demand_grace_seconds,
    }
}

/// Выполнить действия плана на живом кластере и шлюзе паузы (A4 шаг 2 → A5/A6).
///
/// Ошибки отдельных действий **не** прерывают остальные: несработавшая роль не
/// должна ронять запрос (в Python-версии роли тоже стартовали независимо,
/// `SPIKES.md` §14.7). Возвращается журнал — по строке на действие.
pub fn apply(cluster: &Cluster, pause: &Arc<IndexPause>, plan: &Plan) -> Vec<String> {
    let mut log = Vec::new();
    for action in &plan.actions {
        match action {
            Action::PauseIndex { reason } => match pause.pause(reason) {
                Ok(true) => log.push(format!("index.pause поставлен: {reason}")),
                Ok(false) => log.push(format!(
                    "index.pause уже стоял (пауза пользователя) — переиспользуем, \
                     снимать не будем: {reason}"
                )),
                Err(e) => log.push(format!("не удалось поставить index.pause: {e}")),
            },
            Action::ResumeIndex { reason } => match pause.resume() {
                Ok(true) => log.push(format!("index.pause снят: {reason}")),
                Ok(false) => log.push(
                    "index.pause оставлен (пауза пользователя или другие запросы ещё идут)"
                        .to_string(),
                ),
                Err(e) => log.push(format!("не удалось снять index.pause: {e}")),
            },
            Action::Unload {
                name,
                role,
                vram_mib,
                ..
            } => match cluster.find_instance_by_name(name) {
                Ok(Some(id)) => {
                    let loaded = cluster
                        .instance_by_id(id)
                        .ok()
                        .flatten()
                        .map(|i| i.is_loaded())
                        .unwrap_or(false);
                    if !loaded {
                        log.push(format!("'{name}' уже выгружен — ничего не делаем"));
                        continue;
                    }
                    match cluster.unload(id) {
                        Ok(()) => log.push(format!(
                            "выгружен '{name}' (роль {role}) — ожидаем освобождение ≈{vram_mib} МиБ"
                        )),
                        Err(e) => log.push(format!("не удалось выгрузить '{name}': {e}")),
                    }
                }
                Ok(None) => log.push(format!("'{name}' не найден в кластере — выгружать нечего")),
                Err(e) => log.push(format!("список инстансов недоступен: {e}")),
            },
            Action::EnsureLoaded { role } => match cluster.find_instance_by_name(role) {
                Ok(Some(id)) => {
                    let loaded = cluster
                        .instance_by_id(id)
                        .ok()
                        .flatten()
                        .map(|i| i.is_loaded())
                        .unwrap_or(false);
                    if loaded {
                        log.push(format!("роль '{role}' уже загружена"));
                    } else {
                        match cluster.load(id) {
                            Ok(()) => log.push(format!(
                                "роль '{role}': load_instance отправлен (при LOAD_ON_DEMAND \
                                 достаточно самого запроса)"
                            )),
                            Err(e) => log.push(format!("роль '{role}': load_instance не удался: {e}")),
                        }
                    }
                }
                Ok(None) => log.push(format!(
                    "инстанс роли '{role}' не создан — создаёт llm-host при старте (§A6)"
                )),
                Err(e) => log.push(format!("список инстансов недоступен: {e}")),
            },
            Action::ReportShortage { message } => log.push(message.clone()),
        }
    }
    log
}
