//! Чтение `config.yaml` для `llm-host` (A2): маппинг
//! `llm_server.*` / `llm.*` / `gpu.*` → параметры инстансов.
//!
//! Правила совместимости (`PLAN_W2_LLM_HOST.md` §A2):
//! * старые ключи `llm_server.<role>.{port,model,ctx_per_slot,extra_args}`
//!   продолжают работать; `--ctx-size` в llama-server был `parallel * ctx_per_slot`,
//!   поэтому `n_ctx` считаем так же;
//! * из `extra_args` понимаем `-ngl/--n-gpu-layers`, `--batch-size`, `--ubatch-size`,
//!   `-t/--threads`; остальные флаги (например `--cache-type-k`) **не** молча
//!   игнорируются, а попадают в `warnings` для `hdsw check`;
//! * новые ключи `llm.<role>.{model,n_ctx,retention,grace_seconds}` и `gpu.*`
//!   имеют приоритет;
//! * `shared:<role>` резолвится в общий рантайм машины (`%LOCALAPPDATA%\llama-runtime`),
//!   а не в каталог проекта — иначе повторяется дефект `SPIKES.md` §14.7.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml::Value;

use crate::cluster::Device;
use crate::device::{accelerators, vendor_of};
use crate::error::{EngineError, Result};
use crate::ffi::{model_kind, retention};
use crate::vram::VramSource;

/// Роли, которые поднимает `llm-host` (порядок важен для логов/`status`).
pub const ROLES: [&str; 4] = ["chat", "embedding", "rerank", "whisper"];

/// Роли, существующие **ровно в одном экземпляре на машину** (§3.5): индексация
/// («один набор моделей индексации обрабатывает один файл одновременно»).
pub const SINGLE_INSTANCE_ROLES: [&str; 2] = ["embedding", "rerank"];

/// База имени с номером: `chat-2` → `Some("chat")`, `rerank-2` → `Some("rerank")`,
/// `embedding` → `None` (числа нет).
fn numbered_base(role: &str) -> Option<&str> {
    let (base, num) = role.rsplit_once('-')?;
    if base.is_empty() || num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(base)
}

/// Базовая роль имени: `chat-2`/`chat-3` → `chat`, `whisper-2` → `whisper`,
/// `embedding`/`rerank-2` → сами себя (M6 §3.5: реплики отличаются от базовой
/// роли только именем, портом и слотами — модель/контекст у них те же; реплики
/// допустимы только у чата и транскрибации).
pub fn role_base(role: &str) -> &str {
    for base in ["chat", "whisper"] {
        if role
            .strip_prefix(base)
            .and_then(|r| r.strip_prefix('-'))
            .is_some()
        {
            if let Some(nb) = numbered_base(role) {
                if nb == base {
                    return base;
                }
            }
        }
    }
    role
}

/// Номер реплики (`chat-2` → `Some(2)`, `chat`/`embedding`/`rerank-2` → `None`).
pub fn replica_index(role: &str) -> Option<u32> {
    let base = role_base(role);
    if base == role {
        return None;
    }
    role[base.len() + 1..].parse::<u32>().ok()
}

/// Роль-реплика (номерная роль с поддерживаемой базой: `chat-2`, `whisper-3`).
pub fn is_replica(role: &str) -> bool {
    replica_index(role).is_some()
}

/// Запрещённая реплика роли, существующей в единственном экземпляре (§3.5):
/// `embedding-2`, `rerank-3` и т.п. Возвращает базовую роль.
pub fn single_instance_violation(role: &str) -> Option<&'static str> {
    let base = numbered_base(role)?;
    SINGLE_INSTANCE_ROLES.iter().copied().find(|b| *b == base)
}

/// Псевдо-роль автотранскрибации (`PLAN_AUTO_TRANSCRIBE` §10 T1.4).
///
/// Своего инстанса движка у неё нет — демон `hds transcribe-watch` ходит в bridge
/// роли `whisper`. Но приоритет вытеснения у неё **свой**: выше индексации и всех
/// вспомогательных ролей, ниже поиска/чата (порядок ролей по ТЗ:
/// 1) поиск и чат → 2) автотранскрибация → 3) индексация).
pub const TRANSCRIBE_ROLE: &str = "transcribe";

/// Режим работы LLM (`llm_server.mode`, §8.7 плана).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Встроенный рантайм (движок) — цель W2.
    Embedded,
    /// Фасад совместимости поверх внешнего владельца.
    Facade,
    /// LLM выключен (только FTS) — аварийный режим.
    Off,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Embedded => "embedded",
            Mode::Facade => "facade",
            Mode::Off => "off",
        }
    }

    /// Разбор значения ключа (неизвестное ⇒ `Embedded`, как было по умолчанию).
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_lowercase().as_str() {
            "facade" => Mode::Facade,
            "off" | "none" | "disabled" => Mode::Off,
            _ => Mode::Embedded,
        }
    }
}

/// `gpu.policy` — как диспетчер реагирует на конкуренцию запроса и индексации
/// (§8.6.2 основного плана). По умолчанию запрос важнее индексации.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPolicy {
    /// Запрос важнее: пауза индексации + выгрузка индексных ролей по приоритетам.
    QueryPriority,
    /// Индексация важнее: индексные роли не трогаем, запрос получает отчёт о нехватке.
    IndexingPriority,
    /// Ничего не делаем автоматически — только отчёт (ручной режим).
    Manual,
}

impl GpuPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            GpuPolicy::QueryPriority => "query_priority",
            GpuPolicy::IndexingPriority => "indexing_priority",
            GpuPolicy::Manual => "manual",
        }
    }

    /// Разбор значения ключа (неизвестное ⇒ `QueryPriority`, как в §8.6.2).
    pub fn parse(s: &str) -> GpuPolicy {
        match s.trim().to_lowercase().as_str() {
            "indexing_priority" | "indexing-priority" | "index" => GpuPolicy::IndexingPriority,
            "manual" | "off" | "none" => GpuPolicy::Manual,
            _ => GpuPolicy::QueryPriority,
        }
    }
}

/// Раскладка одной карты (M6 §3.1): наш индекс + роли, которым она назначена.
///
/// Источник — `gpu.devices` в `config.yaml`:
///
/// ```yaml
/// gpu:
///   devices:
///     - { index: 1, roles: [chat] }
///     - { index: 2, roles: [embedding, rerank, whisper] }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDeviceAssignment {
    /// Наш индекс устройства: `0` — CPU, `1..N` — ускорители в порядке перечисления
    /// движка (то же семантика, что у `gpu.device_index`).
    pub index: i32,
    /// Роли, назначенные этой карте (имена из `ROLES`).
    pub roles: Vec<String>,
}

/// Пресет раскладки ролей по картам (M6 T4 §3.4) — вендор-гомогенные конфиги
/// из «коробки». Назначает **существующие** роли конфига на карты (реплики
/// `chat-N`/`whisper-N` по-прежнему объявляются явно, как в T2); явная
/// `gpu.devices` сильнее пресета, `llm_server.<role>.device` — сильнее всего.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPreset {
    /// Пресет не задан (компат: все роли на `gpu.device_index`).
    None,
    /// `NVIDIA: одна карта` — все роли на первую карту; масштаб — `parallel`
    /// и/или явная реплика `chat-2` (делят ту же карту).
    NvidiaOne,
    /// `NVIDIA: N карт` — чат-реплики и `whisper` раскладываются по картам
    /// (round-robin), индексация (`embedding`/`rerank`) — на выделенной.
    NvidiaN,
    /// `AMD: N карт` (Vulkan) — раскладка как у `nvidia-n`.
    AmdN,
    /// `Apple Silicon` (Metal) — всё на первую карту.
    Apple,
}

impl GpuPreset {
    /// Разбор `gpu.preset`; `None` — неизвестное значение (в `build` → warning).
    pub fn parse(s: &str) -> Option<GpuPreset> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "off" => Some(GpuPreset::None),
            "nvidia-one" | "nvidia-1" | "nvidia-one-card" => Some(GpuPreset::NvidiaOne),
            "nvidia-n" | "nvidia" | "nvidia-multi" => Some(GpuPreset::NvidiaN),
            "amd-n" | "amd" | "amd-multi" => Some(GpuPreset::AmdN),
            "apple" | "apple-silicon" | "metal" | "apple-metal" => Some(GpuPreset::Apple),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GpuPreset::None => "none",
            GpuPreset::NvidiaOne => "nvidia-one",
            GpuPreset::NvidiaN => "nvidia-n",
            GpuPreset::AmdN => "amd-n",
            GpuPreset::Apple => "apple",
        }
    }

    /// Вендор, которого требует пресет (`None` — не требует, `apple` — Metal).
    pub fn required_vendor(&self) -> Option<&'static str> {
        match self {
            GpuPreset::NvidiaOne | GpuPreset::NvidiaN => Some("nvidia"),
            GpuPreset::AmdN => Some("amd"),
            GpuPreset::Apple => Some("apple"),
            GpuPreset::None => None,
        }
    }

    /// Пресет по фактам об ускорителях (`gpu.autodetect: true`): только
    /// вендор-гомогенные наборы, иначе `None` (смешанные вендоры/UMA — §2).
    pub fn autodetect(facts: &AccelFacts) -> GpuPreset {
        let vendors = facts.unique_vendors();
        if facts.count == 0 || vendors.len() != 1 {
            return GpuPreset::None;
        }
        match vendors[0] {
            "nvidia" => {
                if facts.count >= 2 {
                    GpuPreset::NvidiaN
                } else {
                    GpuPreset::NvidiaOne
                }
            }
            "amd" => GpuPreset::AmdN,
            "apple" => GpuPreset::Apple,
            _ => GpuPreset::None,
        }
    }

    /// Раскладка пресета для **существующих** ролей конфига. Fail-fast с числами,
    /// если карт нет или вендор не совпал (§2/§3.5).
    pub fn layout(
        &self,
        facts: &AccelFacts,
        roles: &[String],
    ) -> std::result::Result<Vec<GpuDeviceAssignment>, String> {
        if *self == GpuPreset::None {
            return Ok(Vec::new());
        }
        if facts.count == 0 {
            return Err(format!(
                "пресет gpu.preset={}: ускорителей не найдено (только CPU) — раскладка \
                 невозможна; проверьте `llm-host devices`",
                self.as_str()
            ));
        }
        if let Some(want) = self.required_vendor() {
            let vendors = facts.unique_vendors();
            if !vendors.is_empty() && !vendors.contains(&want) {
                return Err(format!(
                    "пресет gpu.preset={} ожидает карты {want} — найдено: {}; смешанные \
                     вендоры/чужие карты не поддерживаются (§2)",
                    self.as_str(),
                    vendors.join(" + ")
                ));
            }
        }
        let n = facts.count as i32;
        let mut map: BTreeMap<i32, Vec<String>> = BTreeMap::new();
        if matches!(self, GpuPreset::NvidiaOne | GpuPreset::Apple) {
            // Одна карта: все роли на неё (масштаб — `parallel`/явная реплика).
            for r in roles {
                map.entry(1).or_default().push(r.clone());
            }
        } else {
            // N карт: chat-реплики и whisper — round-robin по картам; индексация
            // (embedding/rerank) — одним экземпляром на выделенной (последней)
            // карте («лёгкая добавка», §3.4); прочие — на первую.
            let mut chats: Vec<&String> = roles.iter().filter(|r| role_base(r) == "chat").collect();
            chats.sort_by_key(|r| replica_index(r).unwrap_or(0));
            for (i, r) in chats.iter().enumerate() {
                map.entry(1 + (i as i32 % n))
                    .or_default()
                    .push((*r).clone());
            }
            let mut whisps: Vec<&String> =
                roles.iter().filter(|r| role_base(r) == "whisper").collect();
            whisps.sort_by_key(|r| replica_index(r).unwrap_or(0));
            for (i, r) in whisps.iter().enumerate() {
                map.entry(1 + (i as i32 % n))
                    .or_default()
                    .push((*r).clone());
            }
            for r in roles {
                let base = role_base(r);
                if base == "chat" || base == "whisper" {
                    continue;
                }
                let card = if base == "embedding" || base == "rerank" {
                    n
                } else {
                    1
                };
                map.entry(card).or_default().push(r.clone());
            }
        }
        Ok(map
            .into_iter()
            .map(|(index, roles)| GpuDeviceAssignment { index, roles })
            .collect())
    }
}

/// Факты об ускорителях, нужные пресету/автодетекту (M6 T4 §3.4): число карт и
/// вендоры по ним. Собирается из устройств движка в реестре/хосте/`diag`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccelFacts {
    pub count: usize,
    /// Вендоры по картам (`nvidia`/`amd`/`intel`/`apple`/`unknown`).
    pub vendors: Vec<String>,
}

impl AccelFacts {
    /// Собрать факты из устройств движка (ускорители в порядке перечисления).
    pub fn from_devices(devices: &[Device]) -> AccelFacts {
        let acc = accelerators(devices);
        AccelFacts {
            count: acc.len(),
            vendors: acc
                .iter()
                .map(|d| vendor_of(&d.description_or_name()).to_string())
                .collect(),
        }
    }

    /// Уникальные вендоры (отсортированы) — для гомогенности (§2) и подсказок.
    pub fn unique_vendors(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.vendors.iter().map(|s| s.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// `gpu.*`: бюджет VRAM, устройство и приоритеты вытеснения (§A4).
#[derive(Debug, Clone)]
pub struct GpuConfig {
    /// Наш индекс устройства: `0` — CPU, `1` — первый GPU (§A2).
    pub device_index: i32,
    /// Раскладка ролей по картам (M6 §3.1, `gpu.devices`). Пустой список —
    /// раскладки нет, все роли идут на карту `device_index` (компат).
    pub devices: Vec<GpuDeviceAssignment>,
    /// Пресет раскладки (M6 T4 §3.4): `gpu.preset`. `None` — не задан. Пресет
    /// применяется, только если `devices` пуст (явная раскладка сильнее).
    pub preset: GpuPreset,
    /// Автодетект раскладки (M6 T4 §3.4): `gpu.autodetect`. По умолчанию выключен
    /// — раскладка только по явному ключу (`gpu.preset`/`gpu.devices`). При `true`
    /// и пустых `devices`/`preset` пресет подбирается по вендорам карт.
    pub autodetect: bool,
    /// Явно ли задан `gpu.n_gpu_layers` в конфиге (иначе уважаем legacy `-ngl`).
    pub n_gpu_layers_set: bool,
    pub n_gpu_layers: i32,
    pub reserve_mb: u64,
    pub vram_budget_mb: Option<u64>,
    pub evict_idle_sec: u64,
    /// Приоритеты вытеснения: chat 100, transcribe 60, embedding 40, rerank 30,
    /// whisper 20 (см. [`TRANSCRIBE_ROLE`]).
    pub priorities: BTreeMap<String, i32>,
    /// `gpu.policy` (A4 шаг 2): что делать при нехватке VRAM.
    pub policy: GpuPolicy,
    /// `gpu.pause_index_on_query` (A4 шаг 2): ставить `index.pause` на время запроса.
    pub pause_index_on_query: bool,
    /// `gpu.external_vram_mb`: вычет на чужих потребителей GPU (по умолчанию 0 —
    /// из бюджета не вычитаем; ключ-ручка для диагностики).
    pub external_vram_mb: u64,
    /// `gpu.vram_source`: источник истины по свободной VRAM (`nvml` по умолчанию).
    pub vram_source: VramSource,
    /// `gpu.abandoned_request_sec`: страж брошенных запросов. Если один вызов
    /// движка держится дольше порога (клиент оборвал запрос по своему таймауту,
    /// отмены в API движка нет), резидент завершается штатной уборкой — GPU
    /// освобождается. `0` — выключить; по умолчанию 900 с.
    pub abandoned_request_sec: u64,
}

impl Default for GpuConfig {
    fn default() -> Self {
        let mut priorities = BTreeMap::new();
        priorities.insert("chat".to_string(), 100);
        // P3 (§2.2 анонимизации): роль аренды GLiNER — приоритет chat;
        // инстанса в кластере нет, lease идёт через /internal/anon/enter|exit.
        priorities.insert(crate::dispatch::ANON_ROLE.to_string(), 100);
        priorities.insert(TRANSCRIBE_ROLE.to_string(), 60);
        priorities.insert("embedding".to_string(), 40);
        priorities.insert("rerank".to_string(), 30);
        priorities.insert("whisper".to_string(), 20);
        GpuConfig {
            device_index: 1, // первый GPU: без явного выбора движок может уйти на CPU (R32)
            devices: Vec::new(),
            preset: GpuPreset::None,
            autodetect: false,
            n_gpu_layers_set: false,
            n_gpu_layers: -1,
            reserve_mb: 1024,
            vram_budget_mb: None,
            evict_idle_sec: 600, // решение заказчика 29.09.2026
            priorities,
            policy: GpuPolicy::QueryPriority, // §8.6.2: запрос важнее индексации
            pause_index_on_query: true,
            external_vram_mb: 0,
            vram_source: VramSource::Nvml, // R29: только NVML — источник истины
            abandoned_request_sec: 900,    // страж брошенных запросов (15 мин)
        }
    }
}

impl GpuConfig {
    /// Свободная VRAM, которой реально можно распоряжаться (A4 шаг 2).
    ///
    /// * источник замера — NVML (или, при `gpu.vram_source: engine`, `list_devices` —
    ///   **недостоверно**, R29);
    /// * `gpu.vram_budget_mb`, если задан, **сужает** замер (ручка для проверки
    ///   критерия A-7: «искусственно сузили бюджет → появились точные цифры нехватки»);
    /// * `gpu.external_vram_mb` вычитается как чужая занятость GPU.
    ///
    /// `None` означает «замера нет» — диспетчер не гадает и сообщает об этом.
    pub fn effective_free_mib(&self, measured_free_mib: Option<u64>) -> Option<u64> {
        let free = measured_free_mib?;
        let free = match self.vram_budget_mb {
            Some(cap) => free.min(cap),
            None => free,
        };
        Some(free.saturating_sub(self.external_vram_mb))
    }

    /// Приоритет роли для вытеснения: чем меньше число, тем раньше она выгружается.
    /// Роль, которой нет в `gpu.priorities`, получает 0 (вытесняется первой) —
    /// так новая роль не сможет незаметно «выдавить» чат.
    pub fn priority_of(&self, role: &str) -> i32 {
        self.priorities.get(role).copied().unwrap_or(0)
    }

    /// Карта роли из раскладки `gpu.devices` (M6 §3.1).
    ///
    /// Возвращает индексы **всех** карт, на которых назначена роль (по порядку
    /// в конфиге); пусто — роль не назначена явно. Валидность («ровно одна
    /// карта на роль», §3.5) проверяет планировщик (`registry::plan_role`).
    pub fn role_assignments(&self, role: &str) -> Vec<i32> {
        self.devices
            .iter()
            .filter(|a| a.roles.iter().any(|r| r == role))
            .map(|a| a.index)
            .collect()
    }

    /// Эффективная раскладка ролей по картам (M6 T4 §3.4). Приоритет источников:
    /// явная `gpu.devices` → `gpu.preset` → автодетект (`gpu.autodetect: true`) →
    /// компат (пусто, все роли на `gpu.device_index`). `role_names` — роли,
    /// объявленные в конфиге (пресет раскладывает именно их). Ошибка — fail-fast
    /// с числами (нет карт / вендор пресета не найден).
    pub fn effective_layout(
        &self,
        facts: &AccelFacts,
        role_names: &[String],
    ) -> std::result::Result<Vec<GpuDeviceAssignment>, String> {
        // Явная раскладка сильнее пресета (§3.4).
        if !self.devices.is_empty() {
            return Ok(self.devices.clone());
        }
        let preset = if self.preset != GpuPreset::None {
            self.preset
        } else if self.autodetect {
            GpuPreset::autodetect(facts)
        } else {
            GpuPreset::None
        };
        if preset == GpuPreset::None {
            return Ok(Vec::new());
        }
        preset.layout(facts, role_names)
    }

    /// Строка о пресете/автодетекте для логов/`llm_host_plan` (M6 T4 §3.4).
    /// `None` — раскладка не задана пресетом (компат или явная `gpu.devices`).
    pub fn preset_note(&self, facts: &AccelFacts) -> Option<String> {
        let vendors = facts.unique_vendors();
        let vendor_txt = if vendors.is_empty() {
            "неизвестно".to_string()
        } else {
            vendors.join(" + ")
        };
        if !self.devices.is_empty() {
            return Some(format!(
                "раскладка gpu.devices: {} записей (пресет не применяется — явная раскладка \
                 сильнее пресета)",
                self.devices.len()
            ));
        }
        if self.preset != GpuPreset::None {
            return Some(format!(
                "пресет раскладки gpu.preset={} (ускорителей: {}, вендоры: {})",
                self.preset.as_str(),
                facts.count,
                vendor_txt
            ));
        }
        if self.autodetect {
            return Some(match GpuPreset::autodetect(facts) {
                GpuPreset::None => format!(
                    "gpu.autodetect=true, но пресет не подобран (ускорителей: {}, вендоры: {}; \
                     смешанные вендоры/UMA не поддерживаются — §2)",
                    facts.count, vendor_txt
                ),
                p => format!(
                    "gpu.autodetect=true → пресет {} (ускорителей: {}, вендоры: {})",
                    p.as_str(),
                    facts.count,
                    vendor_txt
                ),
            });
        }
        None
    }

    /// Строка о бюджете для `hdsw llm-host status` / логов (точные цифры).
    pub fn budget_note(&self) -> String {
        format!(
            "резерв {} МиБ, вычет на чужих {} МиБ, cap {}",
            self.reserve_mb,
            self.external_vram_mb,
            self.vram_budget_mb
                .map(|v| format!("{v} МиБ"))
                .unwrap_or_else(|| "не задан (берём NVML как есть)".to_string())
        )
    }
}

/// Параметры одной роли для `llm-host`.
#[derive(Debug, Clone)]
pub struct RoleConfig {
    pub role: String,
    /// Спецификатор модели из конфига (`shared:<role>` или путь).
    pub model_spec: String,
    pub port: u16,
    pub n_ctx: i32,
    /// `retention_mode`: chat — `KEEP_LOADED`, остальные — `LOAD_ON_DEMAND`.
    pub retention_mode: i32,
    pub grace_seconds: i32,
    pub model_kind: i32,
    pub embedding: bool,
    pub reranking: bool,
    pub n_batch: Option<i32>,
    pub n_ubatch: Option<i32>,
    pub n_threads: Option<i32>,
    /// Тип KV-кэша роли: `llm.<role>.cache_type_k/v` (имя «q8_0» или число) либо legacy
    /// `--cache-type-k/v` из `extra_args`. Работает нашим патчем движка: в стоковом
    /// cluster/bridge API таких полей нет, поэтому флаги раньше игнорировались.
    pub cache_type_k: Option<i32>,
    pub cache_type_v: Option<i32>,
    /// Legacy `-ngl` из `llm_server.<role>.extra_args` (используется, если
    /// `gpu.n_gpu_layers` не задан явно).
    pub legacy_n_gpu_layers: Option<i32>,
    /// Явная карта роли (M6 §3.1): `llm_server.<role>.device` — наш индекс
    /// (`0` = CPU, `1..N` = ускорители). `None` — из `gpu.devices[].roles`,
    /// иначе компат `gpu.device_index`.
    pub device: Option<i32>,
    /// Замечания по роли (идут в `warnings` и `hdsw check`).
    pub notes: Vec<String>,
}

/// Конфиг `llm-host`: всё, что нужно треку A (без чтения индекса/поиска).
#[derive(Debug, Clone)]
pub struct LlmHostConfig {
    pub path: PathBuf,
    pub host: String,
    pub parallel: i32,
    pub autostart: bool,
    pub start_timeout: u64,
    pub mode: Mode,
    /// `llm.model_policy`: `fixed` (без авто-деградации) по умолчанию.
    pub model_policy: String,
    pub gpu: GpuConfig,
    pub roles: Vec<RoleConfig>,
    /// Некритичные замечания (идут в `hdsw check`).
    pub warnings: Vec<String>,
    /// Нарушения контракта §3.5 (fail-fast с числами): повторное назначение
    /// индексной роли, дубликаты портов и т.п. Непустой список = конфиг
    /// запускать нельзя (`Host::start`/`llm_host_plan` отказываются с текстом).
    pub errors: Vec<String>,
}

impl LlmHostConfig {
    /// Роль по имени.
    pub fn role(&self, name: &str) -> Option<&RoleConfig> {
        self.roles.iter().find(|r| r.role == name)
    }

    /// Роли-реплики базовой роли (например все `chat*`): базовая — первой,
    /// затем `chat-2`, `chat-3`… по номеру (M6 §3.5/§3.6: одинаковые реплики,
    /// запросы раскидываются по ним внутри хоста).
    pub fn replicas_of(&self, base: &str) -> Vec<&RoleConfig> {
        let mut out: Vec<&RoleConfig> = self
            .roles
            .iter()
            .filter(|r| role_base(&r.role) == base)
            .collect();
        out.sort_by_key(|r| replica_index(&r.role).unwrap_or(0));
        out
    }

    /// Нарушения контракта (§3.5) в текстовом виде — для бинарей/логов.
    pub fn contract_note(&self) -> Option<String> {
        if self.errors.is_empty() {
            None
        } else {
            Some(format!(
                "конфиг нарушает правила раскладки ролей (§3.5) — {} ошибка(и):\n  {}",
                self.errors.len(),
                self.errors.join("\n  ")
            ))
        }
    }
}

/// Прочитать YAML-конфиг (BOM допускается, как в `hds/config.py`).
pub fn load(path: &Path) -> Result<LlmHostConfig> {
    let text = std::fs::read_to_string(path).map_err(|e| EngineError::Config {
        path: path.to_path_buf(),
        detail: format!("не читается: {e}"),
    })?;
    let root: Value = serde_yaml::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| {
        EngineError::Config {
            path: path.to_path_buf(),
            detail: format!("не разобран как YAML: {e}"),
        }
    })?;
    Ok(build(path, &root))
}

/// Достать значение по пути вида `index.roots` (порт `hds/config.py:dig`).
pub fn dig<'a>(root: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut cur = root;
    for part in dotted.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn dig_str(root: &Value, dotted: &str) -> Option<String> {
    dig(root, dotted)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn dig_i64(root: &Value, dotted: &str) -> Option<i64> {
    dig(root, dotted).and_then(|v| v.as_i64())
}

fn dig_bool(root: &Value, dotted: &str) -> Option<bool> {
    dig(root, dotted).and_then(|v| v.as_bool())
}

/// Собрать конфиг из уже разобранного YAML (тестируется без файловой системы).
pub fn build(path: &Path, root: &Value) -> LlmHostConfig {
    let mut warnings: Vec<String> = Vec::new();
    let mut gpu = GpuConfig::default();
    if let Some(v) = dig_i64(root, "gpu.device_index") {
        gpu.device_index = v as i32;
    }
    // M6 §3.1: раскладка ролей по картам (`gpu.devices: [{index, roles}]`).
    // Неизвестные имена ролей и кривые записи — не молча: предупреждение с номером
    // записи; битые записи пропускаются (остаток раскладки продолжает работать).
    if let Some(list) = dig(root, "gpu.devices").and_then(|v| v.as_sequence()) {
        for (i, item) in list.iter().enumerate() {
            let index = item.get("index").and_then(|v| v.as_i64());
            let roles: Vec<String> = item
                .get("roles")
                .and_then(|v| v.as_sequence())
                .map(|seq| {
                    seq.iter()
                        .filter_map(|r| r.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            match index {
                Some(ix) if ix >= 0 && !roles.is_empty() => {
                    for r in &roles {
                        if !ROLES.contains(&r.as_str()) {
                            warnings.push(format!(
                                "gpu.devices[{i}]: неизвестная роль «{r}» — ожидается одна из: {}",
                                ROLES.join(", ")
                            ));
                        }
                    }
                    gpu.devices.push(GpuDeviceAssignment {
                        index: ix as i32,
                        roles,
                    });
                }
                _ => warnings.push(format!(
                    "gpu.devices[{i}]: запись пропущена — нужны поля index (число ≥ 0) \
                     и непустой список roles"
                )),
            }
        }
        if !gpu.devices.is_empty() && gpu.device_index > 0 {
            warnings.push(format!(
                "заданы и gpu.devices, и gpu.device_index={}: раскладка приоритетна для \
                 назначенных ролей, остальные — на карте device_index (компат)",
                gpu.device_index
            ));
        }
    }
    // M6 T4 §3.4: пресеты раскладки и автодетект. Пресет применяется, только если
    // явная `gpu.devices` пуста (она сильнее); автодетект — только если пусты и
    // `devices`, и `preset`.
    if let Some(v) = dig_str(root, "gpu.preset") {
        match GpuPreset::parse(&v) {
            Some(p) => gpu.preset = p,
            None => warnings.push(format!(
                "gpu.preset: неизвестное значение «{v}» — ожидается одно из: none, \
                 nvidia-one, nvidia-n, amd-n, apple (пресет не применён)"
            )),
        }
    }
    if let Some(v) = dig_bool(root, "gpu.autodetect") {
        gpu.autodetect = v;
    }
    if let Some(v) = dig_i64(root, "gpu.n_gpu_layers") {
        gpu.n_gpu_layers = v as i32;
        gpu.n_gpu_layers_set = true;
    }
    if let Some(v) = dig_i64(root, "gpu.reserve_mb") {
        gpu.reserve_mb = v.max(0) as u64;
    }
    if let Some(v) = dig_i64(root, "gpu.vram_budget_mb") {
        gpu.vram_budget_mb = Some(v.max(0) as u64);
    }
    if let Some(v) = dig_i64(root, "gpu.evict_idle_sec") {
        gpu.evict_idle_sec = v.max(0) as u64;
    }
    if let Some(map) = dig(root, "gpu.priorities").and_then(|v| v.as_mapping()) {
        for (k, v) in map {
            if let (Some(name), Some(num)) = (k.as_str(), v.as_i64()) {
                gpu.priorities.insert(name.to_string(), num as i32);
            }
        }
    }
    // A4 шаг 2: политика диспетчера, пауза индексации, чужой вычет и источник VRAM.
    if let Some(v) = dig_str(root, "gpu.policy") {
        let parsed = GpuPolicy::parse(&v);
        if !["query_priority", "indexing_priority", "manual"]
            .contains(&v.trim().to_lowercase().as_str())
        {
            warnings.push(format!(
                "gpu.policy: неизвестное значение «{v}» — используем {}",
                parsed.as_str()
            ));
        }
        gpu.policy = parsed;
    }
    if let Some(v) = dig_bool(root, "gpu.pause_index_on_query") {
        gpu.pause_index_on_query = v;
    }
    if let Some(v) = dig_i64(root, "gpu.external_vram_mb") {
        gpu.external_vram_mb = v.max(0) as u64;
    }
    if let Some(v) = dig_i64(root, "gpu.abandoned_request_sec") {
        gpu.abandoned_request_sec = v.max(0) as u64;
    }
    if let Some(v) = dig_str(root, "gpu.vram_source") {
        match VramSource::parse(&v) {
            Some(src) => {
                if !src.is_trusted() {
                    // R29: `memory_free` движка врёт до +7,7 ГБ — предупреждаем явно
                    warnings.push(
                        "gpu.vram_source: engine — цифры `list_devices` недостоверны (R29); \
                         для бюджета VRAM используйте nvml"
                            .to_string(),
                    );
                }
                gpu.vram_source = src;
            }
            None => warnings.push(format!(
                "gpu.vram_source: неизвестное значение «{v}» — используем nvml (R29)"
            )),
        }
    }

    // Слоты чата (M4 §3.1): дефолт по профилю — `server` → 5, иначе (desktop/
    // без профиля) 1 — прежнее поведение. Явное `llm_server.parallel` всегда
    // сильнее дефолта. Профиль читаем из того же YAML (`profile: server`).
    let default_parallel = if dig_str(root, "profile").as_deref() == Some("server") {
        5
    } else {
        1
    };
    let parallel = dig_i64(root, "llm_server.parallel")
        .unwrap_or(default_parallel)
        .max(1) as i32;
    let mut roles: Vec<RoleConfig> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    // M5 §3.2 (PLAN_M5_EXTERNAL_CHAT.md): `chat.provider: remote` — llm-host
    // НЕ держит chat-роль в VRAM (RAG идёт во внешний API `chat.base_url`);
    // embedding/rerank поднимаются как раньше. Это и есть «рычаг SLA».
    let chat_remote = dig_str(root, "chat.provider")
        .map(|v| v.trim().eq_ignore_ascii_case("remote"))
        .unwrap_or(false);
    // M6 T2 (§3.5): имена ролей = базовые `ROLES` + явные реплики чата/транскрибации
    // из конфига (`llm.<role>.*`, `llm_server.<role>.*`, `gpu.devices[].roles`).
    // Реплики индексных ролей запрещены: индексация — ровно один экземпляр.
    for role in config_role_names(root, &gpu, &mut errors) {
        let role = role.as_str();
        if role == "chat" && chat_remote {
            warnings.push(
                "chat: provider remote — chat-роль в llm-host не поднимается (RAG ходит \
                 в chat.base_url); llm_server.chat.* игнорируется"
                    .to_string(),
            );
            continue;
        }
        if let Some(rc) = build_role(root, role, parallel, &gpu, &mut warnings) {
            roles.push(rc);
        }
    }
    // §3.5: уникальные порты ролей/реплик (иначе два фасада рвутся за один порт).
    check_unique_ports(&roles, &mut errors);

    LlmHostConfig {
        path: path.to_path_buf(),
        host: dig_str(root, "llm_server.host").unwrap_or_else(|| "127.0.0.1".to_string()),
        parallel,
        autostart: dig_bool(root, "llm_server.autostart").unwrap_or(true),
        start_timeout: dig_i64(root, "llm_server.start_timeout")
            .unwrap_or(300)
            .max(1) as u64,
        mode: Mode::parse(
            &dig_str(root, "llm_server.mode").unwrap_or_else(|| "embedded".to_string()),
        ),
        model_policy: dig_str(root, "llm.model_policy").unwrap_or_else(|| "fixed".to_string()),
        gpu,
        roles,
        warnings,
        errors,
    }
}

/// Имена ролей из конфига: базовые [`ROLES`] + явные реплики чата/транскрибации
/// (`llm.<role>.*`, `llm_server.<role>.*`, `gpu.devices[].roles`).
///
/// Реплики индексных ролей (`embedding-2`, `rerank-3`) — нарушение §3.5
/// («индексация — ровно один экземпляр на машину»): попадают в `errors`
/// (fail-fast с числами), роль не создаётся.
fn config_role_names(root: &Value, gpu: &GpuConfig, errors: &mut Vec<String>) -> Vec<String> {
    // P6.3: при `transcribe.engine: python` ASR делает Python-sidecar (GigaAM),
    // поэтому whisper-роль llm_host не планируется вовсе: ни базовой, ни
    // репликами из `llm.whisper*`/`gpu.devices[].roles`. Старые ключи
    // `index.whisper_*` остаются в конфиге для отката на `engine: legacy`.
    let python_transcribe = dig(root, "transcribe.engine")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().eq_ignore_ascii_case("python"))
        .unwrap_or(false);
    let mut extra: Vec<String> = Vec::new();
    for section in ["llm", "llm_server"] {
        if let Some(map) = dig(root, section).and_then(|v| v.as_mapping()) {
            for (k, _) in map {
                if let Some(name) = k.as_str() {
                    extra.push(name.trim().to_string());
                }
            }
        }
    }
    for a in &gpu.devices {
        extra.extend(a.roles.iter().cloned());
    }
    let mut replicas: Vec<String> = Vec::new();
    for name in extra {
        if name.is_empty() || ROLES.contains(&name.as_str()) || replicas.contains(&name) {
            continue;
        }
        if python_transcribe && role_base(&name) == "whisper" {
            continue;
        }
        if let Some(base) = single_instance_violation(&name) {
            errors.push(format!(
                "роль «{name}»: «{base}» существует ровно в одном экземпляре на машину \
                 (§3.5) — реплики индексных ролей не поддерживаются, уберите «{name}» \
                 (один набор моделей индексации обрабатывает один файл одновременно)"
            ));
            continue;
        }
        if is_replica(&name) {
            replicas.push(name);
        }
    }
    // Реплики — по базовой роли и номеру: `chat-2`, `chat-3`, `whisper-2`…
    replicas.sort_by_key(|n| (role_base(n).to_string(), replica_index(n).unwrap_or(0)));
    let mut names: Vec<String> = ROLES
        .iter()
        .filter(|r| !(python_transcribe && **r == "whisper"))
        .map(|s| s.to_string())
        .collect();
    names.extend(replicas);
    names
}

/// §3.5: порты ролей/реплик уникальны — иначе два фасада рвутся за один порт.
fn check_unique_ports(roles: &[RoleConfig], errors: &mut Vec<String>) {
    let mut seen: BTreeMap<u16, String> = BTreeMap::new();
    for rc in roles {
        if rc.port == 0 {
            continue; // роль без HTTP-порта (whisper-подобные)
        }
        if let Some(prev) = seen.insert(rc.port, rc.role.clone()) {
            errors.push(format!(
                "порт {} занят двумя ролями: «{prev}» и «{}» — репликам задайте явные \
                 порты (`llm.{}.port`); базовые роли остаются на 8010/8011/8012 (§3.5)",
                rc.port, rc.role, rc.role
            ));
        }
    }
}

/// Умолчания роли — из `hds/llama_server.py` (`_role_cfg` и `DEFAULTS`).
///
/// Для реплик (`chat-2`, `whisper-2`) берём умолчания **базовой** роли; порт
/// реплики чата — `8010+N` (§3.6: «порты реплик 8010+N — внутренние/
/// диагностические»), но при совпадении с базовой ролью задаётся явно в конфиге
/// (иначе `check_unique_ports` вернёт ошибку с числами).
fn role_defaults(role: &str) -> (u16, i32, &'static str) {
    match role_base(role) {
        "chat" => {
            let port = match replica_index(role) {
                Some(n) => (8010 + n).min(u16::MAX as u32) as u16,
                None => 8010,
            };
            (
                port,
                32768,
                "--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99",
            )
        }
        "embedding" => (8011, 8192, "--batch-size 8192 --ubatch-size 8192 -ngl 99"),
        "rerank" => (8012, 8192, "-ngl 0"),
        // whisper — не HTTP-роль: ASR-инстанс поднимается в W3 (порт 0)
        _ => (0, 0, ""),
    }
}

/// Параметры роли: новые ключи `llm.<role>.*` приоритетнее старых `llm_server.<role>.*`.
fn build_role(
    root: &Value,
    role: &str,
    parallel: i32,
    gpu: &GpuConfig,
    warnings: &mut Vec<String>,
) -> Option<RoleConfig> {
    let (def_port, def_ctx, def_extra) = role_defaults(role);
    let base = role_base(role);
    let mut notes: Vec<String> = Vec::new();

    // Модель: llm.<role>.model → llm_server.<role>.model → index.whisper_model_path
    let model_spec = dig_str(root, &format!("llm.{role}.model"))
        .or_else(|| dig_str(root, &format!("llm_server.{role}.model")))
        .or_else(|| {
            if base == "whisper" {
                dig_str(root, "index.whisper_model_path")
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            if base == "whisper" {
                String::new()
            } else {
                // дефолт W2: модель из общего рантайма, а не каталог проекта (§14.7);
                // у реплик модель та же, что у базовой роли (§3.5)
                format!("shared:{base}")
            }
        });
    if base == "whisper" && model_spec.is_empty() {
        return None; // ASR-инстанс появится в W3
    }

    let port = dig_i64(root, &format!("llm.{role}.port"))
        .or_else(|| dig_i64(root, &format!("llm_server.{role}.port")))
        .unwrap_or(def_port as i64) as u16;
    // M6 §3.1: явная карта роли (`llm.<role>.device` приоритетнее
    // `llm_server.<role>.device`, как у остальных ключей роли).
    let device = dig_i64(root, &format!("llm.{role}.device"))
        .or_else(|| dig_i64(root, &format!("llm_server.{role}.device")))
        .filter(|v| *v >= 0)
        .map(|v| v as i32);
    if device.is_none()
        && dig_i64(root, &format!("llm_server.{role}.device")).map(|v| v < 0) == Some(true)
    {
        let msg = format!(
            "{role}: llm_server.{role}.device = {v} (< 0) игнорируется — индексы \
             устройств: 0 = CPU, 1..N = ускорители (см. llm-host devices)",
            v = dig_i64(root, &format!("llm_server.{role}.device")).unwrap_or_default()
        );
        warnings.push(msg.clone());
        notes.push(msg);
    }
    let ctx_per_slot =
        dig_i64(root, &format!("llm_server.{role}.ctx_per_slot")).unwrap_or(def_ctx as i64);
    // как у llama-server: --ctx-size = parallel * ctx_per_slot
    let n_ctx = dig_i64(root, &format!("llm.{role}.n_ctx"))
        .unwrap_or(parallel as i64 * ctx_per_slot)
        .max(0) as i32;

    let extra = dig_str(root, &format!("llm_server.{role}.extra_args"))
        .unwrap_or_else(|| def_extra.to_string());
    let ea = parse_extra_args(&extra);
    if !ea.unknown.is_empty() {
        let msg = format!(
            "{role}: флаги extra_args не распознаны и игнорируются: {} — перенесите их \
             в llm.{role}.* / gpu.* (W2)",
            ea.unknown.join(", ")
        );
        warnings.push(msg.clone());
        notes.push(msg);
    }

    // F1 (M5 T0.1, корень выброса ~315 с): повторная загрузка роли on_demand
    // при занятой VRAM идёт через `llama_params_fit_impl` движка, который при
    // тесной памяти («свободно ~1900 МиБ,Projected 334») завершается за ~299 с
    // (замер f1_r1: fit chat 0,57 с → embedding 4,2 с → rerank 27,3 с →
    // embedding-перезагрузка 299 с). Если роль не выгружать, перезагрузок нет —
    // поэтому в server-профиле embedding/rerank по умолчанию держатся в VRAM
    // (`keep`, прогрев при старте, host.rs); desktop экономит VRAM как раньше.
    //
    // Смена политики по запросу владельца (30.10.2026): чат больше НЕ резидент.
    // По умолчанию в обоих профилях `load_on_demand`: модель чата не
    // предзагружается при старте llm-host и не удерживается принудительно —
    // движок выгружает её по grace-таймауту после простоя (по умолчанию 300 с),
    // а первый после простоя запрос платит за загрузку (секунды-десятки секунд).
    // Прежнее поведение возвращает явный `llm.chat.retention: keep_loaded`.
    let server = dig_str(root, "profile").as_deref() == Some("server");
    let default_retention = if server && (base == "embedding" || base == "rerank") {
        retention::KEEP_LOADED
    } else {
        retention::LOAD_ON_DEMAND
    };
    let retention_mode = match dig_str(root, &format!("llm.{role}.retention")).as_deref() {
        Some("keep_loaded") | Some("keep-loaded") | Some("keep") => retention::KEEP_LOADED,
        Some("load_on_demand") | Some("load-on-demand") | Some("ondemand") => {
            retention::LOAD_ON_DEMAND
        }
        Some(other) => {
            let msg = format!(
                "{role}: неизвестное llm.{role}.retention = «{other}» (ожидается \
                 keep_loaded|load_on_demand) — беру значение по умолчанию"
            );
            warnings.push(msg.clone());
            notes.push(msg);
            default_retention
        }
        None => default_retention,
    };
    let grace_seconds = dig_i64(root, &format!("llm.{role}.grace_seconds"))
        .unwrap_or(300)
        .max(0) as i32;

    let (kind, embedding, reranking) = match base {
        "embedding" => (model_kind::EMBEDDINGS, true, false),
        "rerank" => (model_kind::RERANK, false, true),
        "whisper" => (model_kind::WHISPER, false, false),
        _ => (model_kind::TEXT, false, false),
    };

    // legacy -ngl: уважаем, если gpu.n_gpu_layers не задан явно
    let legacy_n_gpu_layers = ea.n_gpu_layers;
    if let Some(n) = legacy_n_gpu_layers {
        if gpu.n_gpu_layers_set {
            if base != "whisper" {
                let msg = format!(
                    "{role}: -ngl {n} из extra_args игнорируется — задано \
                     gpu.n_gpu_layers = {}",
                    gpu.n_gpu_layers
                );
                warnings.push(msg.clone());
                notes.push(msg);
            }
        } else if base != "whisper" {
            notes.push(format!(
                "{role}: -ngl {n} взят из legacy extra_args; перенесите в gpu.n_gpu_layers"
            ));
        }
    }

    // `llm.<role>.*` приоритетнее legacy `extra_args`: у чата большой n_batch
    // (2048) даёт compute-буфер ≈1972 МиБ (замер §7.2), и его имеет смысл уменьшить
    // именно конфигом, а не строкой флагов llama-server.
    let n_batch = dig_i64(root, &format!("llm.{role}.n_batch"))
        .map(|v| v.max(0) as i32)
        .or(ea.n_batch);
    let n_ubatch = dig_i64(root, &format!("llm.{role}.n_ubatch"))
        .map(|v| v.max(0) as i32)
        .or(ea.n_ubatch);
    let n_threads = dig_i64(root, &format!("llm.{role}.n_threads"))
        .map(|v| v.max(0) as i32)
        .or(ea.n_threads);
    // KV-кэш роли: `llm.<role>.cache_type_k/v` (строка «q8_0» или число) приоритетнее
    // legacy `--cache-type-k/v`. Применяется нашим патчем движка.
    let cache_type_k = dig(root, &format!("llm.{role}.cache_type_k"))
        .and_then(|v| v.as_str())
        .and_then(parse_kv_type)
        .or_else(|| dig_i64(root, &format!("llm.{role}.cache_type_k")).map(|v| v.max(0) as i32))
        .or(ea.cache_type_k);
    let cache_type_v = dig(root, &format!("llm.{role}.cache_type_v"))
        .and_then(|v| v.as_str())
        .and_then(parse_kv_type)
        .or_else(|| dig_i64(root, &format!("llm.{role}.cache_type_v")).map(|v| v.max(0) as i32))
        .or(ea.cache_type_v);
    if let (Some(ub), Some(b)) = (n_ubatch, n_batch) {
        if ub > b {
            let msg = format!(
                "{role}: llm.{role}.n_ubatch ({ub}) больше n_batch ({b}) — движок урежет сам, \
                 но лучше задать n_ubatch <= n_batch"
            );
            warnings.push(msg.clone());
            notes.push(msg);
        }
    }

    Some(RoleConfig {
        role: role.to_string(),
        model_spec,
        port,
        n_ctx,
        retention_mode,
        grace_seconds,
        model_kind: kind,
        embedding,
        reranking,
        n_batch,
        n_ubatch,
        n_threads,
        cache_type_k,
        cache_type_v,
        legacy_n_gpu_layers,
        device,
        notes,
    })
}

/// Разобранные флаги legacy `extra_args`.
#[derive(Debug, Default, PartialEq)]
pub struct ExtraArgs {
    pub n_gpu_layers: Option<i32>,
    pub n_batch: Option<i32>,
    pub n_ubatch: Option<i32>,
    pub n_threads: Option<i32>,
    /// `--cache-type-k/v` (ggml_type): работает с нашим патчем движка.
    pub cache_type_k: Option<i32>,
    pub cache_type_v: Option<i32>,
    /// Нераспознанные флаги (с значениями) — для `hdsw check`.
    pub unknown: Vec<String>,
}

/// Тип KV-кэша по имени или числу (как у `llama-server --cache-type-k/v`).
///
/// Понимаем `f16` (1), `q4_0` (2), `q5_0` (6), `q8_0` (8), `bf16` (30) и просто
/// число (ggml_type). Неизвестное имя → `None`, тогда флаг попадёт в `unknown`
/// и честно останется в предупреждениях.
pub fn parse_kv_type(s: &str) -> Option<i32> {
    match s.trim().to_ascii_lowercase().as_str() {
        "f16" | "fp16" => Some(1),
        "q4_0" => Some(2),
        "q5_0" => Some(6),
        "q8_0" => Some(8),
        "bf16" => Some(30),
        "" => None,
        other => other.parse::<i32>().ok(),
    }
}

/// Разобрать строку `extra_args` (как её писал llama-server).
pub fn parse_extra_args(s: &str) -> ExtraArgs {
    let mut out = ExtraArgs::default();
    let tokens: Vec<&str> = s.split_whitespace().collect();
    let mut i = 0usize;
    while i < tokens.len() {
        let t = tokens[i];
        // значение флага — следующий токен, если он не начинается с '-'
        let value = |i: &mut usize| -> Option<String> {
            match tokens.get(*i + 1).copied() {
                Some(v) if !v.starts_with('-') => {
                    *i += 1;
                    Some(v.to_string())
                }
                _ => None,
            }
        };
        match t {
            "-ngl" | "--n-gpu-layers" | "--gpu-layers" => {
                out.n_gpu_layers = value(&mut i).and_then(|v| v.parse().ok());
            }
            "-b" | "--batch-size" => {
                out.n_batch = value(&mut i).and_then(|v| v.parse().ok());
            }
            "-ub" | "--ubatch-size" => {
                out.n_ubatch = value(&mut i).and_then(|v| v.parse().ok());
            }
            "-t" | "--threads" => {
                out.n_threads = value(&mut i).and_then(|v| v.parse().ok());
            }
            "--cache-type-k" | "-ctk" => {
                out.cache_type_k = value(&mut i).and_then(|v| parse_kv_type(&v));
            }
            "--cache-type-v" | "-ctv" => {
                out.cache_type_v = value(&mut i).and_then(|v| parse_kv_type(&v));
            }
            other if other.starts_with('-') => {
                let v = value(&mut i);
                out.unknown.push(match v {
                    Some(v) => format!("{other} {v}"),
                    None => other.to_string(),
                });
            }
            other => out.unknown.push(other.to_string()),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Страж брошенных запросов: ключ из YAML доезжает до `GpuConfig`, 0 = выключен.
    #[test]
    fn abandoned_request_sec_parsed() {
        let root: serde_yaml::Value =
            serde_yaml::from_str("gpu:\n  abandoned_request_sec: 300\n").unwrap();
        let cfg = build(Path::new("t.yaml"), &root);
        assert_eq!(cfg.gpu.abandoned_request_sec, 300);

        let root: serde_yaml::Value =
            serde_yaml::from_str("gpu:\n  abandoned_request_sec: 0\n").unwrap();
        let cfg = build(Path::new("t.yaml"), &root);
        assert_eq!(cfg.gpu.abandoned_request_sec, 0, "0 = явно выключен");

        // без ключа — дефолт 900 с (15 мин)
        let root: serde_yaml::Value = serde_yaml::from_str("gpu: {}\n").unwrap();
        let cfg = build(Path::new("t.yaml"), &root);
        assert_eq!(cfg.gpu.abandoned_request_sec, 900);
    }

    /// Legacy-строка из реального конфига: понятые флаги + список непонятых.
    #[test]
    fn python_engine_drops_whisper_role() {
        let root: serde_yaml::Value = serde_yaml::from_str(
            "transcribe:\n  engine: python\nllm:\n  whisper:\n    model: w.bin\n",
        )
        .unwrap();
        let cfg = build(Path::new("t.yaml"), &root);
        assert!(
            !cfg.roles.iter().any(|r| role_base(&r.role) == "whisper"),
            "{:?}",
            cfg.roles.iter().map(|r| &r.role).collect::<Vec<_>>()
        );
        assert!(cfg.role("chat").is_some());

        let root: serde_yaml::Value = serde_yaml::from_str(
            "transcribe:\n  engine: legacy\nllm:\n  whisper:\n    model: w.bin\n",
        )
        .unwrap();
        let cfg = build(Path::new("t.yaml"), &root);
        assert!(cfg.role("whisper").is_some(), "legacy keeps whisper");
    }

    #[test]
    fn extra_args_parses_known_flags() {
        let ea = parse_extra_args("--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99");
        assert_eq!(ea.n_gpu_layers, Some(99));
        // С нашим патчем движка эти флаги больше не «неизвестные»: они доезжают
        // до llama.cpp (q8_0 = GGML_TYPE_Q8_0 = 8).
        assert_eq!(ea.cache_type_k, Some(8));
        assert_eq!(ea.cache_type_v, Some(8));
        assert!(
            ea.unknown.is_empty(),
            "непознанных флагов быть не должно: {:?}",
            ea.unknown
        );

        let ea = parse_extra_args("--batch-size 8192 --ubatch-size 8192 -ngl 99");
        assert_eq!(ea.n_batch, Some(8192));
        assert_eq!(ea.n_ubatch, Some(8192));
        assert_eq!(ea.n_gpu_layers, Some(99));
        assert!(ea.unknown.is_empty());

        let ea = parse_extra_args("-ngl 0");
        assert_eq!(ea.n_gpu_layers, Some(0));
        assert!(ea.unknown.is_empty());

        let ea = parse_extra_args("-fa --jinja");
        assert_eq!(ea.n_gpu_layers, None);
        assert_eq!(ea.unknown, vec!["-fa".to_string(), "--jinja".to_string()]);
    }

    /// Маппинг конфига: `shared:<role>`, `n_ctx = parallel * ctx_per_slot`,
    /// retention, модель-виды и warning про нераспознанные флаги.
    #[test]
    fn config_maps_roles_like_python() {
        let yaml = r#"
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
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("test.yaml"), &root);

        assert_eq!(cfg.mode, Mode::Embedded);
        assert_eq!(cfg.model_policy, "fixed");
        assert_eq!(cfg.gpu.device_index, 1);
        assert_eq!(cfg.parallel, 1);
        assert_eq!(cfg.roles.len(), 3, "whisper без модели роль не планируем");

        let chat = cfg.role("chat").expect("chat");
        assert_eq!(chat.model_spec, "shared:chat");
        assert_eq!(chat.n_ctx, 32768);
        // Смена политики (30.10.2026): чат по умолчанию load_on_demand — не резидент.
        assert_eq!(chat.retention_mode, retention::LOAD_ON_DEMAND);
        assert_eq!(chat.model_kind, model_kind::TEXT);
        assert_eq!(chat.legacy_n_gpu_layers, Some(99));

        let emb = cfg.role("embedding").expect("embedding");
        assert_eq!(emb.model_kind, model_kind::EMBEDDINGS);
        assert!(emb.embedding && !emb.reranking);
        assert_eq!(emb.retention_mode, retention::LOAD_ON_DEMAND);
        assert_eq!(emb.n_batch, Some(8192));
        assert_eq!(emb.n_ubatch, Some(8192));

        let rr = cfg.role("rerank").expect("rerank");
        assert_eq!(rr.model_kind, model_kind::RERANK);
        assert!(rr.reranking && !rr.embedding);
        assert_eq!(
            rr.legacy_n_gpu_layers,
            Some(0),
            "реранкер по legacy — на CPU"
        );

        // KV-квант из legacy `extra_args` теперь применяется нашим патчем движка,
        // а не превращается в предупреждение «флаг не распознан».
        assert_eq!(chat.cache_type_k, Some(8), "q8_0 из extra_args");
        assert_eq!(
            chat.cache_type_v, None,
            "в этом конфиге задан только `--cache-type-k`"
        );
        assert!(
            !cfg.warnings.iter().any(|w| w.contains("--cache-type-k")),
            "предупреждения про KV-флаги быть не должно: {:?}",
            cfg.warnings
        );
    }

    /// M4 §3.1: слоты чата по профилю — `profile: server` без явного
    /// `llm_server.parallel` → 5; desktop/без профиля → 1; явное значение сильнее.
    #[test]
    fn parallel_defaults_by_profile() {
        let server: Value =
            serde_yaml::from_str("profile: server\nllm_server: {host: \"127.0.0.1\"}").unwrap();
        assert_eq!(build(Path::new("t.yaml"), &server).parallel, 5);
        let desktop: Value = serde_yaml::from_str("llm_server: {host: \"127.0.0.1\"}").unwrap();
        assert_eq!(build(Path::new("t.yaml"), &desktop).parallel, 1);
        let explicit: Value =
            serde_yaml::from_str("profile: server\nllm_server: {host: \"127.0.0.1\", parallel: 2}")
                .unwrap();
        assert_eq!(
            build(Path::new("t.yaml"), &explicit).parallel,
            2,
            "явное значение сильнее профильного дефолта"
        );
    }

    /// F1 (M5 T0.1): в server-профиле embedding/rerank по умолчанию `keep`
    /// (без перезагрузок нет и ~299-с fit при тесной VRAM); desktop —
    /// `load_on_demand` как раньше; явный ключ сильнее дефолта профиля.
    #[test]
    fn retention_defaults_by_profile() {
        let server: Value = serde_yaml::from_str(
            "profile: server\nllm_server: {host: \"127.0.0.1\"}\nembedding: {base_url: \"http://127.0.0.1:8011/v1\"}",
        )
        .unwrap();
        let cfg = build(Path::new("t.yaml"), &server);
        assert_eq!(
            cfg.role("embedding").unwrap().retention_mode,
            retention::KEEP_LOADED,
            "server: embedding держим в VRAM"
        );
        assert_eq!(
            cfg.role("rerank").unwrap().retention_mode,
            retention::KEEP_LOADED,
            "server: rerank держим в VRAM"
        );

        let desktop: Value = serde_yaml::from_str("llm_server: {host: \"127.0.0.1\"}").unwrap();
        let cfg = build(Path::new("t.yaml"), &desktop);
        assert_eq!(
            cfg.role("embedding").unwrap().retention_mode,
            retention::LOAD_ON_DEMAND,
            "desktop: экономия VRAM как раньше"
        );
        assert_eq!(
            cfg.role("rerank").unwrap().retention_mode,
            retention::LOAD_ON_DEMAND
        );

        // Явный ключ сильнее профильного дефолта.
        let explicit: Value = serde_yaml::from_str(
            "profile: server\nllm:\n  embedding:\n    retention: load_on_demand",
        )
        .unwrap();
        let cfg = build(Path::new("t.yaml"), &explicit);
        assert_eq!(
            cfg.role("embedding").unwrap().retention_mode,
            retention::LOAD_ON_DEMAND,
            "явный retention сильнее дефолта профиля"
        );
    }

    /// Имена/числа типов KV-кэша (`--cache-type-k/v`) → ggml_type.
    #[test]
    fn kv_type_names_map_to_ggml_ids() {
        assert_eq!(parse_kv_type("Q8_0"), Some(8), "регистр не важен");
        assert_eq!(parse_kv_type("q8_0"), Some(8));
        assert_eq!(parse_kv_type("f16"), Some(1));
        assert_eq!(parse_kv_type("bf16"), Some(30));
        assert_eq!(
            parse_kv_type("4"),
            Some(4),
            "числовой ggml_type проходит как есть"
        );
        assert_eq!(parse_kv_type("не-тип"), None);
        assert_eq!(parse_kv_type(""), None);
    }

    /// T1.4 (PLAN_AUTO_TRANSCRIBE): `transcribe` — выше индексации и index-ролей,
    /// ниже чата; порядок «1) поиск/чат → 2) автотранскрибация → 3) индексация».
    #[test]
    fn transcribe_priority_sits_between_chat_and_index_roles() {
        let gpu = GpuConfig::default();
        assert_eq!(gpu.priority_of(TRANSCRIBE_ROLE), 60);
        assert!(
            gpu.priority_of("chat") > gpu.priority_of(TRANSCRIBE_ROLE),
            "чат вытесняется позже автотранскрибации"
        );
        for index_role in ["embedding", "rerank", "whisper"] {
            assert!(
                gpu.priority_of(TRANSCRIBE_ROLE) > gpu.priority_of(index_role),
                "'{index_role}' должен вытесняться раньше автотранскрибации"
            );
        }
    }

    /// Роль вне `gpu.priorities` получает 0 — вытесняется первой (защита чата).
    #[test]
    fn unknown_role_priority_is_zero() {
        assert_eq!(GpuConfig::default().priority_of("не-роль"), 0);
    }

    /// M6 T2 (§3.5): чат-реплика `chat-2` — своя роль с портом/контекстом; модель
    /// наследуется от базовой роли (`shared:chat`), retention — как у чата (дефолт
    /// `load_on_demand`; явный `llm.chat-2.retention` тоже работает).
    #[test]
    fn chat_replica_is_parsed_like_base_role() {
        let yaml = r#"
llm:
  chat-2:
    port: 8030
llm_server:
  parallel: 5
  chat: { port: 8010 }
"#;
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("t.yaml"), &root);
        assert!(
            cfg.errors.is_empty(),
            "реплика чата легальна: {:?}",
            cfg.errors
        );
        let chat2 = cfg.role("chat-2").expect("реплика разобрана");
        assert_eq!(chat2.port, 8030, "порт реплики — из конфига");
        assert_eq!(
            chat2.model_spec, "shared:chat",
            "модель — как у базовой роли"
        );
        assert_eq!(chat2.model_kind, model_kind::TEXT);
        assert_eq!(
            chat2.retention_mode,
            retention::LOAD_ON_DEMAND,
            "чат-реплика наследует новый дефолт load_on_demand"
        );
        assert_eq!(cfg.replicas_of("chat").len(), 2, "chat + chat-2");
    }

    /// M6 T2 (§3.5): реплика индексной роли — нарушение контракта с числами
    /// (индексация существует ровно в одном экземпляре на машину).
    #[test]
    fn index_role_replica_is_contract_error() {
        let root: Value =
            serde_yaml::from_str("llm:\n  rerank-2: { model: \"shared:rerank\" }\n").expect("yaml");
        let cfg = build(Path::new("t.yaml"), &root);
        let note = cfg.contract_note().expect("ошибка контракта");
        assert!(note.contains("rerank-2"), "{note}");
        assert!(note.contains("§3.5"), "{note}");
        assert!(note.contains("одном экземпляре"), "{note}");
        assert!(cfg.role("rerank-2").is_none(), "роль не создаётся");
    }

    /// M6 T2 (§3.5): порты ролей уникальны — дубль даёт ошибку с числами.
    #[test]
    fn duplicate_ports_are_contract_error() {
        let root: Value = serde_yaml::from_str(
            "llm:\n  chat-2: { port: 8012 }\nllm_server:\n  chat: { port: 8010 }\n",
        )
        .expect("yaml");
        let cfg = build(Path::new("t.yaml"), &root);
        let note = cfg.contract_note().expect("ошибка контракта");
        assert!(note.contains("8012"), "число порта в сообщении: {note}");
        assert!(note.contains("chat-2"), "{note}");
        assert!(note.contains("rerank"), "обе роли названы: {note}");
    }

    /// M6 T2 (§3.5): `role_base`/`replica_index` разбирают имена реплик, а
    /// индексные роли реплик не имеют.
    #[test]
    fn replica_name_helpers() {
        assert_eq!(role_base("chat"), "chat");
        assert_eq!(role_base("chat-2"), "chat");
        assert_eq!(role_base("whisper-10"), "whisper");
        assert_eq!(role_base("embedding"), "embedding");
        assert_eq!(replica_index("chat-2"), Some(2));
        assert_eq!(replica_index("chat"), None);
        assert!(!is_replica("embedding"));
        assert_eq!(single_instance_violation("embedding-2"), Some("embedding"));
        assert_eq!(single_instance_violation("chat-2"), None);
    }

    /// Новые ключи `llm.<role>.{n_batch,n_ubatch,n_threads}` приоритетнее legacy
    /// `extra_args` (и предупреждают, если `n_ubatch > n_batch`).
    #[test]
    fn new_keys_override_legacy_batch() {
        let yaml = r#"
index:
  whisper_model_path: ""
llm_server:
  host: "127.0.0.1"
  parallel: 1
  chat:
    port: 8010
    model: "shared:chat"
    ctx_per_slot: 32768
    extra_args: "--batch-size 4096 --ubatch-size 4096 -ngl 99"
llm:
  chat:
    n_batch: 512
    n_ubatch: 1024
    n_threads: 4
"#;
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("test.yaml"), &root);
        let chat = cfg.role("chat").expect("chat");
        assert_eq!(
            chat.n_batch,
            Some(512),
            "llm.chat.n_batch перебивает --batch-size"
        );
        assert_eq!(chat.n_ubatch, Some(1024));
        assert_eq!(chat.n_threads, Some(4));
        assert!(
            cfg.warnings.iter().any(|w| w.contains("n_ubatch")),
            "n_ubatch > n_batch — предупреждаем: {:?}",
            cfg.warnings
        );
    }

    /// T1: `gpu.devices` разбирается, неизвестные роли и кривые записи — warn
    /// (не молча), остальная раскладка продолжает работать.
    #[test]
    fn gpu_devices_layout_parsed_with_warnings() {
        let yaml = r#"
index:
  whisper_model_path: ""
gpu:
  device_index: 2
  devices:
    - { index: 1, roles: [chat] }
    - { index: 2, roles: [embedding, rerank] }
    - { index: 3, roles: [unknown_role] }
    - { index: бяка, roles: [] }
    - { index: 4 }
llm_server:
  chat: { port: 8010, model: "shared:chat" }
"#;
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("t.yaml"), &root);
        assert_eq!(
            cfg.gpu.role_assignments("chat"),
            vec![1],
            "карта chat из раскладки"
        );
        assert_eq!(
            cfg.gpu.role_assignments("embedding"),
            vec![2],
            "карта embedding из раскладки"
        );
        assert_eq!(
            cfg.gpu.role_assignments("rerank"),
            vec![2],
            "embedding/rerank легально делят одну карту (§3.5: «лёгкая добавка»)"
        );
        assert!(cfg.gpu.role_assignments("whisper").is_empty());
        assert!(
            cfg.warnings.iter().any(|w| w.contains("unknown_role")),
            "неизвестная роль — предупреждение: {:?}",
            cfg.warnings
        );
        assert!(
            cfg.warnings.iter().any(|w| w.contains("gpu.devices[3]")),
            "кривые записи — предупреждение: {:?}",
            cfg.warnings
        );
        assert!(
            cfg.warnings
                .iter()
                .any(|w| w.contains("gpu.device_index=2")),
            "смешение стилей (devices + device_index) — предупреждение"
        );
    }

    /// T1: `llm_server.<role>.device` парсится; `llm.<role>.device` сильнее;
    /// отрицательный индекс игнорируется с предупреждением.
    #[test]
    fn role_device_key_parsed() {
        let yaml = r#"
index:
  whisper_model_path: ""
llm_server:
  chat:
    port: 8010
    model: "shared:chat"
    device: 2
  rerank:
    port: 8012
    device: -5
llm:
  chat:
    device: 3
"#;
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("t.yaml"), &root);
        assert_eq!(
            cfg.role("chat").unwrap().device,
            Some(3),
            "llm.<role>.device приоритетнее llm_server.<role>.device"
        );
        assert_eq!(
            cfg.role("rerank").unwrap().device,
            None,
            "отрицательный индекс игнорируется"
        );
        assert!(
            cfg.warnings
                .iter()
                .any(|w| w.contains("rerank") && w.contains("-5")),
            "отрицательный device — предупреждение: {:?}",
            cfg.warnings
        );
    }

    /// M6 T4 §3.4: пресет раскладывает существующие роли по картам; явная
    /// `gpu.devices` сильнее; без пресета/автодетекта ничего не меняется.
    #[test]
    fn gpu_preset_layout_and_override() {
        let yaml = r#"
index:
  whisper_model_path: ""
gpu:
  preset: nvidia-n
llm_server:
  parallel: 1
  chat:    { port: 8010, model: "shared:chat" }
  chat-2:  { port: 8030, model: "shared:chat" }
  embedding: { port: 8011, model: "shared:embedding" }
  rerank:  { port: 8012, model: "shared:rerank" }
llm:
  whisper:   { model: "shared:whisper" }
  whisper-2: { model: "shared:whisper" }
"#;
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("t.yaml"), &root);
        assert_eq!(cfg.gpu.preset, GpuPreset::NvidiaN);
        let names: Vec<String> = cfg.roles.iter().map(|r| r.role.clone()).collect();
        let facts = AccelFacts {
            count: 2,
            vendors: vec!["nvidia".into(), "nvidia".into()],
        };
        let layout = cfg.gpu.effective_layout(&facts, &names).expect("layout");
        let card_of = |role: &str| {
            layout
                .iter()
                .find(|a| a.roles.iter().any(|r| r == role))
                .map(|a| a.index)
        };
        assert_eq!(card_of("chat"), Some(1), "чат-реплики — round-robin");
        assert_eq!(card_of("chat-2"), Some(2));
        assert_eq!(card_of("whisper"), Some(1), "whisper — по картам");
        assert_eq!(card_of("whisper-2"), Some(2));
        assert_eq!(
            card_of("embedding"),
            Some(2),
            "индексация — выделенная карта"
        );
        assert_eq!(card_of("rerank"), Some(2));

        // Пресет nvidia-n на AMD-картах — fail-fast с числами (§2).
        let amd = AccelFacts {
            count: 1,
            vendors: vec!["amd".into()],
        };
        let err = cfg.gpu.effective_layout(&amd, &names).expect_err("AMD");
        assert!(err.contains("nvidia"), "err={err}");
        assert!(err.contains("amd"), "err={err}");

        // Явная `gpu.devices` сильнее пресета (§3.4).
        let yaml2 = r#"
index:
  whisper_model_path: ""
gpu:
  preset: nvidia-n
  devices:
    - { index: 1, roles: [chat] }
llm_server:
  chat:   { port: 8010, model: "shared:chat" }
  chat-2: { port: 8030, model: "shared:chat" }
"#;
        let root2: Value = serde_yaml::from_str(yaml2).expect("yaml2");
        let cfg2 = build(Path::new("t.yaml"), &root2);
        let names2: Vec<String> = cfg2.roles.iter().map(|r| r.role.clone()).collect();
        let layout2 = cfg2.gpu.effective_layout(&facts, &names2).expect("layout2");
        assert_eq!(layout2.len(), 1, "взята явная раскладка: {layout2:?}");
        assert_eq!(layout2[0].index, 1);
        assert_eq!(layout2[0].roles, vec!["chat".to_string()]);
    }

    /// M6 T4 §3.4: `gpu.autodetect` по умолчанию выключен (ничего не меняется);
    /// при `true` пресет подбирается по вендорам/числу карт.
    #[test]
    fn gpu_autodetect_off_by_default() {
        assert_eq!(GpuPreset::parse("nvidia-n"), Some(GpuPreset::NvidiaN));
        assert_eq!(GpuPreset::parse("amd"), Some(GpuPreset::AmdN));
        assert_eq!(GpuPreset::parse("metal"), Some(GpuPreset::Apple));
        assert_eq!(GpuPreset::parse("бяка"), None);

        let yaml = "index:\n  whisper_model_path: \"\"\nllm_server:\n  chat: { port: 8010, model: \"shared:chat\" }\n";
        let root: Value = serde_yaml::from_str(yaml).expect("yaml");
        let cfg = build(Path::new("t.yaml"), &root);
        let names: Vec<String> = cfg.roles.iter().map(|r| r.role.clone()).collect();
        let facts = AccelFacts {
            count: 2,
            vendors: vec!["nvidia".into(), "nvidia".into()],
        };
        assert!(!cfg.gpu.autodetect, "автодетект по умолчанию выключен");
        assert!(
            cfg.gpu.effective_layout(&facts, &names).unwrap().is_empty(),
            "без пресета/автодетекта раскладка пуста (компат)"
        );

        let yaml2 = "gpu:\n  autodetect: true\nindex:\n  whisper_model_path: \"\"\nllm_server:\n  chat: { port: 8010, model: \"shared:chat\" }\n";
        let root2: Value = serde_yaml::from_str(yaml2).expect("yaml2");
        let cfg2 = build(Path::new("t.yaml"), &root2);
        let names2: Vec<String> = cfg2.roles.iter().map(|r| r.role.clone()).collect();
        let layout = cfg2
            .gpu
            .effective_layout(&facts, &names2)
            .expect("autodetect layout");
        let card_of = |role: &str| {
            layout
                .iter()
                .find(|a| a.roles.iter().any(|r| r == role))
                .map(|a| a.index)
        };
        assert_eq!(
            card_of("chat"),
            Some(1),
            "две NVIDIA → nvidia-n: {layout:?}"
        );
        assert_eq!(card_of("embedding"), Some(2), "индексация — карта 2");

        // Одна NVIDIA → nvidia-one (одна запись на карту 1).
        let one = AccelFacts {
            count: 1,
            vendors: vec!["nvidia".into()],
        };
        assert_eq!(GpuPreset::autodetect(&one), GpuPreset::NvidiaOne);
        assert_eq!(GpuPreset::autodetect(&facts), GpuPreset::NvidiaN);
    }
}
