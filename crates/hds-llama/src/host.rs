//! A6 — резидентный `llm-host`: единственный владелец GPU + фасад `:8010–8012`.
//!
//! До A6 всё, что умеет хост, жило в бинаре `bin/llm_host_facade.rs` (прогон с
//! `--hold SEC`): создавал инстансы по конфигу (A2), обслуживал фасад (A5) и
//! применял решения диспетчера VRAM (A4). A6 добавляет то, чего не хватало для
//! боевого режима:
//!
//! * **резидентность** — `Host::start` занимает `data/llm-host.pid` (защита от
//!   второго владельца GPU), пишет `data/logs/llm-host.log`, а `Host::stop`
//!   снимает инстансы, отпускает **свою** паузу индексации и освобождает pid;
//! * **управление по HTTP** — внутренние маршруты `/internal/*` фасада
//!   (`status`/`load`/`unload`/`devices`/`stop`): CLI `llm-host` работает через
//!   них, поэтому `cargo run` рядом с резидентом не нужен, а кросс-процессная
//!   адресация инстансов (её нет — замер A3) и не требуется;
//! * **режим `llm_server.mode`** — `embedded` (свой кластер), `facade` (без
//!   инстансов: проксирование на внешний OpenAI-совместимый сервер) и `off`
//!   (LLM выключен, поиск только по FTS).
//!
//! Разделение намеренное: решения (что вытеснить, что влезает) живут в
//! `dispatch`/`budget`/`registry` и проверяются тестами без GPU, а здесь —
//! сборка живой машины и её уборка. Бины (`llm_host`, `llm_host_facade`,
//! `llm_host_status`) остаются тонкими: разбор argv + вызов этого модуля.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use hds_diarize::{
    decode_to_wav, leading_silence_samples, read_wav, render_md, write_wav, Diarizer,
};

use crate::asr_filter::{drop_silent_segments, FilterOutcome};
use crate::budget::{compute_buffer_mib, estimate_need_mib};
use crate::cluster::{Cluster, InstanceSpec};
use crate::config::{self, GpuConfig, LlmHostConfig, Mode};
use crate::device::describe_devices;
use crate::diar::{self, Backend as DiarBackend};
use crate::dispatch::{
    self, instance_use, plan_query, plan_transcribe, Demand, InstanceUse, Plan, Verdict,
};
use crate::error::{EngineError, Result};
use crate::facade::{self, Backend, ChatRequest, ServerConfig, Thinking, Usage};
use crate::gguf::{read_meta, KvBits};
use crate::pause::{read_heartbeat, IndexPause, PauseLease};
use crate::registry::{self, RolePlan};
use crate::resident::{self, Log, PidFile};
use crate::runtime::RuntimePaths;
use crate::status::{device_line, StatusInput, StatusReport};
use crate::vram::{open_vram_probe, VramProbe};
use crate::whisper::{language_from_body, DiarizationParams};
use crate::{engine::EngineCwd, Engine};

/// Корень репозитория (относительно крейта: `crates/hds-llama/../..`).
///
/// Нужен для путей по умолчанию: `config.yaml`, `index.pause`, `data/`.
///
/// Порядок — как в `hds_core::config::project_root` (важно для **поставки**):
/// `HDS_ROOT` → по расположению исполняемого файла → build-time
/// `CARGO_MANIFEST_DIR/../..` (dev-фолбэк). Раньше корень был **только**
/// build-time (путь машины сборки; у CI-сборки — `D:\a\<repo>\<repo>`):
/// распакованный на другой машине релизный `llm_host.exe` падал на старте
/// с «конфиг …\config.yaml: не читается (os error 2)» ещё **до** записи лога.
pub fn repo_root() -> PathBuf {
    if let Some(v) = std::env::var_os("HDS_ROOT") {
        if !v.is_empty() {
            return clean_path(Path::new(&v));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(r) = root_from_exe(dir) {
                return clean_path(&r);
            }
        }
    }
    clean_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."))
}

/// Системный каталог состояния (server-профиль, M2 T2) — зеркально
/// `hds_core::config::system_state_dir` (hds-llama от hds-core не зависит):
/// Windows — `%ProgramData%\hermes-disk-search`, macOS —
/// `/Library/Application Support/hermes-disk-search`.
pub fn system_state_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("ProgramData")
            .filter(|v| !v.is_empty())
            .map(|v| PathBuf::from(v).join("hermes-disk-search"))
    }
    #[cfg(target_os = "macos")]
    {
        Some(
            PathBuf::from("/Library")
                .join("Application Support")
                .join("hermes-disk-search"),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        None
    }
}

/// data-корень файлов состояния резидента (pid/лог/heartbeat, M2 T2):
/// `HDS_ROOT` переопределяет всё (M3 T0 F3 — зеркально
/// `hds_core::config::state_dir`: без этого server-профиль при заданном
/// `HDS_ROOT` всё равно писал состояние в системный каталог), далее
/// `profile: server` в конфиге → системный каталог состояния, иначе корень
/// проекта (прежнее поведение). Профиль читается по пути конфига.
pub fn resident_data_root(config_path: &Path) -> PathBuf {
    if let Some(v) = std::env::var_os("HDS_ROOT") {
        if !v.is_empty() {
            return clean_path(Path::new(&v));
        }
    }
    let is_server = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|t| {
            serde_yaml::from_str::<serde_yaml::Value>(t.trim_start_matches('\u{feff}')).ok()
        })
        .and_then(|v| {
            v.get("profile")
                .and_then(|p| p.as_str())
                .map(|s| s == "server")
        })
        .unwrap_or(false);
    if is_server {
        if let Some(d) = system_state_dir() {
            return d;
        }
    }
    repo_root()
}

/// Корень проекта по каталогу исполняемого файла (чистая функция, зеркально
/// `hds_core::config::root_from_exe`): `<root>\bin\x.exe` → `<root>`;
/// `target\{debug,release}[\deps]\x.exe` → корень репозитория; иначе — каталог exe.
fn root_from_exe(dir: &Path) -> Option<PathBuf> {
    let mut dir = dir.to_path_buf();
    // `target\<profile>\deps\x.exe` → `target\<profile>`
    if dir.file_name().map(|n| n == "deps").unwrap_or(false) {
        dir = dir.parent()?.to_path_buf();
    }
    // `<root>\bin\x.exe` → `<root>`
    if dir.file_name().map(|n| n == "bin").unwrap_or(false) {
        return dir.parent().map(|p| p.to_path_buf());
    }
    // `target\{debug,release}\x.exe` → корень репозитория
    let profile = matches!(
        dir.file_name().and_then(|n| n.to_str()),
        Some("debug") | Some("release")
    );
    if profile
        && dir
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n == "target")
            .unwrap_or(false)
    {
        return dir.parent()?.parent().map(|p| p.to_path_buf());
    }
    Some(dir)
}

/// Абсолютный путь **до** переключения cwd на каталог движка (`Engine::activate`).
pub fn absolutize(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Нормализовать путь (убрать `..`/`.`) — без обращения к ФС и без `\\?\`-префикса,
/// который добавляет `canonicalize`: в логах и отчётах он только мешает.
pub fn clean_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Оценка «модель + KV» по ролям: сколько VRAM просит каждая роль при запросе.
///
/// Считается по конфигу и метаданным GGUF — **без обращения к движку**, поэтому
/// годится и для резидентного хоста, и для разового `llm_host status` без
/// кластера. Диспетчер (`dispatch::plan_query`) обязан получать те же числа, что
/// печатает `status`, иначе решение и отчёт разъезжаются.
pub fn role_needs(cfg: &LlmHostConfig, runtime_root: &Path) -> BTreeMap<String, u64> {
    let mut needs = BTreeMap::new();
    for rc in &cfg.roles {
        let model_path =
            match crate::runtime::resolve_model_checked(runtime_root, &rc.model_spec, &rc.role) {
                Ok(p) => p,
                Err(_) => continue, // роли без модели нечего планировать
            };
        let file_mib = std::fs::metadata(&model_path)
            .map(|m| m.len() >> 20)
            .unwrap_or(0);
        // роль на CPU (`-ngl 0`, например legacy-реранкер) VRAM не занимает: иначе
        // диспетчер считал бы её «кандидатом на вытеснение» и «освобождал» память,
        // которой она не держит
        let n_gpu_layers = if cfg.gpu.n_gpu_layers_set {
            cfg.gpu.n_gpu_layers
        } else {
            rc.legacy_n_gpu_layers.unwrap_or(cfg.gpu.n_gpu_layers)
        };
        if n_gpu_layers == 0 && config::role_base(&rc.role) != "whisper" {
            needs.insert(rc.role.clone(), 0);
            continue;
        }
        let parallel = cfg.parallel.max(1) as i64;
        // KV-тип роли учитываем в оценке: с нашим патчем движка `--cache-type-k/v`
        // доходит до llama.cpp, и q8_0 даёт примерно вдвое меньше KV у чата.
        let kv_bits = if rc.cache_type_k == Some(8) {
            KvBits::Q8_0
        } else {
            KvBits::F16
        };
        let need = match read_meta(&model_path) {
            Ok(meta) => {
                // `rc.n_ctx` — общий контекст (parallel × ctx_per_slot); llama.cpp
                // делит его между слотами, поэтому KV считаем от контекста слота
                // (M4 §3.1). Умножение общего контекста на parallel ещё раз
                // завышало оценку в `parallel` раз (при parallel = 1 не видно).
                let per_slot = crate::budget::per_slot_ctx(rc.n_ctx.max(0) as i64, parallel);
                let base = estimate_need_mib(&meta, file_mib, per_slot, parallel, kv_bits);
                // compute-буфер движка: масштабируется `n_ubatch`, и на больших значениях
                // (legacy embedding `--ubatch-size 8192`) это гигабайты — прежняя оценка
                // «модель + KV» их не видела вовсе.
                let ubatch = rc.n_ubatch.or(rc.n_batch).unwrap_or(2048).max(1) as i64;
                base + compute_buffer_mib(&meta, ubatch).ceil() as u64
            }
            // без метаданных считаем хотя бы вес файла (+5 %, как в оценке бюджета)
            Err(_) => file_mib + file_mib / 20,
        };
        needs.insert(rc.role.clone(), need);
    }
    needs
}

/// Инстансы глазами диспетчера: у каждого инстанса — **его собственная** оценка
/// «модель + KV» (замер A4: считать освобождаемое по запрошенной роли — враньё в
/// арифметике вытеснения), простой — из `idle`.
pub fn instance_uses(
    instances: &[crate::cluster::Instance],
    needs: &BTreeMap<String, u64>,
    idle: &BTreeMap<String, u64>,
) -> Vec<InstanceUse> {
    instances
        .iter()
        .map(|i| {
            let own = needs.get(&i.name).copied().unwrap_or(0);
            instance_use(
                i,
                &i.name.clone(),
                own,
                idle.get(&i.name).copied().unwrap_or(0),
            )
        })
        .collect()
}

/// Конфигурация запуска/опроса хоста — то, что раньше разбирал бинарь `llm_host_facade`.
///
/// Оставлена плоской структурой с `pub` полями (как `Args` в бинаре): бины
/// разбирают argv самостоятельно, а логика старта живёт в [`Host::start`].
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// `config.yaml` проекта (`HDS_CONFIG` — только как подсказка для бинов).
    pub config: PathBuf,
    /// Каталог общего llama-рантайма (иначе `LLAMA_RUNTIME_DIR`/ОС-умолчание).
    pub runtime: Option<PathBuf>,
    /// Каталог движка (иначе автопоиск: `index.whisper_engine_dir` → `%APPDATA%`).
    pub engine_dir: Option<PathBuf>,
    /// Переопределение `llm_server.host` (например для проверок на 127.0.0.2).
    pub host: Option<String>,
    /// Смещение портов для проверочных прогонов (`chat=base`, `embedding=base+1`…).
    pub port_base: Option<u16>,
    /// Принудительный `n_gpu_layers` (проверки: `--ngl 0` — всё на CPU, без VRAM).
    pub ngl: Option<i32>,
    /// Переопределение режима размышлений по умолчанию.
    pub thinking: Option<Thinking>,
    /// Диспетчер VRAM (вытеснение/пауза). `false` — только наблюдение.
    pub dispatcher: bool,
    /// Каталог сигнальных файлов (`index.pause`) — корень проекта.
    pub pause_dir: PathBuf,
    /// Каталог состояния для pid/лог/heartbeat (M2 T2): desktop — корень проекта,
    /// `profile: server` — системный каталог (`%ProgramData%\hermes-disk-search` /
    /// `/Library/Application Support/hermes-disk-search`).
    pub data_dir: PathBuf,
    /// pid-файл (`None` — не занимать: разовые прогоны, проверки).
    pub pid_file: Option<PathBuf>,
    /// лог-файл (`None` — только консоль).
    pub log_file: Option<PathBuf>,
    /// Обслуживать `/internal/*` (CLI). По умолчанию да (`true`).
    pub internal: bool,
    /// Индекс GPU в номерe NVML (0 — первая NVIDIA-карта).
    pub nvml_index: u32,
}

impl Default for HostConfig {
    fn default() -> Self {
        let root = repo_root();
        // M2 T2: конфиг — `HDS_CONFIG` → корень проекта; если в корне его нет
        // (server-поставка) — системный каталог состояния.
        let config = match std::env::var("HDS_CONFIG") {
            Ok(v) if !v.is_empty() => PathBuf::from(v),
            _ => {
                let root_cfg = root.join("config.yaml");
                if root_cfg.is_file() {
                    root_cfg
                } else if let Some(sys) = system_state_dir() {
                    let sys_cfg = sys.join("config.yaml");
                    if sys_cfg.is_file() {
                        sys_cfg
                    } else {
                        root_cfg
                    }
                } else {
                    root_cfg
                }
            }
        };
        let data_dir = resident_data_root(&config);
        HostConfig {
            config,
            runtime: None,
            engine_dir: None,
            host: None,
            port_base: None,
            ngl: None,
            thinking: None,
            dispatcher: true,
            pause_dir: root,
            data_dir: data_dir.clone(),
            pid_file: Some(resident::default_pid_path(&data_dir)),
            log_file: Some(resident::default_log_path(&data_dir)),
            internal: true,
            nvml_index: 0,
        }
    }
}

impl HostConfig {
    /// Конфиг `config.yaml` проекта со всеми умолчаниями (резидентность включена).
    pub fn new(config: impl Into<PathBuf>) -> Self {
        HostConfig {
            config: config.into(),
            ..HostConfig::default()
        }
    }

    /// Конфиг **без** pid/лог-файлов: разовые прогоны (`--hold`), проверки и тесты
    /// не должны отбирать resident-слот у боевого процесса.
    pub fn without_residency(mut self) -> Self {
        self.pid_file = None;
        self.log_file = None;
        self
    }

    /// Путь pid-файла (даже если резидентность выключена — для сообщений).
    pub fn pid_path(&self) -> PathBuf {
        self.pid_file
            .clone()
            .unwrap_or_else(|| resident::default_pid_path(&self.pause_dir))
    }

    /// Путь лог-файла (даже если лог выключен — для сообщений).
    pub fn log_path(&self) -> PathBuf {
        self.log_file
            .clone()
            .unwrap_or_else(|| resident::default_log_path(&self.pause_dir))
    }
}

/// Аргументы локального отчёта — «что происходит с GPU/ролями прямо сейчас»,
/// без резидентного процесса. Это то, что делает `llm_host_status`, а также
/// `llm_host status`, если резидент не отвечает.
pub struct LocalStatusArgs {
    pub config: PathBuf,
    pub runtime: Option<PathBuf>,
    pub engine_dir: Option<PathBuf>,
    pub baseline_used_mib: Option<u64>,
    pub pause_dir: PathBuf,
    /// Не открывать движок (например когда он занят боевым процессом).
    pub no_engine: bool,
    pub nvml_index: u32,
}

impl Default for LocalStatusArgs {
    fn default() -> Self {
        LocalStatusArgs {
            config: HostConfig::default().config,
            runtime: None,
            engine_dir: None,
            baseline_used_mib: None,
            pause_dir: repo_root(),
            no_engine: false,
            nvml_index: 0,
        }
    }
}

/// Готовый локальный отчёт: сам `StatusReport` + строки, которые печатает бинарь.
pub struct LocalStatus {
    pub report: StatusReport,
    pub pause_line: String,
    /// `None` — в кластере нет инстансов (или движок не открывался).
    pub instances_line: Option<String>,
    /// Как измеряется **целевое** устройство (проба ↔ устройство движка).
    pub target_line: Option<String>,
    /// M6 T3 §3.3: VRAM по картам (карта, бэкенд, свободно, роли).
    pub cards_line: Option<String>,
}

impl LocalStatus {
    /// Человеческие строки для консоли (`llm_host_status`, `llm_host status`).
    pub fn lines(&self) -> Vec<String> {
        let mut out = self.report.lines();
        if let Some(line) = &self.target_line {
            out.push(line.clone());
        }
        if let Some(line) = &self.cards_line {
            out.push(line.clone());
        }
        out.push(self.pause_line.clone());
        if let Some(line) = &self.instances_line {
            out.push(line.clone());
        }
        out
    }
}

/// Собрать локальный отчёт: устройства движка, свободная VRAM (NVML — источник
/// истины, R29), бюджет, `index.pause` + heartbeat, роли и прогноз диспетчера.
pub fn local_status(args: &LocalStatusArgs) -> Result<LocalStatus> {
    let cfg = config::load(&args.config)?;
    let paths = match &args.runtime {
        Some(r) => RuntimePaths::new(r.clone()),
        None => RuntimePaths::from_env()?,
    };

    let mut engine_dir: Option<PathBuf> = args.engine_dir.clone();
    let mut devices = Vec::new();
    let mut instances = Vec::new();
    if !args.no_engine {
        match Engine::open(args.engine_dir.as_deref()) {
            Ok(engine) => {
                engine_dir = Some(engine.dir().to_path_buf());
                let _cwd = engine.activate()?; // cwd движка обязателен (грабли A1/A2)
                let cluster = engine.create_cluster()?;
                devices = cluster.devices()?;
                instances = cluster.instances()?;
            }
            Err(e) => println!("движок недоступен: {e}"),
        }
    }

    let needs = role_needs(&cfg, &paths.root);
    let planned = if devices.is_empty() {
        Vec::new()
    } else {
        registry::plan(&cfg, &paths.root, &devices)
    };

    let pause = IndexPause::new(args.pause_dir.clone());
    let heartbeat = read_heartbeat(&args.pause_dir);
    let vram_probe = open_vram_probe(args.nvml_index);
    let vram = vram_probe.as_ref().and_then(|p| p.snapshot());

    // Пробы, привязанные к устройствам движка: прогноз должен считаться по тому
    // устройству, на которое поедет роль (`gpu.device_index`), а не по «первой
    // карте системы» — иначе на машине с iGPU+dGPU прогноз бессмыслен.
    let probes = bind_device_probes(&devices, args.nvml_index);
    let target_bridge =
        match crate::device::selection_from_config_index(cfg.gpu.device_index, &devices) {
            Ok(crate::device::DeviceSelection::Csv(csv)) => csv
                .split(',')
                .next()
                .and_then(|s| s.trim().parse::<i32>().ok()),
            _ => None,
        };
    let by_probe = target_bridge
        .and_then(|idx| probes.get(&idx))
        .and_then(|p| p.snapshot())
        .map(|v| v.free_mib);
    let by_engine = target_bridge.and_then(|idx| {
        devices
            .iter()
            .find(|d| d.bridge_device_index == idx)
            .map(|d| d.memory_free >> 20)
    });
    let target_free = match (by_probe, by_engine) {
        (Some(p), Some(e)) => Some(p.min(e)),
        (Some(p), None) => Some(p),
        (None, Some(e)) => Some(e),
        (None, None) => None,
    };
    let target_line = target_bridge.map(|idx| {
        let dev = devices.iter().find(|d| d.bridge_device_index == idx);
        match &probes.get(&idx) {
            Some(p) => format!(
                "измерение целевого устройства (index={idx}{}): {} ({}) — свободно {} МиБ",
                dev.map(|d| format!(", {}", d.description_or_name()))
                    .unwrap_or_default(),
                p.name(),
                p.source().as_str(),
                target_free
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "?".to_string())
            ),
            None => format!(
                "измерение целевого устройства (index={idx}{}): проба не сопоставилась — \
                 по числам движка: свободно {} МиБ",
                dev.map(|d| format!(", {}", d.description_or_name()))
                    .unwrap_or_default(),
                target_free
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "?".to_string())
            ),
        }
    });

    // Прогноз диспетчера: что будет, если запрос чата придёт прямо сейчас.
    let idle: BTreeMap<String, u64> = BTreeMap::new();
    let uses = instance_uses(&instances, &needs, &idle);
    let forecast = cfg.role("chat").map(|_| {
        plan_query(
            &cfg.gpu,
            target_free,
            &Demand::new("chat", needs.get("chat").copied().unwrap_or(0)),
            &uses,
        )
    });

    let report = StatusReport::build(StatusInput {
        config: &cfg,
        runtime_root: &paths.root,
        engine_dir: engine_dir.as_deref(),
        devices: &devices,
        instances: &instances,
        vram,
        vram_source: cfg.gpu.vram_source,
        baseline_used_mib: args.baseline_used_mib,
        paused: pause.is_paused(),
        pause_file: pause.path(),
        heartbeat,
        planned: &planned,
        needs: &needs,
        decision: None,
        forecast: forecast.as_ref().map(|p| ("chat", p)),
    });

    let pause_line = format!(
        "пауза индексации: файл {} — {}, вложенность {}",
        pause.path().display(),
        if pause.is_paused() {
            "стоит"
        } else {
            "нет"
        },
        pause.depth()
    );
    let instances_line = if instances.is_empty() {
        None
    } else {
        Some(format!(
            "инстансы кластера: {}",
            uses.iter()
                .map(|i| format!(
                    "{}[{}/{}, активных {}]",
                    i.name,
                    crate::ffi::state::name(i.state),
                    i.retention_label(),
                    i.active_requests
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ))
    };

    // M6 T3 §3.3: строки по картам — какая карта сколько имеет свободной VRAM и
    // какие роли на ней (свидетельство раскладки для оператора).
    let role_cards: BTreeMap<String, i32> = planned
        .iter()
        .filter_map(|p| p.ready())
        .filter_map(|i| i.bridge_device_index.map(|c| (i.role.clone(), c)))
        .collect();
    let cards = cards_lines(&devices, &role_cards, &|c| {
        probes
            .get(&c)
            .and_then(|p| p.snapshot())
            .map(|v| v.free_mib)
    });
    let cards_line = if cards.is_empty() {
        None
    } else {
        Some(cards.join("; "))
    };

    Ok(LocalStatus {
        report,
        pause_line,
        instances_line,
        target_line,
        cards_line,
    })
}

/// Кластер, доступный из потоков фасада (§3 плана W2: «один владелец GPU»).
///
/// `Cluster` держит сырой указатель движка (FFI), поэтому сам по себе не `Send`;
/// фасад обслуживает запросы в потоках сокетов — значит доступ сериализуем
/// мьютексом: движок вызывается **из одного потока за раз**.
///
/// Обёртка над [`crate::gate::Gate`] добавлена после инцидента `W4_REPORT.md` §14:
/// зависший вызов движка держал мьютекс, а снаружи это выглядело как «резидент не
/// отвечает» (статус и роли ждали тот же мьютекс). Теперь видно **кто** держит
/// движок и **сколько**, а «наблюдательные» вызовы умеют ждать с бюджетом
/// ([`ClusterShared::try_with`]) и честно отдавать `Busy`.
pub struct ClusterShared(crate::gate::Gate<Cluster>);

// SAFETY: доступ к кластеру идёт только через шлюз ([`crate::gate::Gate`]):
// `with()`/`with_tagged()`/`try_with()` — эксклюзивно (управление кластером),
// `with_shared()`/`try_with_shared()` — параллельно для генерации (M4 T2).
// Методы `Cluster` берут `&self` (FFI-вызовы внутри), поэтому разделяемый доступ
// безопасен на уровне Rust; конкурентность самих запросов внутри движка
// обеспечивают слоты инстанса (патч P1: `wait_for_instance_slot` с дедлайном).
unsafe impl Send for ClusterShared {}
unsafe impl Sync for ClusterShared {}

/// Бюджет ожидания для «наблюдательных» вызовов (статус, `/props`, арбитр):
/// дольше ждать нет смысла — лучше честно сказать «движок занят».
pub const OBSERVE_BUDGET: Duration = Duration::from_millis(300);

/// Порог «долгого» вызова движка (мс): при превышении вызывающий пишет строку в лог
/// резидента и в heartbeat — чтобы инцидент вида «загрузка роли повисла» был виден
/// постфактум (зависший вызов из `W4_REPORT.md` §14 шёл 3 часа и не оставлял следов).
pub const SLOW_CALL_MS: u64 = 30_000;

/// Бюджет ожидания шлюза при загрузке/ожидании готовности роли (`ensure_loaded`,
/// `/internal/load`). Дольше ждать не нужно: движок либо отвечает, либо мы честно
/// говорим «занят» (`W4_REPORT.md` §14).
pub const LOAD_STEP_BUDGET: Duration = Duration::from_secs(30);

/// Бюджет ожидания шлюза для применения решения диспетчера. Если движок занят чужим
/// вызовом, вытеснение **откладывается**: вставать в очередь нельзя — именно это
/// превращало запрос в «резидент не отвечает».
pub const DISPATCH_BUDGET: Duration = Duration::from_millis(500);

/// Бюджет ожидания для фонового арбитра: занят движок — просто пропускаем такт
/// (арбитру не нужно решение «во что бы то ни стало»).
pub const ARBITER_BUDGET: Duration = Duration::from_millis(500);

/// Ошибка «движок занят»: клиент видит её как `503` вместо бесконечного ожидания.
fn busy_err(busy: crate::gate::Busy) -> EngineError {
    EngineError::Other(format!("движок занят ({busy}) — попробуйте позже"))
}

/// Страж брошенных запросов (чистая функция — тестируется без движка): сработать,
/// если держатель — «engine» (запрос чата/эмбеддингов, а не служебная операция)
/// и он держится дольше порога. Порог 0 = страж выключен (никогда не срабатывает).
fn abandoned_request_guard(busy: &crate::gate::Busy, engine_tag: &str, threshold_sec: u64) -> bool {
    threshold_sec > 0 && busy.what == engine_tag && busy.secs() >= threshold_sec
}

impl ClusterShared {
    pub fn new(cluster: Cluster) -> ClusterShared {
        ClusterShared(crate::gate::Gate::new(cluster))
    }

    /// Вызвать движок под мьютексом (единственный блокирующий путь к кластеру).
    pub fn with<T>(&self, f: impl FnOnce(&Cluster) -> T) -> T {
        self.0.with_tagged("engine", f)
    }

    /// То же, но с меткой операции — она попадает в `busy`/`status`/heartbeat
    /// (роль `embedding`, `wait_loaded`, `arbiter`…), поэтому зависший движок
    /// больше не выглядит безымянным.
    pub fn with_tagged<T>(&self, what: &str, f: impl FnOnce(&Cluster) -> T) -> T {
        self.0.with_tagged(what, f)
    }

    /// Ожидание с бюджетом: `Err(Busy)` — движок занят (внутри — кто и сколько).
    pub fn try_with<T>(
        &self,
        what: &str,
        budget: Duration,
        f: impl FnOnce(&Cluster) -> T,
    ) -> std::result::Result<T, crate::gate::Busy> {
        self.0.try_with(what, budget, f)
    }

    /// **Разделяемый** вызов для генерации (M4 T2): параллельно с другими
    /// генерациями, но не с управлением кластером (загрузка/выгрузка/арбитр).
    ///
    /// Метка занятости — `engine` (как у [`ClusterShared::with`]): страж брошенных
    /// запросов и heartbeat продолжают её узнавать.
    pub fn with_shared<T>(&self, f: impl FnOnce(&Cluster) -> T) -> T {
        self.0.with_shared_tagged("engine", f)
    }

    /// Разделяемый вызов с бюджетом: `Err(Busy)` — идёт управление кластером.
    pub fn try_with_shared<T>(
        &self,
        what: &str,
        budget: Duration,
        f: impl FnOnce(&Cluster) -> T,
    ) -> std::result::Result<T, crate::gate::Busy> {
        self.0.try_with_shared(what, budget, f)
    }

    /// Дождаться готовности инстанса, **не удерживая шлюз**: опрос шагами, между
    /// шагами замок свободен. Нужен всем, кто ждёт загрузку (`ensure_loaded`,
    /// `/internal/load`): иначе один ожидающий блокирует `status`, арбитр и другие роли
    /// — ровно то, что случилось в инциденте `W4_REPORT.md` §14 (загрузка держала
    /// мьютекс, а снаружи это выглядело как «резидент не отвечает»).
    pub fn wait_loaded_stepwise(
        &self,
        what: &str,
        id: crate::ffi::InstanceId,
        timeout: Duration,
    ) -> Result<crate::cluster::Instance> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.try_with(what, LOAD_STEP_BUDGET, |c| {
                c.instance_by_id(id).ok().flatten()
            }) {
                Ok(Some(inst)) if inst.is_loaded() || inst.is_failed() => return Ok(inst),
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(EngineError::InstanceNotFound {
                        name: format!("id={id}"),
                    })
                }
                // шлюз занят чужим вызовом: ждём следующего шага, пока идёт таймаут
                Err(_) => {}
            }
            if Instant::now() >= deadline {
                return Err(EngineError::Other(format!(
                    "таймаут ожидания загрузки инстанса {id} ({timeout:?}): движок не ответил"
                )));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Кто держит движок сейчас (`None` — свободен).
    pub fn busy(&self) -> Option<crate::gate::Busy> {
        self.0.busy()
    }

    /// Последний долгий вызов `(что, мс)` — для `status`/heartbeat/логов.
    pub fn last_slow(&self) -> Option<(String, u64)> {
        self.0.last_slow()
    }

    /// Порог «долгого» вызова движка (мс) — при превышении пишем в лог и в heartbeat.
    pub fn set_slow_ms(&self, ms: u64) {
        self.0.set_slow_ms(ms);
    }
}

/// Нормализация имени устройства для сопоставления: нижний регистр, без
/// пунктуации, одиночные пробелы (`AMD Radeon(TM) Graphics` → `amd radeon tm graphics`).
fn normalized_device_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// «Шумовые» токены имён, которые драйверы добавляют произвольно (`(TM)`, `(R)`):
/// после их удаления имена одной карты у движка, NVML и DXGI совпадают буквально.
const DEVICE_NAME_NOISE: [&str; 3] = ["tm", "r", "reg"];

/// Значимые токены имени устройства (нормализация минус шумовые токены).
fn device_name_tokens(s: &str) -> Vec<String> {
    normalized_device_name(s)
        .split_whitespace()
        .filter(|t| !DEVICE_NAME_NOISE.contains(t))
        .map(|t| t.to_string())
        .collect()
}

/// Совпадают ли устройство движка и проба VRAM (NVML/DXGI)?
///
/// Сверка **строгая**: после нормализации и удаления шумовых токенов списки
/// токенов должны совпасть точно. Это осознанный выбор: движок, NVML и DXGI
/// берут имя у одного драйвера, поэтому имена либо совпадают, либо различаются
/// по существу. «Мягкое» сопоставление по вхождению дало бы ложные срабатывания
/// (`RTX 3060` ⊂ `RTX 3060 Ti`, `Radeon Graphics` ⊂ `Radeon Vega Graphics`) и
/// привязало бы пробу **другой** карты — а это тот самый баг «мерим один GPU,
/// грузим на другой», от которого мы уходим. Не сопоставилось — проба не
/// используется (падаем на числа движка), и это безопаснее.
pub fn names_match(engine_desc: &str, probe_name: &str) -> bool {
    let a = device_name_tokens(engine_desc);
    let b = device_name_tokens(probe_name);
    !a.is_empty() && a == b
}

/// Сопоставить пробы VRAM с устройствами движка: bridge-индекс → проба.
///
/// Порядок предпочтения: NVML (источник истины на NVIDIA, R29) → DXGI
/// (вендор-нейтральный, Windows). Проба используется только для устройства с
/// совпавшим именем; устройство без совпадения остаётся на числах движка.
pub fn bind_device_probes(
    devices: &[crate::cluster::Device],
    nvml_index: u32,
) -> BTreeMap<i32, Arc<dyn VramProbe>> {
    let mut candidates: Vec<Arc<dyn VramProbe>> = Vec::new();
    // NVML: карта из конфига — первой, затем остальные NVIDIA-карты по индексу.
    if let Ok(p) = crate::vram::NvmlProbe::open(nvml_index) {
        candidates.push(Arc::new(p));
    }
    for idx in 0..8u32 {
        if idx == nvml_index {
            continue;
        }
        if let Ok(p) = crate::vram::NvmlProbe::open(idx) {
            candidates.push(Arc::new(p));
        }
    }
    #[cfg(windows)]
    if let Ok(adapters) = crate::vram::dxgi::DxgiProbe::open_all() {
        candidates.extend(
            adapters
                .into_iter()
                .map(|p| Arc::new(p) as Arc<dyn VramProbe>),
        );
    }
    let mut out: BTreeMap<i32, Arc<dyn VramProbe>> = BTreeMap::new();
    for dev in devices.iter().filter(|d| d.is_accelerator()) {
        let desc = dev.description_or_name();
        if let Some(p) = candidates.iter().find(|c| names_match(&desc, &c.name())) {
            out.insert(dev.bridge_device_index, Arc::clone(p));
        }
    }
    out
}

/// Основная часть — перенос из `bin/llm_host_facade.rs` (A5), поэтому поля и
/// порядок решений совпадают с проверенным живым прогоном (§7.3 `W2_REPORT.md`).
/// Убрать `PauseIndex` из плана для ролей, которые обслуживают **самого индексатора**.
///
/// `embedding`/`rerank` во время индексации вызывает сам индексатор; пауза
/// индексации на таких запросах — «индексатор ставит паузу себе»: файл
/// `index.pause` появляется/исчезает на каждом запросе эмбеддингов → UI мигает
/// «идёт ↔ пауза», а лог диспетчера растёт на две строки за запрос (живой
/// инцидент 05.10.2026, ноутбук во время первичной индексации). Пауза имеет
/// смысл для пользовательских запросов (chat) и транскрибации — их не трогаем.
/// Арифметика VRAM и остальные действия плана не меняются.
fn strip_index_self_pause(plan: &mut Plan, role: &str) {
    if role == "embedding" || role == "rerank" {
        plan.actions
            .retain(|a| !matches!(a, crate::dispatch::Action::PauseIndex { .. }));
    }
}

pub struct ClusterBackend {
    /// Кластер движка. `None` в режимах `facade`/`off` — инстансов нет, запросы
    /// ролей отклоняются честной причиной (или проксируются, если есть upstream).
    cluster: Option<Arc<ClusterShared>>,

    /// Инстансы по ролям (id из `create_instance`).
    ids: BTreeMap<String, i64>,
    /// Модель и контекст роли — для `/props`.
    model_path: BTreeMap<String, String>,
    n_ctx: BTreeMap<String, i32>,
    /// Оценка «модель + KV» по ролям (для решений диспетчера).
    needs: BTreeMap<String, u64>,
    gpu: GpuConfig,
    vram_probe: Option<Arc<dyn VramProbe>>,
    /// Устройства движка на старте — источник сопоставления проб VRAM с GPU.
    devices: Vec<crate::cluster::Device>,
    /// Проба VRAM, **привязанная к устройству** (bridge-индекс → проба).
    ///
    /// Зачем: на машине с двумя GPU измерять одну карту, а грузить модель на
    /// другую — грубая ошибка (живой замер 05.10.2026: NVML мерил RTX (свободно
    /// 3569 МиБ), а роль выбиралась на встроенную AMD с 31 ГиБ свободных → ложный
    /// «не хватает VRAM»). Привязка — по имени устройства (`names_match`).
    probes: BTreeMap<i32, Arc<dyn VramProbe>>,
    pause: Arc<IndexPause>,
    /// Диспетчер включён (флаг `--dispatcher on`).
    dispatch_enabled: bool,
    /// Параллелизм слотов (`llama_server.parallel`) — для `/props`.
    parallel: i32,
    /// Журнал решений диспетчера (для `--json`, логов и `status`).
    decisions: Mutex<Vec<String>>,
    /// Последнее решение диспетчера — его печатает `status` (не прогноз: резидент
    /// знает факт).
    last_plan: Mutex<Option<Plan>>,
    /// Простой инстансов: время последнего запроса/загрузки.
    last_used: Mutex<BTreeMap<String, Instant>>,
    /// Флаг остановки процесса: выставляет `/internal/stop`.
    stop: Arc<AtomicBool>,
    /// Лог резидентно процесса (консоль + `data/logs/llm-host.log`).
    log: Arc<Log>,
    started: Instant,
    pid: u32,
    // --- данные для `/internal/status` (отчёт собирается на живых данных) ---
    cfg: LlmHostConfig,
    runtime_root: PathBuf,
    engine_dir: Option<PathBuf>,
    baseline_used_mib: Option<u64>,
    planned: Vec<RolePlan>,
    pause_dir: PathBuf,
    pid_path: PathBuf,
    log_path: PathBuf,
    /// Режим работы: `embedded` (кластер) | `facade` (проксирование) | `off`.
    mode: Mode,
    /// Базовые URL внешнего владельца по ролям (режим `facade`).
    upstream: BTreeMap<String, String>,
    /// W3 + M6 T2 §3.5: транскрибаторы whisper **по картам** (bridge-индекс → клетка).
    /// Один экземпляр на карту, один файл за раз (per-card mutex); разные файлы на
    /// разных картах идут параллельно. Ключ `-1` — CPU.
    whisper: Mutex<BTreeMap<i32, Arc<WhisperCell>>>,
    /// M6 T2 §3.6: роли-реплики чата (`chat`, `chat-2`, …) — запросы фасада
    /// раскидываются по ним **least-busy**, при равенстве — round-robin.
    chat_roles: Vec<String>,
    /// Счётчик round-robin между равнозагруженными репликами (атомарный: фасад
    /// обслуживает запросы в потоках сокетов).
    chat_rr: std::sync::atomic::AtomicUsize,
    /// M6 T2 §3.5/§3.6: карты, на которых разрешена транскрибация (роли
    /// `whisper`, `whisper-2`, …): один экземпляр на карту, карту под файл
    /// выбираем по свободной VRAM (числа — в логе).
    whisper_cards: Vec<(String, i32)>,
    /// M6 T3 §3.3: карта каждой роли (`role → bridge-индекс`) — из поднятого плана.
    ///
    /// Диспетчер считает бюджет и вытеснение **по карте роли**, а не по «целевой
    /// карте» `gpu.device_index`: на машине с несколькими GPU нехватка на одной
    /// карте не должна выгружать роли с другой. Пусто (нет раскладки/одна карта) —
    /// поведение до M6 сохраняется.
    role_cards: BTreeMap<String, i32>,
    /// T3.1 (§6): сейчас идёт задание автотранскрибации — индексация и предохранитель
    /// простоя не вытесняют транскрибатор, пока он работает. Запрос чата/поиска
    /// вытеснить его **может** (приоритет 1 > 2).
    transcribe_active: Arc<AtomicBool>,
    /// Страж брошенных запросов: порог одного вызова движка в секундах
    /// (`gpu.abandoned_request_sec`; 0 — выключен).
    abandoned_request_sec: u64,
    /// W4: настройки диаризации (`diarization.*`) — backend, модель, EP, вход ASR.
    diar: diar::DiarSettings,
    /// Backend по умолчанию (конфиг или авто по решению №10) — кэш решения.
    diar_auto: OnceLock<DiarBackend>,
    /// Nemotron-3: проверенная модель и ORT-сессия; переиспользуются между
    /// заданиями (`run_*` требует `&mut`, поэтому под `Mutex`).
    nemotron: Mutex<Option<Diarizer>>,
    /// Явный `index.ffmpeg_cmd` (остальной поиск — `diar::find_ffmpeg`).
    ffmpeg: Option<PathBuf>,
}

/// Ленивый транскрибатор whisper под `Mutex` (raw-указатели bridge ⇒ `!Send/!Sync`;
/// доступ строго под `Mutex`, поэтому `Send+Sync` помечаем вручную — как
/// [`ClusterShared`]).
struct WhisperCell(Mutex<crate::whisper::Whisper>);
unsafe impl Send for WhisperCell {}
unsafe impl Sync for WhisperCell {}

/// Дополнительный буфер whisper на GPU (compute/KV сверх веса модели), МиБ —
/// для проверки VRAM-бюджета перед созданием (критерий приёмки W3).
const WHISPER_OVERHEAD_MIB: u64 = 256;

/// Решение об устройстве whisper по VRAM (чистая логика — тестируется без движка).
///
/// `free == None` (NVML недоступен) — доверяем запросу (best effort). Иначе, если
/// свободной VRAM меньше «модель + буфер + резерв», возвращаем CPU (`-1`) и причину.
fn whisper_device(
    gpu: i32,
    model_mib: u64,
    reserve_mb: u64,
    free: Option<u64>,
) -> (i32, Option<String>) {
    if gpu < 0 {
        return (gpu, None);
    }
    let need = model_mib + WHISPER_OVERHEAD_MIB + reserve_mb;
    match free {
        Some(f) if f < need => (
            -1,
            Some(format!(
                "свободно {f} МиБ < {need} (модель {model_mib} + буфер \
                 {WHISPER_OVERHEAD_MIB} + резерв {reserve_mb})"
            )),
        ),
        _ => (gpu, None),
    }
}

/// R3.4 (PLAN_TRANSCRIBE_SERVICE): каталоги моделей в порядке приоритета —
/// env `HDS_MODELS_DIR` → `%ProgramData%\hermes-disk-search\models` (машинный, виден
/// службам под LocalSystem) → общий каталог движка `%APPDATA%\OpenResearchTools\models`.
/// Перебор идёт по списку: первый каталог с найденной моделью побеждает.
fn models_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(v) = std::env::var_os("HDS_MODELS_DIR") {
        if !v.is_empty() {
            out.push(PathBuf::from(v));
        }
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        let p = PathBuf::from(pd);
        if !p.as_os_str().is_empty() {
            out.push(p.join("hermes-disk-search").join("models"));
        }
    }
    if let Some(base) = directories::BaseDirs::new() {
        out.push(base.config_dir().join("OpenResearchTools").join("models"));
    }
    out
}

/// whisper-модель из общего каталога моделей: `*.bin`/`*.gguf` в каталоге `*whisper*`.
///
/// R3.4: каталоги перебираются в приоритете [`models_roots`].
fn default_whisper_model() -> Option<PathBuf> {
    for root in models_roots() {
        if let Some(p) = whisper_in_dir(&root) {
            return Some(p);
        }
    }
    None
}

/// Поиск whisper-модели в одном каталоге моделей.
fn whisper_in_dir(root: &std::path::Path) -> Option<PathBuf> {
    for e in std::fs::read_dir(root).ok()?.flatten() {
        let dir = e.path();
        let is_whisper = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase().contains("whisper"))
            .unwrap_or(false);
        if !dir.is_dir() || !is_whisper {
            continue;
        }
        for f in std::fs::read_dir(&dir).ok()?.flatten() {
            let p = f.path();
            let ok = p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("bin") || x.eq_ignore_ascii_case("gguf"))
                .unwrap_or(false);
            if ok {
                return Some(p);
            }
        }
    }
    None
}

/// sortformer-модель диаризации из общего каталога моделей: первый `*.gguf` в
/// каталоге `*sortformer*`. R3.4: каталоги перебираются в приоритете [`models_roots`].
fn default_diarization_model() -> Option<PathBuf> {
    for root in models_roots() {
        if let Some(p) = diarization_in_dir(&root) {
            return Some(p);
        }
    }
    None
}

/// Поиск sortformer-модели в одном каталоге моделей.
fn diarization_in_dir(root: &std::path::Path) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase().contains("sortformer"))
                    .unwrap_or(false)
        })
        .collect();
    dirs.sort();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|f| f.path())
            .filter(|p| {
                p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("gguf"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort();
        if let Some(p) = files.into_iter().next() {
            return Some(p);
        }
    }
    None
}

/// Взводит флаг «идёт задание автотранскрибации» и снимает его на `Drop` (§6, T3.1).
///
/// Флаг читают `uses()`/`whisper_idle_evict()`: индексация и предохранитель простоя
/// не вытесняют транскрибатор **во время работы**. Запрос чата/поиска вытеснить его
/// может — приоритет 1 > 2.
struct TranscribeGuard(Arc<AtomicBool>);

impl TranscribeGuard {
    fn set(flag: &Arc<AtomicBool>) -> TranscribeGuard {
        flag.store(true, Ordering::Relaxed);
        TranscribeGuard(Arc::clone(flag))
    }
}

impl Drop for TranscribeGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

/// Оценка VRAM задания автотранскрибации: whisper-модель + sortformer + буфер.
///
/// Считается по размерам файлов (замер T0.1: turbo 1549 МБ + sortformer 449 МБ,
/// фактическая дельта VRAM ≈ 0,86 ГБ) — этого достаточно для решения арбитра.
fn transcribe_need_mib(whisper: &Path, diar: Option<&Path>) -> u64 {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len() >> 20).unwrap_or(0);
    size(whisper) + diar.map(size).unwrap_or(0) + WHISPER_OVERHEAD_MIB
}

/// Выбор реплики под запрос (M6 §3.6): **least-busy**, при равенстве —
/// round-robin. Чистая функция (загрузки передаются аргументом) — тестируется
/// без движка; порядок реплик — как в конфиге (`chat`, `chat-2`, …).
pub fn pick_replica<'a>(
    roles: &'a [String],
    loads: &BTreeMap<String, i32>,
    rr: &std::sync::atomic::AtomicUsize,
) -> Option<&'a str> {
    if roles.is_empty() {
        return None;
    }
    if roles.len() == 1 {
        return Some(roles[0].as_str());
    }
    let min = roles
        .iter()
        .map(|r| loads.get(r).copied().unwrap_or(0))
        .min()
        .unwrap_or(0);
    let idle: Vec<&String> = roles
        .iter()
        .filter(|r| loads.get(*r).copied().unwrap_or(0) == min)
        .collect();
    let idx = rr.fetch_add(1, Ordering::Relaxed) % idle.len();
    Some(idle[idx].as_str())
}

impl ClusterBackend {
    /// Кластер движка или объяснение, почему его нет (режим `llm_server.mode`).
    fn cl(&self) -> Result<&Arc<ClusterShared>> {
        self.cluster.as_ref().ok_or_else(|| {
            EngineError::Other(match self.mode {
                Mode::Off => "LLM выключен (llm_server.mode: off) — инстансы не создаются, \
                     поиск работает только по FTS"
                    .to_string(),
                Mode::Facade => "режим llm_server.mode: facade — своих инстансов нет, роли держит \
                     внешний OpenAI-совместимый сервер"
                    .to_string(),
                Mode::Embedded => "кластер движка не создан".to_string(),
            })
        })
    }

    /// Свободная VRAM — NVML (источник истины, R29) или DXGI (вендор-нейтральный
    /// фолбэк Windows); `None` — измерить нечем.
    fn free_mib(&self) -> Option<u64> {
        self.vram_probe
            .as_ref()
            .and_then(|p| p.snapshot())
            .map(|v| v.free_mib)
    }

    /// Bridge-индекс устройства, выбранного для ролей (`gpu.device_index`).
    ///
    /// Именно это устройство получит инстанс, поэтому и измерять надо его, а не
    /// «первую карту системы» (на машинах с iGPU + dGPU это разные GPU).
    fn target_bridge_index(&self) -> Option<i32> {
        match crate::device::selection_from_config_index(self.gpu.device_index, &self.devices) {
            Ok(crate::device::DeviceSelection::Csv(csv)) => {
                csv.split(',').next()?.trim().parse::<i32>().ok()
            }
            _ => None,
        }
    }

    /// Живые числа движка по устройству (неблокирующе: движок занят → `None`).
    fn engine_free_mib(&self, bridge_index: i32) -> Option<u64> {
        let cluster = self.cluster.as_ref()?;
        let devices = cluster
            .try_with("devices", DISPATCH_BUDGET, |c| c.devices())
            .ok()?
            .ok()?;
        devices
            .iter()
            .find(|d| d.bridge_device_index == bridge_index)
            .map(|d| d.memory_free >> 20)
    }

    /// Имя устройства движка по bridge-индексу (`CUDA0`/`Vulkan0`/`Metal0`/`CPU`).
    ///
    /// Раньше имя диаризации хардкодилось `CUDA{gpu}` — валидно только для
    /// CUDA-сборки движка; на Vulkan/Metal-сборках такого бэкенда нет, и
    /// диаризация падала «failed to initialize backend: CUDA0» (живой инцидент
    /// 05.10.2026, ноутбук AMD). Для CPU-пути (`gpu < 0`) возвращаем `None` —
    /// движок сам оставит диаризацию на CPU (`whisper_no_gpu`).
    fn engine_device_name(&self, bridge_index: i32) -> Option<String> {
        if bridge_index < 0 {
            return None;
        }
        let cluster = self.cluster.as_ref()?;
        let devices = cluster
            .try_with("devices", DISPATCH_BUDGET, |c| c.devices())
            .ok()?
            .ok()?;
        devices
            .iter()
            .find(|d| d.bridge_device_index == bridge_index)
            .map(|d| d.name.clone())
    }

    /// Свободная VRAM **целевого устройства** — то, чем распоряжается диспетчер.
    ///
    /// Берём минимум из двух сигналов: проба (NVML/DXGI — «сколько разрешено ОС»)
    /// и живые числа движка по тому же устройству («сколько видит его бэкенд»).
    /// На UMA-системах (iGPU, Metal) второе — реальная граница: DXGI-бюджет там
    /// заметно больше heap движка (замер 05.10.2026: бюджет iGPU 48 ГиБ против
    /// heap 16 ГиБ). Если проба не сопоставилась — используем числа движка с
    /// оговоркой R29 (лучше, чем «нет замера»: движок — тот, кто аллоцирует).
    fn target_free_mib(&self) -> Option<u64> {
        let idx = self.target_bridge_index()?;
        let by_probe = self
            .probes
            .get(&idx)
            .and_then(|p| p.snapshot())
            .map(|v| v.free_mib);
        match (by_probe, self.engine_free_mib(idx)) {
            (Some(p), Some(e)) => Some(p.min(e)),
            (Some(p), None) => Some(p),
            (None, Some(e)) => Some(e),
            (None, None) => None,
        }
    }

    /// Свободная VRAM **заданной** карты (проба по bridge-индексу, минимум с
    /// числами движка) — для выбора карты под транскрибацию (M6 §3.6).
    ///
    /// `None` — замера нет (например UMA/AMD: NVML карту не мерит) — это честное
    /// «нет замера», а не догадка; решение тогда по числам движка/фолбэку.
    fn card_free_mib(&self, bridge_index: i32) -> Option<u64> {
        if bridge_index < 0 {
            return None;
        }
        let by_probe = self
            .probes
            .get(&bridge_index)
            .and_then(|p| p.snapshot())
            .map(|v| v.free_mib);
        match (by_probe, self.engine_free_mib(bridge_index)) {
            (Some(p), Some(e)) => Some(p.min(e)),
            (Some(p), None) => Some(p),
            (None, Some(e)) => Some(e),
            (None, None) => None,
        }
    }

    /// Свободная VRAM **карты роли** (M6 T3 §3.3): проба ↔ числа движка по её
    /// bridge-индексу. Роль без раскладки — как раньше: целевое устройство
    /// (`gpu.device_index`), то есть одна карта на все роли.
    fn free_for_role(&self, role: &str) -> Option<u64> {
        match self.role_cards.get(role).copied() {
            Some(card) => self.card_free_mib(card),
            None => self.target_free_mib(),
        }
    }

    /// Инстансы **карты роли** — бюджет и вытеснение внутри неё (M6 T3 §3.3).
    ///
    /// Роль без раскладки карт не ограничивает: отдаём все инстансы (компат).
    fn uses_for_role(&self, role: &str, uses: Vec<InstanceUse>) -> Vec<InstanceUse> {
        let card = self.role_cards.get(role).copied();
        crate::dispatch::on_card(&uses, card)
    }

    /// Карта под файл транскрибации (M6 §3.6): среди настроенных ролей
    /// `whisper*` выбираем карту с максимальной свободной VRAM (числа — в логе),
    /// один файл — одна карта (split отвергнут). Ролей не настроено — остаётся
    /// исторический дефолт (bridge-индекс 0, дальше решает `whisper_device`).
    fn pick_whisper_card(&self, model_mib: u64) -> i32 {
        if self.whisper_cards.is_empty() {
            return 0;
        }
        let need = model_mib + WHISPER_OVERHEAD_MIB + self.gpu.reserve_mb;
        let mut best: Option<(i32, u64, String)> = None;
        for (role, card) in &self.whisper_cards {
            let free = self.card_free_mib(*card);
            self.log.line(&format!(
                "[whisper] карта {card} (роль {role}): свободно {} — нужно {need} МиБ \
                 (модель {model_mib} + буфер {WHISPER_OVERHEAD_MIB} + резерв {})",
                free.map(|f| f.to_string())
                    .unwrap_or_else(|| "нет замера".to_string()),
                self.gpu.reserve_mb
            ));
            let fits = free.map(|f| f >= need).unwrap_or(true);
            if fits {
                // «нет замера» считаем свободной: движок сам решит (как раньше)
                let score = free.unwrap_or(u64::MAX);
                match &best {
                    Some((_, s, _)) if *s >= score => {}
                    _ => best = Some((*card, score, role.clone())),
                }
            }
        }
        match best {
            Some((card, score, role)) => {
                self.log.line(&format!(
                    "[whisper] выбрана карта {card} (роль {role}): свободно {} МиБ ≥ нужно \
                     {need} МиБ — файл пойдёт на неё (§3.6: один файл — одна карта)",
                    if score == u64::MAX {
                        "нет замера".to_string()
                    } else {
                        score.to_string()
                    }
                ));
                card
            }
            None => {
                let card = self.whisper_cards[0].1;
                self.log.line(&format!(
                    "[whisper] ни одна карта не даёт {need} МиБ — берём карту {card}: \
                     `whisper_device` уведёт на CPU по VRAM-бюджету (§3.6)"
                ));
                card
            }
        }
    }

    fn id_of(&self, role: &str) -> Result<i64> {
        self.cl()?;
        self.ids.get(role).copied().ok_or_else(|| {
            EngineError::Other(format!(
                "инстанс роли '{role}' не создан (нет модели или роли нет в конфиге)"
            ))
        })
    }

    /// Загружена ли роль прямо сейчас (M4 T2).
    ///
    /// Разделяемый доступ: эксклюзивный `with` при занятом генерацией движке
    /// просто встал бы в очередь за ней и вернул бы сериализацию, которую T2 и
    /// убирает. Ошибки/занятость трактуем как «не загружена» — тогда вызывающий
    /// идёт штатным путём (`prepare` + `ensure_loaded`).
    fn role_loaded(&self, role: &str) -> bool {
        let (Ok(cl), Ok(id)) = (self.cl(), self.id_of(role)) else {
            return false;
        };
        cl.try_with_shared("check", OBSERVE_BUDGET, |c| {
            c.instance_by_id(id)
                .ok()
                .flatten()
                .map(|i| i.is_loaded())
                .unwrap_or(false)
        })
        .unwrap_or(false)
    }

    /// Загрузить роль, если она выгружена, и дождаться готовности.
    ///
    /// Нужен не только для `LOAD_ON_DEMAND`: диспетчер может выгрузить чат, чтобы
    /// отдать память другой роли (§8.6.2), и следующий запрос обязан его вернуть.
    fn ensure_loaded(&self, role: &str) -> Result<i64> {
        let cl = self.cl()?;
        let id = self.id_of(role)?;
        let loaded = cl.with_tagged("instances", |c| {
            c.instance_by_id(id)
                .ok()
                .flatten()
                .map(|i| i.is_loaded())
                .unwrap_or(false)
        });
        if !loaded {
            let load_tag = format!("load:{role}");
            // `load_instance` идёт секунды (модель с диска) — это нормально, но и сама
            // загрузка, и ожидание готовности берут бюджет, а не «ждут вечно»: замок
            // между шагами свободен (§14).
            cl.try_with(&load_tag, LOAD_STEP_BUDGET, |c| c.load(id))
                .map_err(|busy| {
                    EngineError::Other(format!(
                        "движок занят ({busy}) — роль '{role}' не загрузить за {} с",
                        LOAD_STEP_BUDGET.as_secs()
                    ))
                })??;
            let wait_tag = format!("wait_loaded:{role}");
            cl.wait_loaded_stepwise(&wait_tag, id, Duration::from_secs(300))?;
        }
        self.note_used(role);
        Ok(id)
    }

    /// Отметить, что роль работала (для `gpu.evict_idle_sec`).
    fn note_used(&self, name: &str) {
        if let Ok(mut m) = self.last_used.lock() {
            m.insert(name.to_string(), Instant::now());
        }
    }

    /// Простой по инстансам в секундах (для арбитра).
    fn idle_map(&self) -> BTreeMap<String, u64> {
        let now = Instant::now();
        self.last_used
            .lock()
            .map(|m| {
                m.iter()
                    .map(|(k, t)| (k.clone(), now.duration_since(*t).as_secs()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Инстансы кластера глазами диспетчера (с оценками и простоем).
    ///
    /// Ожидание ограничено [`OBSERVE_BUDGET`]: если движок занят (загрузка/инференс
    /// другой роли), арбитр и `status` получают пустой список и **busy**-метку, а не
    /// встают в очередь за зависшим вызовом (инцидент `W4_REPORT.md` §14).
    fn uses(&self) -> Vec<InstanceUse> {
        let instances = self
            .cluster
            .as_ref()
            .and_then(|c| {
                c.try_with("instances", OBSERVE_BUDGET, |x| x.instances())
                    .ok()
            })
            .and_then(|r| r.ok())
            .unwrap_or_default();
        let mut uses = instance_uses(&instances, &self.needs, &self.idle_map());
        // M6 T3 §3.3: помечаем карту инстанса (имя инстанса = имя роли) — по ней
        // диспетчер считает бюджет и ограничивает вытеснение картой.
        for u in uses.iter_mut() {
            if let Some(card) = self.role_cards.get(&u.name).copied() {
                u.card = Some(card);
            }
        }
        // §6 (T3.1): пока идёт задание автотранскрибации, помечаем его носитель —
        // индексация и предохранитель простоя его не вытесняют.
        if self.transcribe_active.load(Ordering::Relaxed) {
            for u in uses.iter_mut() {
                if u.role == crate::dispatch::TRANSCRIBE_TRANSPORT_ROLE {
                    u.transcribe_active = true;
                }
            }
        }
        uses
    }

    /// Загрузка чат-реплик (M6 §3.6): активные + ожидающие запросы инстанса.
    ///
    /// Читается неблокирующе (`OBSERVE_BUDGET`): занят движок — считаем, что
    /// загрузки неизвестны (0), и решает round-robin; запрос всё равно попадёт
    /// в реплику.
    fn chat_loads(&self) -> BTreeMap<String, i32> {
        let instances = self
            .cluster
            .as_ref()
            .and_then(|c| {
                c.try_with("chat:pick", OBSERVE_BUDGET, |x| x.instances())
                    .ok()
            })
            .and_then(|r| r.ok())
            .unwrap_or_default();
        self.chat_roles
            .iter()
            .map(|role| {
                let load = self
                    .ids
                    .get(role)
                    .and_then(|id| instances.iter().find(|i| i.id == *id))
                    .map(|i| i.active_request_count + i.queued_request_count)
                    .unwrap_or(0);
                (role.clone(), load)
            })
            .collect()
    }

    /// Реплика чата под запрос: least-busy, при равенстве — round-robin (§3.6).
    /// `None` — чат-роль не поднята вовсе.
    fn pick_chat_role(&self) -> Option<String> {
        pick_replica(&self.chat_roles, &self.chat_loads(), &self.chat_rr).map(|s| s.to_string())
    }

    /// Перед запросом: решить по VRAM и применить решение (ARB-1/ARB-2).
    ///
    /// Возвращает `PauseLease`, который держится до конца запроса: на `Drop` пауза
    /// снимается (если ставили её мы) — индексные роли вернутся сами.
    ///
    /// `Err` — после вытеснения памяти всё ещё не хватает: запрос надо отклонить с
    /// отчётом (`Verdict::NotEnough`), **без авто-деградации** (§8.6.2).
    fn prepare(&self, role: &str) -> Result<Option<PauseLease>> {
        let need = self.needs.get(role).copied().unwrap_or(0);
        self.prepare_demand(role, need, false)
    }

    /// Как [`ClusterBackend::prepare`], но потребность задаётся явно, и можно выбрать
    /// план **автотранскрибации** ([`plan_transcribe`], §6).
    ///
    /// Нужно потому, что у логической роли `transcribe` нет инстанса в кластере:
    /// её потребность (whisper + sortformer + буфер) считает вызывающий.
    fn prepare_demand(
        &self,
        role: &str,
        need: u64,
        transcribe: bool,
    ) -> Result<Option<PauseLease>> {
        if !self.dispatch_enabled {
            return Ok(None);
        }
        if need == 0 {
            return Ok(None);
        }
        let plan = {
            // сколько освободит каждый инстанс — по ЕГО собственной роли, а не по
            // запрошенной: иначе арифметика вытеснения врёт (поймано живым прогоном)
            let uses = self.uses_for_role(role, self.uses());
            let demand = Demand::new(role, need);
            let mut plan = if transcribe {
                plan_transcribe(&self.gpu, self.free_for_role(role), &demand, &uses)
            } else {
                plan_query(&self.gpu, self.free_for_role(role), &demand, &uses)
            };
            if let Some(card) = self.role_cards.get(role).copied() {
                plan.notes.push(format!(
                    "карта роли '{role}' — {card}: бюджет и вытеснение в её пределах (M6 §3.3)"
                ));
            }
            strip_index_self_pause(&mut plan, role);
            plan
        };
        // Применяем решение к движку — но **с бюджетом**: если движок занят чужим
        // вызовом (загрузка/инференс другой роли), вытеснение откладываем. Ждать
        // нельзя: именно ожидание за чужим вызовом превращало запрос в «резидент не
        // отвечает» (`W4_REPORT.md` §14), а запрос без вытеснения обычно проходит —
        // движок сам поднимет роль (`LOAD_ON_DEMAND`) или честно откажет.
        let mut defer_line: Option<String> = None;
        let deferred = match self
            .cl()?
            .try_with("dispatcher", DISPATCH_BUDGET, |cluster| {
                dispatch::apply(cluster, &self.pause, &plan)
            }) {
            Ok(log) => {
                for line in plan.lines().iter().chain(log.iter()) {
                    self.log.line(&format!("[dispatcher] {line}"));
                }
                false
            }
            Err(busy) => {
                defer_line = Some(format!("движок занят ({busy}) — вытеснение отложено"));
                self.log.line(&format!(
                    "[dispatcher] {}",
                    defer_line.as_deref().unwrap_or("")
                ));
                true
            }
        };
        let verdict_str = plan.verdict.as_str().to_string();
        if let Ok(mut d) = self.decisions.lock() {
            d.push(format!("{role}: {verdict_str}"));
            d.extend(plan.lines());
            if let Some(why) = &defer_line {
                d.push(format!("[deferred] {why}"));
            }
        }
        // Verdict::Unknown («замера нет — решает движок») — это пропуск запроса,
        // а не отказ: план уже содержит EnsureLoaded. Раньше Unknown трактовался
        // как «не хватает» и отклонялся с вводящим в заблуждение «свободно ?» —
        // на машинах без NVML (AMD/Intel) автотранскрибация не проходила вовсе
        // (живой инцидент 05.10.2026).
        let verdict_ok = plan.verdict.is_ok() || matches!(plan.verdict, Verdict::Unknown);
        if let Ok(mut last) = self.last_plan.lock() {
            *last = Some(plan);
        }
        if !verdict_ok && !deferred {
            // Паузу мы поставили «под запрос» (действие PauseIndex), но запрос
            // отклонён — снимаем её здесь же: иначе неудавшийся запрос оставит
            // индексацию стоящей навсегда (R30: «индексация встала»). Чужую паузу
            // `resume` не трогает — только нашу.
            let _ = self.pause.resume();
            return Err(EngineError::Other(format!(
                "не хватает VRAM для роли '{role}': нужно {need} МиБ, свободно {}; \
                 вытеснение не помогло — авто-деградации нет (gpu.model_policy: {})",
                self.free_for_role(role)
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                self.cfg.model_policy
            )));
        }
        if !verdict_ok {
            // Вытеснение не выполнено (движок занят), а вердикт «не хватает» — это наша
            // оценка, а не приговор: движок умеет выгружать сам. Отдаём запрос как есть —
            // откажет, и клиент получит честную ошибку движка.
            self.log.line(&format!(
                "[dispatcher] вердикт {verdict_str}: вытеснение не выполнено — запрос \
                 '{role}' отдаём движку как есть"
            ));
        }
        Ok(self.pause.lease(&format!("запрос роли {role}")).ok())
    }

    /// Базовый URL внешнего владельца роли (режим `llm_server.mode: facade`).
    fn upstream_for(&self, role: &str) -> Option<String> {
        if self.mode != Mode::Facade {
            return None;
        }
        self.upstream
            .get(role)
            .or_else(|| self.upstream.get("*"))
            .filter(|u| !u.trim().is_empty())
            .cloned()
    }

    /// Роли, которыми управляет фасад (есть инстанс или upstream).
    fn known_roles(&self) -> Vec<String> {
        let mut v: Vec<String> = self.ids.keys().cloned().collect();
        for r in self.upstream.keys() {
            if r != "*" && !v.contains(r) {
                v.push(r.clone());
            }
        }
        v
    }

    /// Снимок состояния резидента для heartbeat-файла (L1, `W4_REPORT.md` §15).
    ///
    /// Собирается **без** обращения к движку: занятость читается с гейта
    /// ([`ClusterShared::busy`]), VRAM — из NVML плюс атрибуция по процессам (PDH).
    /// Именно поэтому heartbeat обновляется, даже когда движок занят и HTTP не отвечает.
    pub fn heartbeat(&self, pid: u32, uptime_sec: u64, ts_unix: u64) -> resident::Heartbeat {
        let used = self
            .vram_probe
            .as_ref()
            .and_then(|p| p.snapshot())
            .map(|s| s.used_mib)
            .unwrap_or(0);
        let attribution = crate::gpuattr::attribution_for_current_process(used);
        let busy = self
            .cluster
            .as_ref()
            .and_then(|cl| cl.busy())
            .map(|b| b.to_string());
        let (last_slow_what, last_slow_ms) =
            match self.cluster.as_ref().and_then(|cl| cl.last_slow()) {
                Some((what, ms)) => (Some(what), Some(ms)),
                None => (None, None),
            };
        let has_vram = used > 0;
        resident::Heartbeat {
            pid,
            uptime_sec,
            ts_unix,
            busy,
            last_slow_what,
            last_slow_ms,
            vram_ours_mib: has_vram.then_some(attribution.ours_mib),
            vram_foreign_mib: has_vram.then_some(attribution.foreign_mib),
            vram_total_used_mib: has_vram.then_some(attribution.total_used_mib),
        }
    }

    /// Поток heartbeat резидента: раз в [`resident::HEARTBEAT_EVERY_SECS`] пишет
    /// `data/llm-host.heartbeat.json` (pid, uptime, занятость движка, атрибуция VRAM).
    ///
    /// Зачем отдельный поток: при занятом движке фасад может не ответить, и без файла
    /// внешний наблюдатель (UI, `hds check`, `llm-host status`) остаётся с диагнозом
    /// «резидент не отвечает» — ровно то, что мешало в инциденте §14.
    pub fn spawn_heartbeat(
        self: &Arc<Self>,
        path: PathBuf,
        stop: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            let step = Duration::from_millis(250);
            let every = Duration::from_secs(resident::HEARTBEAT_EVERY_SECS);
            let mut reported_busy = false;
            loop {
                let hb = me.heartbeat(me.pid, me.started.elapsed().as_secs(), resident::unix_now());
                if let Err(e) = hb.write(&path) {
                    me.log
                        .note(&format!("heartbeat: не записал {}: {e}", path.display()));
                }
                // Переходы «занят ↔ свободен» дублируем в лог: файла для разбора
                // инцидента мало (его перезаписывает следующий снимок).
                match (&hb.busy, reported_busy) {
                    (Some(what), false) => {
                        me.log.line(&format!(
                            "[busy] движок занят: {what} (см. heartbeat {})",
                            path.display()
                        ));
                        reported_busy = true;
                    }
                    (None, true) => {
                        me.log.line("[busy] движок свободен");
                        reported_busy = false;
                    }
                    _ => {}
                }
                // Страж брошенных запросов: «engine — N с» дольше порога = генерация,
                // чей клиент уже ушёл. Штатная остановка резидента освобождает GPU.
                if me.abandoned_request_sec > 0 {
                    if let Some(b) = me.cl().ok().as_ref().and_then(|c| c.busy()) {
                        if abandoned_request_guard(&b, "engine", me.abandoned_request_sec) {
                            me.log.line(&format!(
                                "[guard] вызов движка «engine» держится {} с ≥ порога {} с \
                                 — клиент, скорее всего, оборвал запрос; ШТАТНАЯ остановка \
                                 резидента для освобождения GPU (автозапуск поднимет заново)",
                                b.secs(),
                                me.abandoned_request_sec
                            ));
                            // Result осознанно не проверяем: страж повторит попытку
                            // на следующем такте, если остановка не прошла.
                            let _ = me.internal_stop();
                        }
                    }
                }
                // сон мелкими кусками: остановка не должна ждать такт целиком
                let mut slept = Duration::ZERO;
                while slept < every && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(step);
                    slept += step;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
            }
            me.log
                .line("heartbeat: поток остановлен (файл оставлен как последний снимок)");
        })
    }

    /// Поток фонового арбитра: ARB-3 (живой прогон индексации и нехватка VRAM →
    /// разрешено выгрузить резидента-чат) и ARB-5 (простой дольше `gpu.evict_idle_sec`).
    ///
    /// Зачем отдельный поток: решения «в запросе» принимает `prepare`, но эти два
    /// сценария к запросам не привязаны — индексация идёт сама, а роль простаивает
    /// между запросами. Такт — раз в 15 с (компромисс: не мешать движку и успевать
    /// реагировать до того, как чужая память понадобится).
    ///
    /// Ничего не делает при `gpu.policy: manual` и когда `gpu.evict_idle_sec = 0`
    /// (решения остаются за оператором) — это проверяет сама `dispatch`.
    /// W3 ARB-5: выгрузить whisper-транскрибатор по простою — освободить VRAM.
    ///
    /// Транскрибатор не является инстансом кластера (bridge-API), поэтому арбитр
    /// следит за ним отдельно; пересоздаётся лениво при следующем запросе.
    /// При `gpu.policy: manual` или `evict_idle_sec = 0` не трогаем (за оператором).
    fn whisper_idle_evict(&self) {
        if self.gpu.policy == crate::config::GpuPolicy::Manual || self.gpu.evict_idle_sec == 0 {
            return;
        }
        // §6 (T3.1): идёт задание автотранскрибации — предохранитель простоя не
        // должен выгружать транскрибатор прямо во время работы.
        if self.transcribe_active.load(Ordering::Relaxed) {
            return;
        }
        if let Some(sec) = self.idle_map().get("whisper").copied() {
            if sec >= self.gpu.evict_idle_sec {
                let mut map = self.whisper.lock().unwrap();
                let cards: Vec<i32> = map.keys().copied().collect();
                if !cards.is_empty() {
                    map.clear();
                    self.log.line(&format!(
                        "[arbiter/idle] whisper: простой {sec} с ≥ {} — выгружены \
                         транскрибаторы карт {cards:?} (VRAM освобождена)",
                        self.gpu.evict_idle_sec
                    ));
                }
            }
        }
    }

    pub fn spawn_arbiter(self: &Arc<Self>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            let interval = Duration::from_secs(15);
            // сон мелкими кусками: остановка хоста не должна ждать такт целиком
            // (иначе `llm-host stop` висит до 15 с — поймано тестом `host_resident`)
            let step = Duration::from_millis(250);
            let mut slept = Duration::ZERO;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(step);
                slept += step;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if slept >= interval {
                    slept = Duration::ZERO;
                    me.arbiter_tick();
                }
            }
        })
    }

    /// Один такт арбитра (без сна — чтобы вызывать и из тестов/CLI).
    pub fn arbiter_tick(&self) {
        if self.cl().is_err() {
            return; // режимы facade/off: инстансов нет
        }
        // W3 ARB-5: простой whisper → выгружаем транскрибатор (VRAM возвращается)
        self.whisper_idle_evict();
        let heartbeat = read_heartbeat(&self.pause_dir);
        let indexing_live = heartbeat.as_ref().map(|h| h.is_live()).unwrap_or(false);
        let mut plans: Vec<(String, Plan)> = Vec::new();

        // ARB-5: простой роли (свой предохранитель + grace роли)
        //
        // M6 T3 §3.3: на машине с раскладкой по картам предохранитель работает
        // **в каждой карте отдельно** — роли другой карты не выгружаем чужой
        // нехваткой VRAM. Карты неизвестны (нет раскладки) — поведение прежнее.
        let uses = self.uses();
        let cards = dispatch::cards_in_use(&uses);
        if cards.is_empty() {
            let idle_actions = dispatch::idle_evictions(&self.gpu, &uses);
            if !idle_actions.is_empty() {
                let mut plan = Plan::new(self.free_mib(), 0, self.gpu.reserve_mb);
                plan.actions = idle_actions;
                plans.push(("idle".to_string(), plan));
            }
        } else {
            for card in cards {
                let on_card = dispatch::on_card(&uses, Some(card));
                let idle_actions = dispatch::idle_evictions(&self.gpu, &on_card);
                if idle_actions.is_empty() {
                    continue;
                }
                let mut plan = Plan::new(self.card_free_mib(card), 0, self.gpu.reserve_mb);
                plan.notes.push(format!(
                    "карта {card}: предохранитель простоя — только её роли (M6 §3.3)"
                ));
                plan.actions = idle_actions;
                plans.push((format!("idle:{card}"), plan));
            }
        }

        // ARB-3: идёт индексация, памяти не хватает — резидент (chat) уступает ей
        if indexing_live {
            let need = self.needs.get("embedding").copied().unwrap_or(0);
            if need > 0 {
                // Индексация — на карте роли `embedding` (M6 §3.3/§3.5): бюджет и
                // вытеснение в её пределах; нет раскладки — прежнее поведение.
                let card = self.role_cards.get("embedding").copied();
                let uses_on = dispatch::on_card(&uses, card);
                let plan = dispatch::plan_indexing(
                    &self.gpu,
                    self.free_for_role("embedding"),
                    &Demand::new("embedding", need),
                    &uses_on,
                );
                if !plan.actions.is_empty() {
                    plans.push(("indexing".to_string(), plan));
                }
            }
        }

        for (why, plan) in plans {
            // Арбитр не должен залипать на чужом вызове: занят движок — пропускаем такт
            // (в §14 арбитр ждал за зависшей загрузкой и не мог вытеснить ничего).
            let log = match self.cl() {
                Ok(cl) => match cl.try_with("arbiter", ARBITER_BUDGET, |cluster| {
                    dispatch::apply(cluster, &self.pause, &plan)
                }) {
                    Ok(log) => log,
                    Err(busy) => vec![format!("движок занят ({busy}) — вытеснение отложено")],
                },
                Err(_) => Vec::new(),
            };
            for line in plan.lines().iter().chain(log.iter()) {
                self.log.line(&format!("[arbiter/{why}] {line}"));
            }
            if let Ok(mut d) = self.decisions.lock() {
                d.push(format!("[arbiter/{why}] {}", plan.verdict.as_str()));
                d.extend(plan.lines());
            }
            if let Ok(mut last) = self.last_plan.lock() {
                *last = Some(plan);
            }
        }
    }
}

impl Backend for ClusterBackend {
    fn chat(&self, req: &ChatRequest) -> Result<(String, Usage)> {
        // M4 T2: чат — KEEP_LOADED и в типичном случае уже загружен. `prepare` и
        // `ensure_loaded` берут ЭКСКЛЮЗИВНЫЙ доступ, поэтому при загруженной роли
        // идём сразу в разделяемую генерацию: иначе 5 параллельных RAG вставали
        // строго по очереди, а слоты движка простаивали (замер 08.10.2026,
        // `tools/parity/out/m4_slots_*.json`). Пауза индексации сохраняется —
        // её держит сам lease, без обращения к кластеру.
        //
        // M6 T2 §3.6: наружу один вход (порт 8010), а запрос уходит в свободную
        // реплику — least-busy, при равенстве round-robin (здесь, внутри хоста).
        let role = self.pick_chat_role().unwrap_or_else(|| "chat".to_string());
        let ready = self.role_loaded(&role);
        let _lease = if ready {
            self.pause.lease(&format!("запрос роли {role}")).ok()
        } else {
            self.prepare(&role)?
        };
        let id = if ready {
            self.id_of(&role)?
        } else {
            self.ensure_loaded(&role)?
        };
        let out = self.cl()?.with_shared(|c| {
            c.chat_complete(
                id,
                &req.prompt,
                req.n_predict,
                req.temperature,
                req.reasoning(),
            )
        })?;
        self.note_used(&role);
        if !out.ok {
            return Err(EngineError::Other(format!(
                "чат-инстанс вернул ошибку: {}",
                if out.error.is_empty() {
                    "без текста ошибки"
                } else {
                    &out.error
                }
            )));
        }
        Ok((
            out.text,
            Usage {
                prompt_tokens: out.metrics.prompt_tokens,
                completion_tokens: out.metrics.decoded_tokens,
            },
        ))
    }

    fn embeddings(&self, body_json: &str) -> Result<String> {
        // M4 T2: та же схема, что у чата — при загруженной роли разделяемая
        // генерация (несколько запросов эмбеддингов не выстраиваются в очередь).
        let ready = self.role_loaded("embedding");
        let _lease = if ready {
            self.pause.lease("запрос роли embedding").ok()
        } else {
            self.prepare("embedding")?
        };
        let id = if ready {
            self.id_of("embedding")?
        } else {
            self.ensure_loaded("embedding")?
        };
        let out = self
            .cl()?
            .with_shared(|c| c.embeddings_json(id, body_json, true))?;
        self.note_used("embedding");
        out.ensure_ok()
            .map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(out.json)
    }

    fn rerank(&self, body_json: &str) -> Result<String> {
        // M4 T2: см. `chat`/`embeddings`.
        let ready = self.role_loaded("rerank");
        let _lease = if ready {
            self.pause.lease("запрос роли rerank").ok()
        } else {
            self.prepare("rerank")?
        };
        let id = if ready {
            self.id_of("rerank")?
        } else {
            self.ensure_loaded("rerank")?
        };
        let out = self.cl()?.with_shared(|c| c.rerank_json(id, body_json))?;
        self.note_used("rerank");
        out.ensure_ok()
            .map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(out.json)
    }

    /// `/props` как у `llama-server`. В режиме `facade` спрашиваем сам апстрим:
    /// Python-версия (`hds/llama_server.py::probe`) считает инстанс «своим» именно
    /// по `model_path` из `/props`, поэтому прокси-режим обязан его отдавать.
    fn props(&self, role: &str) -> Option<Value> {
        if let Some(base) = self.upstream_for(role) {
            let url = format!("{}/props", base.trim_end_matches('/'));
            if let Ok((200, json)) =
                crate::http::client_json("GET", &url, None, Duration::from_secs(15))
            {
                return Some(json);
            }
        }
        let id = self.ids.get(role).copied()?;
        // L1: состояние роли читаем с бюджетом — при занятом движке честно сообщаем
        // «busy» (кто держит и сколько), а не висим вместе с ним.
        let (state, busy) = match self.cluster.as_ref() {
            Some(cl) => match cl.try_with("props", OBSERVE_BUDGET, |x| {
                x.instance_by_id(id)
                    .ok()
                    .flatten()
                    .map(|i| i.state_name.clone())
            }) {
                Ok(state) => (state.unwrap_or_default(), Value::Null),
                Err(b) => (String::new(), json!(b.to_string())),
            },
            None => (String::new(), Value::Null),
        };
        Some(json!({
            "model_path": self.model_path.get(role).cloned().unwrap_or_default(),
            "n_ctx": self.n_ctx.get(role).copied().unwrap_or(0),
            "total_slots": self.parallel.max(1),
            "state": state,
            "busy": busy,
        }))
    }

    fn upstream(&self, role: &str) -> Option<String> {
        self.upstream_for(role)
    }

    /// `/internal/status` — готовый `StatusReport` (те же поля, что у разового
    /// `llm_host_status --json`) + человеческие строки + факты о процессе.
    ///
    /// Прогноза здесь нет: резидент знает **фактическое** последнее решение
    /// диспетчера (`decision`), а не «что было бы, если запрос придёт сейчас».
    fn internal_status(&self) -> Result<Value> {
        // L1 (`W4_REPORT.md` §14): статус обязан отвечать быстро. Если движок занят,
        // отдаём пустые списки и строку «кто держит и сколько» вместо ожидания
        // за зависшим вызовом.
        let mut busy: Option<String> = None;
        let devices = match self.cluster.as_ref() {
            Some(cl) => match cl.try_with("status:devices", OBSERVE_BUDGET, |x| x.devices()) {
                Ok(Ok(devices)) => devices,
                Ok(Err(e)) => {
                    self.log.note(&format!("[internal] devices: {e}"));
                    Vec::new()
                }
                Err(b) => {
                    busy = Some(b.to_string());
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let instances = match self.cluster.as_ref() {
            Some(cl) => match cl.try_with("status:instances", OBSERVE_BUDGET, |x| x.instances()) {
                Ok(Ok(instances)) => instances,
                Ok(Err(e)) => {
                    self.log.note(&format!("[internal] instances: {e}"));
                    Vec::new()
                }
                Err(b) => {
                    busy = Some(b.to_string());
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        let vram = self.vram_probe.as_ref().and_then(|p| p.snapshot());
        let decision = self.last_plan.lock().ok().and_then(|p| p.clone());
        let report = StatusReport::build(StatusInput {
            config: &self.cfg,
            runtime_root: &self.runtime_root,
            engine_dir: self.engine_dir.as_deref(),
            devices: &devices,
            instances: &instances,
            vram,
            vram_source: self.cfg.gpu.vram_source,
            baseline_used_mib: self.baseline_used_mib,
            paused: self.pause.is_paused(),
            pause_file: self.pause.path(),
            heartbeat: read_heartbeat(&self.pause_dir),
            planned: &self.planned,
            needs: &self.needs,
            decision: decision.as_ref(),
            forecast: None,
        });
        let mut json = report.json();
        if let Value::Object(map) = &mut json {
            // M6 T3 §3.3: VRAM по картам — «карта, бэкенд, свободно, роли».
            let cards = cards_lines(&devices, &self.role_cards, &|c| self.card_free_mib(c));
            let mut lines = report.lines();
            lines.extend(cards.iter().cloned());
            map.insert("lines".to_string(), json!(lines));
            map.insert("cards".to_string(), json!(cards));
            map.insert("mode".to_string(), json!(self.mode.as_str()));
            map.insert("pid".to_string(), json!(self.pid));
            map.insert(
                "uptime_sec".to_string(),
                json!(self.started.elapsed().as_secs()),
            );
            map.insert(
                "pid_file".to_string(),
                json!(self.pid_path.display().to_string()),
            );
            map.insert(
                "log_file".to_string(),
                json!(self.log_path.display().to_string()),
            );
            map.insert(
                "dispatcher_enabled".to_string(),
                json!(self.dispatch_enabled),
            );
            map.insert(
                "instances_by_role".to_string(),
                json!(self
                    .ids
                    .iter()
                    .map(|(r, id)| format!("{r}={id}"))
                    .collect::<Vec<_>>()),
            );
            map.insert(
                "dispatcher_log".to_string(),
                json!(self.decisions.lock().map(|d| d.clone()).unwrap_or_default()),
            );
            // L1: наблюдаемость движка — занят ли он, кем и что было долгим вызовом.
            map.insert(
                "engine_busy".to_string(),
                busy.map_or(Value::Null, |b| json!(b)),
            );
            map.insert(
                "engine_last_slow".to_string(),
                self.cluster
                    .as_ref()
                    .and_then(|cl| cl.last_slow())
                    .map_or(Value::Null, |(what, ms)| json!({ "what": what, "ms": ms })),
            );
        }
        Ok(json)
    }

    /// `/internal/devices` — устройства движка (bridge-индексы, память, бэкенд).
    fn internal_devices(&self) -> Result<Value> {
        let cl = self.cl()?;
        let devices = cl
            .try_with("devices", OBSERVE_BUDGET, |c| c.devices())
            .map_err(busy_err)??;
        Ok(json!({
            "lines": devices.iter().map(device_line).collect::<Vec<_>>(),
            "devices": devices
                .iter()
                .map(|d| json!({
                    "bridge_device_index": d.bridge_device_index,
                    "backend": d.backend,
                    "name": d.name,
                    // M6 T1: описание устройства (имя GPU) — по нему пункт `gpu_roles`
                    // в `hds check` определяет вендора и iGPU/UMA.
                    "description": d.description,
                    "memory_free_mib": d.memory_free_mib(),
                    "memory_total_mib": d.memory_total as f64 / (1024.0 * 1024.0),
                }))
                .collect::<Vec<_>>(),
        }))
    }

    /// `/internal/load` — поднять роль (инстансы создаются при старте `llm-host`).
    ///
    /// Перед загрузкой спрашиваем диспетчер (`prepare`): он поставит паузу индексации,
    /// вытеснит простые роли по приоритетам и, если памяти всё равно нет, вернёт
    /// понятный отчёт вместо невнятной ошибки движка (`invalid vector subscript`
    /// при нехватке VRAM — наблюдено живым прогоном, §9.10 `W2_REPORT.md`).
    fn internal_load(&self, role: &str) -> Result<Value> {
        let known = self.known_roles();
        let id = self.id_of(role).map_err(|_| {
            EngineError::Other(format!(
                "роль '{role}' не поднята в этом процессе; известные роли: {}",
                if known.is_empty() {
                    "нет".to_string()
                } else {
                    known.join(", ")
                }
            ))
        })?;
        let _lease = self.prepare(role)?;
        let cl = self.cl()?;
        cl.try_with(&format!("load:{role}"), LOAD_STEP_BUDGET, |c| c.load(id))
            .map_err(busy_err)??;
        let inst =
            cl.wait_loaded_stepwise(&format!("wait_loaded:{role}"), id, Duration::from_secs(300))?;
        self.note_used(role);
        self.log.line(&format!(
            "[internal] роль '{role}': загружена ({}), id={id}",
            inst.state_name
        ));
        Ok(json!({ "role": role, "id": id, "state": inst.state_name, "loaded": inst.is_loaded() }))
    }

    /// `/internal/unload` — выгрузить роль, не удаляя инстанс (вернётся по запросу).
    fn internal_unload(&self, role: &str) -> Result<Value> {
        let cl = self.cl()?;
        let id = self.id_of(role)?;
        cl.try_with(&format!("unload:{role}"), OBSERVE_BUDGET, |c| c.unload(id))
            .map_err(busy_err)??;
        let state = cl
            .with(|c| {
                c.instance_by_id(id)
                    .ok()
                    .flatten()
                    .map(|i| i.state_name.clone())
            })
            .unwrap_or_default();
        self.log.line(&format!(
            "[internal] роль '{role}': выгружена ({state}), id={id}"
        ));
        Ok(json!({ "role": role, "id": id, "state": state, "loaded": false }))
    }

    /// `/internal/stop` — попросить процесс завершиться: `Host::wait` увидит флаг и
    /// выполнит обычную уборку (`facade::serve` → stop → снятие инстансов →
    /// отпускание своей паузы → освобождение pid-файла).
    fn internal_stop(&self) -> Result<Value> {
        self.stop.store(true, Ordering::Relaxed);
        self.log
            .line("[internal] получена команда stop: завершаюсь после ответа");
        Ok(json!({ "stopping": true, "pid": self.pid }))
    }

    /// W3 + T1 (PLAN_AUTO_TRANSCRIBE): транскрибация аудио/видео через bridge-API
    /// движка (роль `whisper`).
    ///
    /// Тело: `{"path": ..., "mode": "subtitle|speech|transcript", "custom": "4.5",
    /// "gpu": 0, "model": ...}`. Транскрибатор создаётся лениво и переиспользуется.
    ///
    /// Для автотранскрибации (§5.1) добавляются `{"diarization": true,
    /// "diarization_model": "...gguf", "diarization_backend": "sortformer",
    /// "diarization_feed_ms": 10800001, "return_text": true}`.
    ///
    /// W4 (`PLAN_DIARIZATION_NEMOTRON.md`): backend выбирается телом
    /// `diarization_backend` → конфигом `diarization.backend` → авто (решение №10).
    /// * `nemotron3` — свой путь ([`Self::transcribe_nemotron3`]): ffmpeg → полный WAV →
    ///   ASR по срезу → диаризация по полному WAV → `.md`; sortformer-модель не нужна;
    /// * `sortformer` — прежний путь движка без изменений (`mode: transcript`);
    /// * `off` — обычный ASR без меток (`transcript` превращается в `speech`).
    ///
    /// `return_text: true` добавляет в ответ `text` — полный текст вывода (для
    /// `transcript`/`nemotron3` это `.md` с метками `SPEAKER_NN`/`UNASSIGNED`).
    fn internal_transcribe(&self, body: &Value) -> Result<Value> {
        let engine_dir = self.engine_dir.as_ref().ok_or_else(|| {
            EngineError::Other("движок не загружен — транскрибация недоступна".to_string())
        })?;
        let path = body
            .get("path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| EngineError::Other("нет поля path".to_string()))?;
        let mode = body
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("subtitle");
        let custom = body
            .get("custom")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "4.5".to_string());
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(default_whisper_model)
            .ok_or_else(|| {
                EngineError::Other("не найдена whisper-модель (index.whisper_model)".to_string())
            })?;

        // §5.1: диаризация включается только явным флагом; `return_text` — просьба
        // вернуть полный текст вывода (для `transcript` — `.md` с метками спикеров).
        let diarization = body
            .get("diarization")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let return_text = body
            .get("return_text")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // W1: язык ASR. Тело `language` ("" = авто) → дефолт продукта `ru`.
        // Без фиксации языка whisper выдаёт английские галлюцинации на тишине
        // (`DIARIZATION_NEMOTRON_HANDOFF.md` п. 5, спайк §8).
        let language = language_from_body(body);
        // W4: backend диаризации. Тело `diarization_backend` сильнее конфига
        // (решение владельца 21.10.2026), иначе — `diarization.backend`, а при
        // отсутствии секции — авто: nemotron3 при валидной модели, иначе
        // sortformer (решение №10).
        let diar_backend = self.pick_diar_backend(body);

        // Бюджет VRAM (критерий приёмки W3): не создавать whisper на GPU, если
        // «модель + буфер + резерв» не влезает — тогда CPU-fallback с сообщением.
        //
        // M6 T2 §3.5/§3.6: карту под файл выбирает **хост** по свободной VRAM
        // (числа — в логе); `whisper`/`whisper-2`/… дают по экземпляру на карту,
        // один файл — одна карта (split отвергнут). Явный `gpu` в теле запроса
        // (`index.whisper_gpu`/`auto_transcribe.whisper_gpu`) сильнее выбора.
        let model_mib = std::fs::metadata(&model)
            .map(|m| m.len() >> 20)
            .unwrap_or(0);
        let explicit_gpu = body.get("gpu").and_then(|v| v.as_i64()).map(|v| v as i32);
        let mut gpu = match explicit_gpu {
            Some(v) => v,
            None => self.pick_whisper_card(model_mib),
        };
        let (dev, fallback) =
            whisper_device(gpu, model_mib, self.gpu.reserve_mb, self.card_free_mib(gpu));
        if let Some(reason) = fallback {
            self.log.line(&format!(
                "[whisper] {reason} — транскрибация на CPU (VRAM-бюджет)"
            ));
        }
        gpu = dev;

        // Один транскрибатор на карту (клетки в `Arc`: держим блокировку только
        // своей карты, поэтому разные файлы на разных картах идут параллельно).
        let cell: Arc<WhisperCell> = {
            let mut map = self.whisper.lock().unwrap();
            match map.get(&gpu) {
                Some(existing) => Arc::clone(existing),
                None => {
                    self.log.line(&format!(
                        "[whisper] создаю транскрибатор: модель {} ({})",
                        model.display(),
                        if gpu >= 0 {
                            format!("bridge-карта {gpu}")
                        } else {
                            "CPU".to_string()
                        }
                    ));
                    let api = crate::bridge_audio::BridgeAudio::load(engine_dir)?;
                    let w = crate::whisper::Whisper::new(api, &model, gpu, -1)?;
                    let cell = Arc::new(WhisperCell(Mutex::new(w)));
                    map.insert(gpu, Arc::clone(&cell));
                    cell
                }
            }
        };
        // Диаризацию собираем после финального решения об устройстве whisper: по
        // умолчанию отдаём движку имя его устройства (как референс — `CUDA0`).
        // W4: sortformer — только когда backend выбран явно/авто как sortformer;
        // Nemotron-3 идёт своим путём (модель движка не нужна, VRAM не занимает).
        if diarization {
            match diar_backend {
                DiarBackend::Sortformer => self
                    .log
                    .line("[diar] backend=sortformer (legacy) — путь движка без изменений"),
                DiarBackend::Off => self
                    .log
                    .line("[diar] backend=off — обычный ASR без меток спикеров"),
                DiarBackend::Nemotron3 => {}
            }
        }
        let diar = if diarization && diar_backend == DiarBackend::Sortformer {
            let diar_model = body
                .get("diarization_model")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .or_else(default_diarization_model)
                .ok_or_else(|| {
                    EngineError::Other(
                        "диаризация включена, но sortformer-модель не найдена: задайте \
                         auto_transcribe.diarization_model или установите модель \
                         (installers\\fetch_diarization_model.ps1). Fallback в speech запрещён \
                         (PLAN_AUTO_TRANSCRIBE §0 п.6)."
                            .to_string(),
                    )
                })?;
            if !diar_model.is_file() {
                return Err(EngineError::Other(format!(
                    "sortformer-модель не найдена: {}",
                    diar_model.display()
                )));
            }
            let backend = body
                .get("diarization_backend")
                .and_then(|v| v.as_str())
                .unwrap_or("sortformer");
            let feed_ms = body
                .get("diarization_feed_ms")
                .and_then(|v| v.as_f64())
                .unwrap_or(10_800_001.0);
            let device = body
                .get("diarization_device")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| self.engine_device_name(gpu));
            let mut d = DiarizationParams::new(&diar_model);
            d.backend = backend.to_string();
            d.feed_ms = feed_ms;
            d.device = device;
            self.log.line(&format!(
                "[transcribe] диаризация: {} (backend {backend}, feed_ms {feed_ms}, device {})",
                diar_model.display(),
                d.device.as_deref().unwrap_or("авто")
            ));
            Some(d)
        } else {
            None
        };

        // §6 (T3.1): задание автотранскрибации — приоритет 2 из 3. Пока оно идёт:
        // * индексация на паузе (`index.pause` — аренда ниже);
        // * при нехватке VRAM первыми уступают индексные роли, затем чат;
        // * индексация и предохранитель простоя не выбивают транскрибатор (флаг).
        let _flag = TranscribeGuard::set(&self.transcribe_active);
        let need_mib = transcribe_need_mib(&model, diar.as_ref().map(|d| d.model_path.as_path()));
        let _lease = self.prepare_demand(config::TRANSCRIBE_ROLE, need_mib, true)?;
        self.log.line(&format!(
            "[transcribe] задание: нужно ≈{need_mib} МиБ VRAM, приоритет роли '{}' = {}",
            config::TRANSCRIBE_ROLE,
            self.gpu.priority_of(config::TRANSCRIBE_ROLE)
        ));

        self.log.line(&format!(
            "[transcribe] язык ASR: {}",
            language.as_deref().unwrap_or("авто")
        ));
        // W4: Nemotron-3 — свой путь (декод один раз, ASR по срезу, диаризация по
        // полному WAV). Ветка sortformer ниже остаётся нетронутой (регресс).
        if diarization && diar_backend == DiarBackend::Nemotron3 {
            return self.transcribe_nemotron3(
                &cell,
                path,
                mode,
                &custom,
                language.as_deref(),
                return_text,
            );
        }
        // `off` + `mode: transcript`: движку нельзя отдавать `transcript` (он
        // потребует sortformer-модель), поэтому просим сплошной текст.
        let engine_mode = if diarization && diar_backend == DiarBackend::Off && mode == "transcript"
        {
            "speech"
        } else {
            mode
        };
        let w = cell.0.lock().unwrap();
        let tr = w.transcribe_file(
            Path::new(path),
            engine_mode,
            &custom,
            diar.as_ref(),
            language.as_deref(),
        )?;
        drop(w);
        // активность роли whisper — для вытеснения по простою (ARB-5, W3)
        self.note_used("whisper");
        let segments: Vec<Value> = tr
            .segments
            .iter()
            .map(|s| json!({ "text": s.text, "t_start": s.t_start, "t_end": s.t_end }))
            .collect();
        // `backend` — аддитивный ключ (старые клиенты его не читают).
        let mut diar_info = json!({ "enabled": diar.is_some() });
        if diarization {
            diar_info["backend"] = json!(diar_backend.as_str());
        }
        let mut out = json!({
            "path": path,
            "mode": mode,
            "custom": custom,
            "out_ext": tr.out_ext,
            "diarization": diar_info,
            "segments": segments,
            "stats": tr.json.get("stats").cloned().unwrap_or(Value::Null),
        });
        if return_text {
            out["text"] = json!(tr.raw_text);
        }
        Ok(out)
    }
}

/// W4: выбор backend и nemotron3-ветка — это данные **живой машины**
/// (`ClusterBackend`), а не ответы фасада, поэтому отдельный `impl`.
impl ClusterBackend {
    /// Backend диаризации для запроса: тело → конфиг → авто (решение владельца).
    ///
    /// Тело `diarization_backend` сильнее конфига — так `hds-index` может просить
    /// конкретный backend на один запрос, не меняя конфиг. Неизвестное значение
    /// не ломает задание: предупреждение в лог и решение по умолчанию.
    fn pick_diar_backend(&self, body: &Value) -> DiarBackend {
        let raw = body
            .get("diarization_backend")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match raw {
            Some(v) => match DiarBackend::parse(v) {
                Some(b) => b,
                None => {
                    let fallback = self.resolved_diar_backend();
                    self.log.note(&format!(
                        "[diar] diarization_backend '{v}' неизвестен (nemotron3|sortformer|off) — \
                         беру {}",
                        fallback.as_str()
                    ));
                    fallback
                }
            },
            None => self.resolved_diar_backend(),
        }
    }

    /// Backend по умолчанию: конфиг, а если секции нет — авто (решение №10).
    ///
    /// Авто: `nemotron3`, когда файл модели есть **и** проходит sha256; иначе
    /// `sortformer` + предупреждение в лог. Решение кэшируется (`OnceLock`),
    /// поэтому 400 МБ хешируются не чаще одного раза за процесс.
    fn resolved_diar_backend(&self) -> DiarBackend {
        *self.diar_auto.get_or_init(|| {
            if let Some(b) = self.diar.backend {
                return b;
            }
            let cfg = self.diar.nemotron_config();
            let ok = !cfg.model.as_os_str().is_empty()
                && cfg.model.is_file()
                && hds_diarize::verify_model(&cfg.model).is_ok();
            if ok {
                self.log.line(&format!(
                    "[diar] секция diarization не задана: backend=nemotron3 (модель {} проходит \
                     sha256)",
                    cfg.model.display()
                ));
                DiarBackend::Nemotron3
            } else {
                self.log.note(
                    "[diar] секция diarization не задана, модель Nemotron-3 не найдена или не \
                     прошла sha256 — backend=sortformer (legacy)",
                );
                DiarBackend::Sortformer
            }
        })
    }

    /// Диаризация полного WAV моделью Nemotron-3 → turns + имя устройства.
    ///
    /// ORT-сессия живёт в хосте (создаётся при первом задании): загрузка модели —
    /// основная цена старта, а задания идут последовательно. `Mutex` держится на
    /// время прогона — параллельные файлы ждут освобождения сессии.
    fn run_nemotron(
        &self,
        samples: &[i16],
        sample_rate: u32,
    ) -> Result<(Vec<hds_diarize::Turn>, String)> {
        let cfg = self.diar.nemotron_config();
        let mut guard = self.nemotron.lock().unwrap();
        if guard.is_none() {
            self.log.line(&format!(
                "[diar] nemotron3: модель {}, EP {}, блок {:.0} мин, порог {:.2}/{:.2}",
                cfg.model.display(),
                cfg.device.as_str(),
                cfg.chunk_min,
                cfg.onset,
                cfg.offset
            ));
            let d = Diarizer::load(&cfg).map_err(|e| {
                EngineError::Other(format!("модель Nemotron-3 ({}): {e}", cfg.model.display()))
            })?;
            *guard = Some(d);
        }
        let d = guard.as_mut().expect("только что создан");
        let turns = d
            .run_i16(samples, sample_rate)
            .map_err(|e| EngineError::Other(format!("диаризация Nemotron-3: {e}")))?;
        Ok((turns, d.device().as_str().to_string()))
    }

    /// Nemotron-3 (W4): ffmpeg → полный WAV → ASR по срезу → диаризация по
    /// полному WAV → постфильтр → сшивка → `.md`.
    ///
    /// Инварианты варианта A (`PLAN_DIARIZATION_NEMOTRON.md` §3.4):
    /// * декодирование **одно**, этот же WAV идёт в диаризацию;
    /// * обрезка ведущей тишины — только вход ASR (PCM-срез, без второго декода);
    /// * `trim_offset` прибавляется к таймкодам ASR ровно один раз (в
    ///   [`diar::merge_absolute`] и в постфильтре);
    /// * `turns`, `segments[]` ответа, `.md` и `.srt` — сразу в абсолютной шкале.
    #[allow(clippy::too_many_arguments)]
    fn transcribe_nemotron3(
        &self,
        cell: &Arc<WhisperCell>,
        src: &str,
        mode: &str,
        custom: &str,
        language: Option<&str>,
        return_text: bool,
    ) -> Result<Value> {
        let started = Instant::now();
        let ffmpeg = diar::find_ffmpeg(self.ffmpeg.as_deref()).ok_or_else(|| {
            EngineError::Other(
                "диаризация nemotron3: не найден ffmpeg (index.ffmpeg_cmd → PATH → стандартные \
                 каталоги); установите ffmpeg"
                    .to_string(),
            )
        })?;
        let tmp = diar::TempDir::new("hds_nemotron")
            .map_err(|e| EngineError::Other(format!("временный каталог задания: {e}")))?;
        let full = tmp.path().join("full.wav");

        // [decode] один проход: этот WAV — эталон шкалы для ASR и диаризации.
        let t0 = Instant::now();
        decode_to_wav(&ffmpeg, Path::new(src), &full)
            .map_err(|e| EngineError::Other(format!("декодирование {src}: {e}")))?;
        let decode_sec = t0.elapsed().as_secs_f64();
        let wav =
            read_wav(&full).map_err(|e| EngineError::Other(format!("{}: {e}", full.display())))?;
        self.log.line(&format!(
            "[diar] декод: {:.1} с аудио за {decode_sec:.2} с ({})",
            wav.samples.len() as f64 / wav.sample_rate as f64,
            ffmpeg.display()
        ));

        // [trim] ведущая тишина — только для ASR (диаризация её не видит).
        let t0 = Instant::now();
        let rate = wav.sample_rate;
        let total_samples = wav.samples.len();
        let cut = if self.diar.asr.trim_leading_silence {
            leading_silence_samples(&wav.samples, rate, &self.diar.trim_config())
        } else {
            0
        };
        let trim_offset = cut as f64 / rate as f64;
        let asr_input = if cut == 0 {
            full.clone()
        } else {
            let p = tmp.path().join("asr_trim.wav");
            write_wav(&p, &wav.samples[cut..], rate)
                .map_err(|e| EngineError::Other(format!("срез WAV: {e}")))?;
            p
        };
        self.log.line(&format!(
            "[diar] обрезка ASR: {trim_offset:.3} с ведущей тишины, срез {:.1} с ({:.2} с)",
            total_samples.saturating_sub(cut) as f64 / rate as f64,
            t0.elapsed().as_secs_f64()
        ));
        // W7 (RSS ≤ 1.5 ГБ на 77 мин): на время ASR полный PCM не держим — его
        // нагрузка (модель и буферы движка) и так доминирует в пике процесса;
        // постфильтр и диаризация читают WAV заново (~0.1 с на 77 мин).
        drop(wav);

        // [asr] движку — срез; режим `subtitle` нужен ради таймкодов.
        let t0 = Instant::now();
        let tr = {
            let w = cell.0.lock().unwrap();
            w.transcribe_file(&asr_input, diar::ASR_MODE_SUBTITLE, custom, None, language)?
        };
        // активность роли whisper — для вытеснения по простою (ARB-5, W3)
        self.note_used("whisper");
        let asr_sec = t0.elapsed().as_secs_f64();
        self.log.line(&format!(
            "[diar] ASR: {} сегментов за {asr_sec:.1} с (язык {})",
            tr.segments.len(),
            language.unwrap_or("авто")
        ));
        // W7: полный WAV нужен постфильтру и диаризации — читаем заново.
        let t0 = Instant::now();
        let wav =
            read_wav(&full).map_err(|e| EngineError::Other(format!("{}: {e}", full.display())))?;
        self.log.line(&format!(
            "[diar] повторное чтение WAV для постфильтра и диаризации: {:.2} с",
            t0.elapsed().as_secs_f64()
        ));
        // [filter] «реплики из тишины» — по полному WAV, offset один раз.
        let filtered = if self.diar.asr.drop_silent_segments {
            drop_silent_segments(&tr.segments, &wav, trim_offset, &self.diar.filter_config())
        } else {
            FilterOutcome {
                kept: tr.segments.clone(),
                dropped: Vec::new(),
            }
        };
        if !filtered.dropped.is_empty() {
            for s in filtered.dropped.iter().take(5) {
                self.log.line(&format!(
                    "[diar] отброшено (тишина): [{:.2}-{:.2}] {}",
                    s.t_start + trim_offset,
                    s.t_end + trim_offset,
                    s.text
                ));
            }
            self.log.line(&format!(
                "[diar] постфильтр: отброшено {} из {} сегментов",
                filtered.filtered_count(),
                tr.segments.len()
            ));
        }

        // [diar] Nemotron-3 — по полному WAV; сбой при `required: false` не валит
        // задание (пишем `.md` без меток, `diarization.enabled: false`).
        let t0 = Instant::now();
        let (turns, device, diar_error) = match self.run_nemotron(&wav.samples, wav.sample_rate) {
            Ok((turns, device)) => (turns, device, None),
            Err(e) => {
                let msg = e.to_string();
                if self.diar.required {
                    return Err(e);
                }
                self.log.note(&format!(
                    "диаризация nemotron3 не удалась ({msg}) — пишу .md без меток \
                     (diarization.required: false)"
                ));
                (Vec::new(), "нет".to_string(), Some(msg))
            }
        };
        let diar_sec = t0.elapsed().as_secs_f64();
        let num_speakers = diar::speaker_count(&turns);
        self.log.line(&format!(
            "[diar] nemotron3: EP={device}, {num_speakers} спикеров, {} turn'ов, {diar_sec:.1} с",
            turns.len()
        ));

        // [merge] сегменты (шкала среза) + turns (абсолютная) → блоки `.md`.
        let blocks = diar::merge_absolute(&filtered.kept, &turns, trim_offset);
        let text = render_md(&blocks, diar::MD_UNASSIGNED);
        let total_sec = started.elapsed().as_secs_f64();
        let stats = diar::NemotronStats {
            num_speakers,
            num_turns: turns.len(),
            filtered_segments: filtered.filtered_count(),
            timings: diar::NemotronTimings {
                decode: decode_sec,
                asr: asr_sec,
                diar: diar_sec,
                total: total_sec,
            },
        };
        let segments: Vec<Value> = filtered
            .kept
            .iter()
            .map(|s| {
                json!({
                    "text": s.text,
                    "t_start": s.t_start + trim_offset,
                    "t_end": s.t_end + trim_offset,
                })
            })
            .collect();
        self.log.line(&format!(
            "[diar] итог: {} реплик, {num_speakers} спикеров, {total_sec:.1} с",
            blocks.len()
        ));
        let mut diar_info = json!({
            "enabled": diar_error.is_none(),
            "backend": DiarBackend::Nemotron3.as_str(),
            "num_speakers": num_speakers,
        });
        if let Some(err) = &diar_error {
            diar_info["error"] = json!(err);
        }
        let mut out = json!({
            "path": src,
            "mode": mode,
            "custom": custom,
            "out_ext": "md",
            "diarization": diar_info,
            "segments": segments,
            "stats": stats.to_json(tr.json.get("stats")),
        });
        if return_text {
            out["text"] = json!(text);
        }
        Ok(out)
    }
}

/// Записать JSON-отчёт (каталог создаётся) — для бинарей `--json`.
pub fn write_json_file(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| EngineError::Other(format!("json: {e}")))?;
    std::fs::write(path, text)
        .map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
    println!("отчёт: {}", path.display());
    Ok(())
}

/// `--ngl N` для проверочных прогонов: переопределяет офлоад **в плане** (а не в
/// копии спеки при создании), чтобы `status` показывал то, что реально применили.
///
/// При `N = 0` роль переводится на **CPU-устройство**: одного `n_gpu_layers = 0`
/// мало — движок зовёт `llama_params_fit` и может сам офлоаднуть модель обратно
/// (живой прогон 30.09.2026: `offloaded 33/33 layers to GPU`, `CUDA0 model buffer
/// 6306 МиБ` несмотря на `--ngl 0`; см. `W2_REPORT.md` §9.10). Надёжный
/// переключатель устройства — `manual_devices_csv` (находка A1).
fn apply_ngl_override(
    planned: &mut [RolePlan],
    ngl: i32,
    devices: &[crate::cluster::Device],
    log: &Log,
) {
    let cpu = crate::device::cpu_device(devices).map(|d| d.bridge_device_index);
    let mut applied = 0;
    for p in planned.iter_mut() {
        if let RolePlan::Ready(inst) = p {
            if config::role_base(&inst.role) == "whisper" {
                continue; // у аудио-роли свои правила устройства (W3)
            }
            if ngl == 0 {
                if let Some(idx) = cpu {
                    inst.spec.manual_devices_csv = Some(idx.to_string());
                }
                // явный ноль (а не `None`): `status` должен показывать ровно то,
                // что применено, а не «-1 = как в конфиге»
                inst.spec.n_gpu_layers = Some(0);
            } else {
                inst.spec.n_gpu_layers = Some(ngl);
            }
            inst.spec.allow_cpu = Some(true);
            inst.notes.push(format!(
                "проверочный прогон: --ngl {ngl}, устройство {}",
                inst.spec.manual_devices_csv.as_deref().unwrap_or("авто")
            ));
            applied += 1;
        }
    }
    log.line(&format!(
        "проверочный режим: --ngl {ngl} применён к {applied} ролям (устройство: {})",
        if ngl == 0 {
            format!(
                "CPU{}",
                cpu.map(|c| format!(" (bridge index {c})"))
                    .unwrap_or_default()
            )
        } else {
            "как в конфиге".to_string()
        }
    ));
}

/// Порты фасада по ролям конфига (`--port-base` — смещение для проверок).
///
/// Порт берём из `llm_server.<role>.port` (конфиг заказчика), а если он не задан —
/// из умолчаний `llama-server` (`8010/8011/8012`): клиенты (UI, MCP, Hermes)
/// должны попасть туда же, куда ходили раньше.
pub fn port_roles(cfg: &LlmHostConfig, base: Option<u16>) -> Vec<(String, u16)> {
    cfg.roles
        .iter()
        // M6 T2 §3.6: наружу публикуется **один фасад** (chat/embedding/rerank);
        // роли-реплики (`chat-2`, `whisper-2`) — внутренние: их порты только
        // справочные (печатаются в статусе/`hds check`), клиенты ходят на 8010.
        .filter(|rc| !config::is_replica(&rc.role))
        .filter_map(|rc| {
            let port = match base {
                Some(b) => match rc.role.as_str() {
                    "chat" => b,
                    "embedding" => b + 1,
                    "rerank" => b + 2,
                    _ => return None, // whisper в фасад не входит (W3)
                },
                None if rc.port > 0 => rc.port,
                None => facade::default_port(&rc.role),
            };
            if port == 0 {
                return None;
            }
            Some((rc.role.clone(), port))
        })
        .collect()
}

/// Клиентские адреса внутреннего API (`/internal/*`) для CLI: те же порты, что у
/// фасада. Порядок — как в конфиге (сначала `chat`).
pub fn client_ports(cfg: &LlmHostConfig, host: &str, base: Option<u16>) -> Vec<String> {
    port_roles(cfg, base)
        .into_iter()
        .map(|(_, p)| format!("http://{host}:{p}"))
        .collect()
}

/// Значение из YAML по пути `a.b.c` (`chat.*` и прочие ключи, которых нет в
/// `LlmHostConfig`).
pub fn dig<'a>(root: &'a serde_yaml::Value, path: &str) -> Option<&'a serde_yaml::Value> {
    let mut cur = root;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

/// Строка из YAML по пути (`None` — нет ключа или это не строка).
pub fn dig_str(root: &serde_yaml::Value, path: &str) -> Option<String> {
    dig(root, path)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Число из YAML по пути.
pub fn dig_i64(root: &serde_yaml::Value, path: &str) -> Option<i64> {
    dig(root, path).and_then(|v| v.as_i64())
}

/// Дробное из YAML по пути.
pub fn dig_f64(root: &serde_yaml::Value, path: &str) -> Option<f64> {
    dig(root, path).and_then(|v| v.as_f64())
}

/// Базовые URL внешнего владельца для режима `llm_server.mode: facade`.
///
/// Источники (по приоритету): `llm_server.facade_url` — на все роли (`*`),
/// `llm_server.facade.<role>` — на конкретную, иначе штатные клиентские адреса из
/// конфига (`chat.base_url`, `embedding.base_url`, `rerank.url`): они уже указывают
/// на тот же OpenAI-совместимый сервер, что обслуживает проект сегодня.
fn upstream_map(yaml: &serde_yaml::Value, cfg: &LlmHostConfig) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for rc in &cfg.roles {
        if let Some(url) = dig_str(yaml, &format!("llm_server.facade.{}", rc.role)) {
            m.insert(rc.role.clone(), url);
        }
    }
    for (role, path) in [
        ("chat", "chat.base_url"),
        ("embedding", "embedding.base_url"),
        ("rerank", "rerank.url"),
    ] {
        if let Some(url) = dig_str(yaml, path) {
            m.entry(role.to_string()).or_insert(url);
        }
    }
    if let Some(url) = dig_str(yaml, "llm_server.facade_url") {
        m.insert("*".to_string(), url);
    }
    m
}

/// Строка раскладки «роль → карта/backend» (M6 T2 §3.2) для лога/статуса.
///
/// Роли, не поднятые (`RolePlan::Failed`), в строку не входят — о них пишет
/// отдельная строка лога с числами.
/// M6 T3 §3.3: строки VRAM **по картам** — карта, бэкенд, устройство, свободно и
/// какие роли на ней.
///
/// `free` — «сколько свободно на карте» (проба ↔ числа движка): замыкание, чтобы
/// функция осталась чистой и проверялась тестами без движка и GPU.
pub fn cards_lines(
    devices: &[crate::cluster::Device],
    role_cards: &BTreeMap<String, i32>,
    free: &dyn Fn(i32) -> Option<u64>,
) -> Vec<String> {
    let mut cards: Vec<i32> = devices
        .iter()
        .filter(|d| d.is_accelerator())
        .map(|d| d.bridge_device_index)
        .chain(role_cards.values().copied())
        .collect();
    cards.sort_unstable();
    cards.dedup();
    cards
        .into_iter()
        .map(|card| {
            let dev = devices.iter().find(|d| d.bridge_device_index == card);
            let backend = dev
                .map(|d| d.backend.clone())
                .unwrap_or_else(|| "—".to_string());
            let name = dev
                .map(|d| d.description_or_name())
                .unwrap_or_else(|| "—".to_string());
            let mut roles: Vec<String> = role_cards
                .iter()
                .filter(|(_, c)| **c == card)
                .map(|(r, _)| r.clone())
                .collect();
            roles.sort();
            let free_txt = free(card)
                .map(|f| format!("{f} МиБ"))
                .unwrap_or_else(|| "нет замера".to_string());
            format!(
                "карта {card} ({backend}, {name}): свободно {free_txt}, роли: {}",
                if roles.is_empty() {
                    "—".to_string()
                } else {
                    roles.join(", ")
                }
            )
        })
        .collect()
}

pub fn layout_line(planned: &[RolePlan]) -> String {
    let items: Vec<String> = planned
        .iter()
        .filter_map(|p| p.ready())
        .map(|i| {
            format!(
                "{} → карта {} ({})",
                i.role,
                i.bridge_device_index
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                i.device_backend.as_deref().unwrap_or("—")
            )
        })
        .collect();
    if items.is_empty() {
        "нет поднятых ролей".to_string()
    } else {
        items.join(", ")
    }
}

/// Человеческое описание апстримов для лога.
fn describe_upstream(m: &BTreeMap<String, String>) -> String {
    if let Some(url) = m.get("*") {
        return format!("{url} (все роли)");
    }
    m.iter()
        .map(|(r, u)| format!("{r} → {u}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Резидентный `llm-host`: владеет движком, инстансами, фасадом, pid- и лог-файлом.
///
/// Порядок полей важен: при `Drop` они уходят в этом же порядке, поэтому
/// `backend` (вместе с его `Arc<ClusterShared>`) освобождается раньше `cluster`,
/// а `cluster` (в `Drop` — `cluster_destroy`) раньше `engine`.
pub struct Host {
    cfg: HostConfig,
    resolved: LlmHostConfig,
    server: ServerConfig,
    host: String,
    mode: Mode,
    log: Arc<Log>,
    pid: Option<PidFile>,
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
    /// Поток фонового арбитра (ARB-3/ARB-5) — присоединяется в `stop`.
    arbiter: Option<std::thread::JoinHandle<()>>,
    /// Поток heartbeat резидента (L1): `data/llm-host.heartbeat.json`.
    heartbeat: Option<std::thread::JoinHandle<()>>,
    backend: Option<Arc<ClusterBackend>>,
    cluster: Option<Arc<ClusterShared>>,
    engine: Option<Engine>,
    /// Защита «текущий каталог = каталог движка» (`EngineCwd`): держим всю жизнь
    /// хоста, иначе движок перестанет видеть ggml-бэкенды (грабли A1/A2).
    _cwd: Option<EngineCwd>,
    ids: BTreeMap<String, i64>,
    needs: BTreeMap<String, u64>,
    pause: Arc<IndexPause>,
    engine_dir: Option<PathBuf>,
    ports: Vec<(String, u16)>,
    cleanup: Vec<String>,
    started: Instant,
}

impl Host {
    /// Поднять хост: лог → pid-файл → движок и инстансы (по режиму) → фасад.
    ///
    /// Порядок не случаен: pid-файл занимается **до** загрузки движка, чтобы
    /// второй экземпляр падал сразу и не трогал чужие инстансы (защита от второго
    /// владельца GPU), а лог открывается ещё раньше — иначе падение на старте не
    /// оставит следов.
    pub fn start(cfg: HostConfig) -> Result<Host> {
        let started = Instant::now();
        let log = Arc::new(match &cfg.log_file {
            Some(p) => Log::to_file(p).map_err(|e| {
                EngineError::Other(format!("не удалось открыть лог {}: {e}", p.display()))
            })?,
            None => Log::silent(),
        });
        let pid = match &cfg.pid_file {
            Some(p) => Some(PidFile::acquire(p)?),
            None => None,
        };
        if let Some(p) = &pid {
            log.line(&format!(
                "резидентность: pid {} (файл {})",
                p.pid(),
                p.path().display()
            ));
        }
        if let Some(p) = log.path() {
            log.line(&format!("лог: {}", p.display()));
        }

        let resolved = config::load(&cfg.config)?;
        let yaml: serde_yaml::Value =
            serde_yaml::from_str(&std::fs::read_to_string(&cfg.config).map_err(|e| {
                EngineError::Other(format!("конфиг {}: {e}", cfg.config.display()))
            })?)
            .map_err(|e| EngineError::Other(format!("конфиг {}: {e}", cfg.config.display())))?;
        for w in &resolved.warnings {
            log.note(&format!("конфиг: {w}"));
        }
        // W4: секция `diarization` необязательна; опечатка в ней не мешает хосту
        // подняться (жёсткая проверка значений — `hds check`, W5). Решение об
        // автовыборе backend принимается лениво (`resolved_diar_backend`), но
        // логируем его сразу: молчаливый выбор модели — источник сюрпризов.
        let (diar_settings, diar_warnings) = diar::DiarSettings::from_yaml(&yaml);
        for w in &diar_warnings {
            log.note(&format!("диаризация: {w}"));
        }
        if let Some(b) = diar_settings.backend {
            log.line(&format!(
                "[diar] backend={} (конфиг diarization.backend)",
                b.as_str()
            ));
        }
        let ffmpeg = diar::configured_ffmpeg(&yaml);
        // M6 T2 §3.5: нарушение контракта раскладки (реплики индексных ролей,
        // дубли портов) — fail-fast с числами, а не «подняли половину ролей».
        if let Some(note) = resolved.contract_note() {
            log.note(&note);
            return Err(EngineError::Other(note));
        }
        let paths = match &cfg.runtime {
            Some(r) => RuntimePaths::new(r.clone()),
            None => RuntimePaths::from_env()?,
        };
        let host = cfg.host.clone().unwrap_or_else(|| resolved.host.clone());
        let ports = port_roles(&resolved, cfg.port_base);
        let mode = resolved.mode;
        let server = ServerConfig {
            host: host.clone(),
            ports: ports.clone(),
            thinking: cfg.thinking.unwrap_or_else(|| {
                Thinking::parse(
                    &dig_str(&yaml, "chat.thinking").unwrap_or_else(|| "off".to_string()),
                )
            }),
            max_tokens: dig_i64(&yaml, "chat.max_tokens").unwrap_or(600) as i32,
            temperature: dig_f64(&yaml, "chat.temperature").unwrap_or(0.2) as f32,
            internal: cfg.internal,
            ..ServerConfig::default()
        };
        log.line(&format!(
            "конфиг: {} (режим llm_server.mode: {})",
            cfg.config.display(),
            mode.as_str()
        ));
        log.line(&format!(
            "фасад: thinking по умолчанию {}, max_tokens {}, temperature {}",
            server.thinking.as_str(),
            server.max_tokens,
            server.temperature
        ));

        let mut engine = None;
        let mut cwd = None;
        let mut cluster: Option<Arc<ClusterShared>> = None;
        let mut ids: BTreeMap<String, i64> = BTreeMap::new();
        let mut model_path: BTreeMap<String, String> = BTreeMap::new();
        let mut n_ctx_map: BTreeMap<String, i32> = BTreeMap::new();
        let mut planned = Vec::new();
        let mut engine_dir: Option<PathBuf> = None;
        let mut baseline_used_mib: Option<u64> = None;
        // Устройства движка (заполняются в режиме `embedded`) — для привязки проб VRAM.
        let mut engine_devices: Vec<crate::cluster::Device> = Vec::new();
        let upstream = upstream_map(&yaml, &resolved);
        let vram_probe = open_vram_probe(cfg.nvml_index);

        if let Some(p) = &vram_probe {
            if let Some(v) = p.snapshot() {
                log.line(&format!(
                    "VRAM ({}): {} — занято {} / {} МиБ, свободно {} МиБ",
                    p.source().as_str(),
                    p.name(),
                    v.used_mib,
                    v.total_mib,
                    v.free_mib
                ));
                // baseline: наша занятость = измеритель used − это значение (R29)
                baseline_used_mib = Some(v.used_mib);
            }
        } else {
            log.note("измеритель VRAM недоступен (нет ни NVML, ни DXGI): бюджет не проверяется");
        }

        match mode {
            Mode::Off => {
                log.line(
                    "llm_server.mode: off — движок не загружаю, инстансы не создаю \
                     (поиск работает только по FTS; фасад отвечает 503 честной причиной)",
                );
            }
            Mode::Facade => {
                if upstream.is_empty() {
                    return Err(EngineError::Other(
                        "llm_server.mode: facade, но внешний владелец не задан: укажите \
                         llm_server.facade_url (все роли), llm_server.facade.<role> или \
                         штатные адреса chat.base_url / embedding.base_url / rerank.url"
                            .to_string(),
                    ));
                }
                log.line(&format!(
                    "llm_server.mode: facade — своих инстансов не создаю, проксирую как есть на {}",
                    describe_upstream(&upstream)
                ));
            }
            Mode::Embedded => {
                log.line("llm_server.mode: embedded — поднимаю свои инстансы");
                let eng = Engine::open(cfg.engine_dir.as_deref())?;
                let guard = eng.activate()?; // cwd движка обязателен (грабли A1/A2)
                let cls = eng.create_cluster()?;
                log.line(&format!("движок: {}", eng.dir().display()));
                engine_dir = Some(eng.dir().to_path_buf());

                let devices = cls.devices()?;
                log.line(&format!(
                    "устройства движка: {}",
                    describe_devices(&devices)
                ));
                engine_devices = devices.clone();
                planned = registry::plan(&resolved, &paths.root, &devices);
                if let Some(ngl) = cfg.ngl {
                    apply_ngl_override(&mut planned, ngl, &devices, &log);
                }
                // M6 T2 §3.2: раскладка «роль → карта/backend» печатается при
                // старте — это ключевое свидетельство приёмки (роли реально идут
                // на назначенные карты).
                log.line(&format!("раскладка ролей: {}", layout_line(&planned)));
                // M6 T4 §3.4: пресет раскладки/автодетект — с числами (§5,
                // свидетельство приёмки; секретов не печатаем).
                let accel_facts = crate::config::AccelFacts::from_devices(&engine_devices);
                if let Some(note) = resolved.gpu.preset_note(&accel_facts) {
                    log.line(&format!("пресет: {note}"));
                }
                for p in &planned {
                    let inst = match p {
                        RolePlan::Ready(i) => i,
                        RolePlan::Failed { role, error } => {
                            log.note(&format!("роль {role}: не поднята — {error}"));
                            continue;
                        }
                    };
                    let spec: InstanceSpec = inst.spec.clone();
                    // Размер с учётом шардов (первый фрагмент → сумма всех частей).
                    let file_mib = crate::gguf::file_total_mib(&inst.model_path);
                    let mut need_mib: Option<u64> = None;
                    if let Ok(meta) = read_meta(&inst.model_path) {
                        let n_ctx = spec.n_ctx.unwrap_or(0) as i64;
                        let parallel = resolved.parallel.max(1) as i64;
                        // KV-тип роли и compute-буфер — часть честной оценки «нужно».
                        let kv_bits = if spec.cache_type_k == Some(8) {
                            KvBits::Q8_0
                        } else {
                            KvBits::F16
                        };
                        let n_ubatch = spec.n_ubatch.or(spec.n_batch).unwrap_or(2048).max(1) as i64;
                        let buffer_mib = compute_buffer_mib(&meta, n_ubatch);
                        // KV считается от контекста **слота**: llama.cpp делит общий
                        // n_ctx (= parallel × ctx_per_slot) между слотами (M4 §3.1).
                        // Передача общего контекста как per-slot завышала оценку
                        // KV в `parallel` раз — при parallel = 1 это было не видно.
                        let per_slot = crate::budget::per_slot_ctx(n_ctx, parallel);
                        let need = estimate_need_mib(&meta, file_mib, per_slot, parallel, kv_bits)
                            + buffer_mib.ceil() as u64;
                        log.line(&format!(
                            "роль {}: модель {file_mib} МиБ, KV {} {:.0} МиБ (слот {per_slot} токенов × {parallel}, KV-слоёв {} из {}), \
                             compute-буфер {:.0} МиБ (n_ubatch {}), n_batch {}, нужно {} МиБ",
                            inst.role,
                            if kv_bits == KvBits::Q8_0 {
                                "q8_0"
                            } else {
                                "f16"
                            },
                            crate::budget::kv_cache_mib(&meta, per_slot, parallel, kv_bits),
                            meta.kv_layer_count(),
                            meta.block_count,
                            buffer_mib,
                            n_ubatch,
                            spec.n_batch.unwrap_or(2048),
                            need
                        ));
                        need_mib = Some(need);
                        // Смысл предупреждения: при тесной карте compute-буфер съедает
                        // до 2 ГиБ, а уменьшение `n_batch` — самый дешёвый рычаг
                        // (замер 30.09.2026: 2048 → 512 даёт −1503 МиБ, §10.5).
                        if inst.role == "chat" && spec.n_batch.is_none() {
                            log.note(
                                "chat: llm.chat.n_batch не задан — движок возьмёт 2048 \
                                 (compute-буфер ≈2 ГиБ). При тесной карте задайте \
                                 `llm.chat.n_batch: 512` (+ `n_ubatch: 512`): замер дал −1503 МиБ VRAM",
                            );
                        }
                    }
                    // M4 §3.1: fit-гейт до создания инстанса — «модель + KV×parallel +
                    // compute-буфер» против свободной VRAM с резервом. Молчаливая
                    // деградация запрещена (решение заказчика): не влезает — явная
                    // ошибка с числами и подсказкой, а не невнятный отказ движка.
                    let gpu_role = spec.n_gpu_layers.is_some_and(|v| v != 0)
                        || config::role_base(&inst.role) == "whisper";
                    if gpu_role {
                        if let Some(need) = need_mib {
                            let measured = vram_probe
                                .as_ref()
                                .and_then(|p| p.snapshot())
                                .map(|v| v.free_mib);
                            let free = resolved.gpu.effective_free_mib(measured);
                            let reserve = resolved.gpu.reserve_mb;
                            let parallel = resolved.parallel.max(1) as i64;
                            match crate::budget::check_fit(free, need, reserve) {
                                crate::budget::Fit::NotEnough {
                                    free_mib,
                                    need_mib,
                                    reserve_mib,
                                    short_mib,
                                } if config::role_base(&inst.role) == "chat" => {
                                    return Err(EngineError::Other(format!(
                                        "роль chat не влезает в VRAM: нужно {need_mib} МиБ \
                                         (модель + KV×{parallel} слотов + compute-буфер), свободно \
                                         {free_mib} МиБ, резерв {reserve_mib} МиБ — не хватает \
                                         {short_mib} МиБ. Снизьте `llm_server.parallel` (сейчас \
                                         {parallel}) или `llm_server.chat.ctx_per_slot`; при тесной \
                                         карте помогает `llm.chat.n_batch: 512`"
                                    )));
                                }
                                crate::budget::Fit::NotEnough {
                                    free_mib, need_mib, ..
                                } => {
                                    // Роль грузится по запросу (LOAD_ON_DEMAND) —
                                    // стартуем, но честно предупреждаем с числами:
                                    // при обращении диспетчер вытеснит простые роли.
                                    log.note(&format!(
                                        "роль {}: сейчас может не влезть в VRAM (нужно {need_mib} МиБ, \
                                         свободно {free_mib} МиБ) — LOAD_ON_DEMAND, при обращении \
                                         диспетчер вытеснит простые роли по приоритетам",
                                        inst.role
                                    ));
                                }
                                crate::budget::Fit::Fits { .. } => {}
                            }
                        }
                    }
                    let id = cls.create_instance(&spec)?;
                    log.line(&format!("  инстанс '{}' id={id} создан", inst.role));
                    ids.insert(inst.role.clone(), id);
                    model_path.insert(inst.role.clone(), inst.model_path.display().to_string());
                    n_ctx_map.insert(inst.role.clone(), spec.n_ctx.unwrap_or(0));
                }
                if ids.is_empty() {
                    return Err(EngineError::Other(
                        "ни одной роли не удалось поднять — проверьте модели в общем рантайме \
                         (`installers/ensure_llama_runtime.ps1`) и конфиг"
                            .to_string(),
                    ));
                }
                if let Some(chat) = ids.get("chat").copied() {
                    log.line("загружаю чат-инстанс (KEEP_LOADED)…");
                    cls.load(chat)?;
                    let inst = cls.wait_loaded(
                        chat,
                        Duration::from_secs(300),
                        Duration::from_millis(500),
                    )?;
                    log.line(&format!("  чат: {}", inst.state_name));
                }
                // F1 (M5 T0.1): прогрев KEEP-ролей сразу после чата — иначе первый
                // RAG-запрос, дотянувшийся до on-demand роли, платил за её загрузку
                // сам (замер f1_r1: fit rerank ~27 с, перезагрузка embedding ~299 с
                // при тесной VRAM — выброс p95 315 с). Прогрев делает загрузку
                // детерминированной частью старта, а не сюрпризом под нагрузкой.
                for role in ["embedding", "rerank"] {
                    let keep = resolved
                        .role(role)
                        .is_some_and(|r| r.retention_mode == crate::ffi::retention::KEEP_LOADED);
                    if !keep {
                        continue;
                    }
                    if let Some(id) = ids.get(role).copied() {
                        log.line(&format!("прогрев роли {role} (KEEP_LOADED)…"));
                        match cls.load(id) {
                            Ok(()) => {
                                let inst = cls.wait_loaded(
                                    id,
                                    Duration::from_secs(300),
                                    Duration::from_millis(500),
                                )?;
                                log.line(&format!("  {role}: {}", inst.state_name));
                            }
                            Err(e) => {
                                // Не влезла — НЕ молчим: старт продолжается (роль
                                // поднимется по запросу), но диагностика кричит.
                                log.note(&format!(
                                    "прогрев роли {role} не удался: {e} — загрузится по \
                                     запросу (диспетчер вытеснит простые роли)"
                                ));
                            }
                        }
                    }
                }

                let shared = Arc::new(ClusterShared::new(cls));
                // L1: порог «долгого» вызова движка — строка появится в логе/heartbeat.
                shared.set_slow_ms(SLOW_CALL_MS);
                cluster = Some(shared);
                engine = Some(eng);
                cwd = Some(guard);
            }
        }

        let mut needs = role_needs(&resolved, &paths.root);
        if cfg.ngl == Some(0) {
            // проверочный прогон «всё на CPU»: VRAM ролям не нужна, иначе диспетчер
            // «освобождал» бы память выгрузкой ролей, которых там нет
            for v in needs.values_mut() {
                *v = 0;
            }
        }
        let pause = Arc::new(IndexPause::new(cfg.pause_dir.clone()));
        if pause.is_paused() {
            log.line(
                "index.pause уже стоит: запросы будут переиспользовать паузу и НЕ снимать её \
                 (пауза пользователя)",
            );
        }
        let stop = Arc::new(AtomicBool::new(false));
        // M6 T2 §3.5/§3.6: чат-реплики (по ним фасад раскидывает запросы) и карты
        // транскрибации (по одной на карту). Собираем из поднятого плана.
        let chat_roles: Vec<String> = planned
            .iter()
            .filter_map(|p| p.ready())
            .filter(|i| config::role_base(&i.role) == "chat")
            .map(|i| i.role.clone())
            .collect();
        let whisper_cards: Vec<(String, i32)> = planned
            .iter()
            .filter_map(|p| p.ready())
            .filter(|i| config::role_base(&i.role) == "whisper")
            .filter_map(|i| i.bridge_device_index.map(|c| (i.role.clone(), c)))
            .collect();
        if chat_roles.len() > 1 {
            log.line(&format!(
                "чат-реплики ({}): {} — маршрутизация least-busy → round-robin (§3.6)",
                chat_roles.len(),
                chat_roles.join(", ")
            ));
        }
        if !whisper_cards.is_empty() {
            log.line(&format!(
                "транскрибация по картам: {} — один файл на карту (§3.5/§3.6)",
                whisper_cards
                    .iter()
                    .map(|(r, c)| format!("{r} → карта {c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // M6 T3 §3.3: карта каждой роли — по ней диспетчер считает бюджет и
        // вытеснение (роли другой карты не трогаем). Пусто — прежнее поведение.
        let role_cards: BTreeMap<String, i32> = planned
            .iter()
            .filter_map(|p| p.ready())
            .filter_map(|i| i.bridge_device_index.map(|c| (i.role.clone(), c)))
            .collect();
        if !role_cards.is_empty() {
            log.line(&format!(
                "бюджет и вытеснение по картам ролей: {}",
                role_cards
                    .iter()
                    .map(|(r, c)| format!("{r} → карта {c}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // Пробы VRAM, привязанные к устройствам движка по имени: диспетчер мерит
        // именно то устройство, на которое поедет модель (iGPU vs dGPU!).
        let device_probes = bind_device_probes(&engine_devices, cfg.nvml_index);
        if !device_probes.is_empty() {
            let bound = device_probes
                .iter()
                .map(|(idx, p)| format!("{idx}:{} ({})", p.name(), p.source().as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            log.line(&format!("пробы VRAM по устройствам: {bound}"));
        } else if !engine_devices.is_empty() {
            log.note(
                "ни одна проба VRAM не сопоставилась с устройством движка — решения по числам движка (R29)",
            );
        }
        let backend = Arc::new(ClusterBackend {
            cluster: cluster.clone(),
            ids: ids.clone(),
            model_path,
            n_ctx: n_ctx_map,
            needs: needs.clone(),
            gpu: resolved.gpu.clone(),
            vram_probe,
            devices: engine_devices,
            probes: device_probes,
            parallel: resolved.parallel,
            dispatch_enabled: cfg.dispatcher,
            decisions: Mutex::new(Vec::new()),
            last_plan: Mutex::new(None),
            last_used: Mutex::new(BTreeMap::new()),
            pause: Arc::clone(&pause),
            stop: Arc::clone(&stop),
            log: Arc::clone(&log),
            started,
            pid: pid.as_ref().map(|p| p.pid()).unwrap_or(std::process::id()),
            cfg: resolved.clone(),
            runtime_root: paths.root.clone(),
            engine_dir: engine_dir.clone(),
            baseline_used_mib,
            planned,
            pause_dir: cfg.pause_dir.clone(),
            pid_path: cfg.pid_path(),
            log_path: cfg.log_path(),
            mode,
            upstream,
            whisper: Mutex::new(BTreeMap::new()),
            chat_roles,
            chat_rr: std::sync::atomic::AtomicUsize::new(0),
            whisper_cards,
            role_cards,
            transcribe_active: Arc::new(AtomicBool::new(false)),
            abandoned_request_sec: resolved.gpu.abandoned_request_sec,
            diar: diar_settings,
            diar_auto: OnceLock::new(),
            nemotron: Mutex::new(None),
            ffmpeg,
        });
        // Решение «конфиг или авто» — один раз и в лог (решение владельца №10):
        // при отсутствии секции модель Nemotron-3 проверяется по sha256 здесь.
        backend.resolved_diar_backend();

        let handles = if ports.is_empty() {
            log.note(
                "в конфиге нет ролей чата/эмбеддингов/реранка — фасад не поднимается \
                 (проверьте llm_server.<role>.model)",
            );
            Vec::new()
        } else {
            facade::serve(
                &server,
                Arc::clone(&backend) as Arc<dyn Backend>,
                Arc::clone(&stop),
            )?
        };
        // фоновый арбитр: ARB-3 (при живой индексации резидент уступает VRAM) и ARB-5 (простой)
        let arbiter = backend.spawn_arbiter(Arc::clone(&stop));
        // L1: heartbeat резидента — состояние видно снаружи даже когда движок занят и
        // фасад не отвечает (`W4_REPORT.md` §14).
        let heartbeat_path = resident::default_heartbeat_path(&cfg.data_dir);
        let heartbeat = backend.spawn_heartbeat(heartbeat_path.clone(), Arc::clone(&stop));
        log.line(&format!(
            "фон: heartbeat резидента (такт {} с, {}; «кто держит движок» видно и без HTTP)",
            resident::HEARTBEAT_EVERY_SECS,
            heartbeat_path.display()
        ));
        log.line(&format!(
            "фон: арбитр простоя и индексации (такт 15 с, gpu.evict_idle_sec = {}, policy {})",
            resolved.gpu.evict_idle_sec,
            resolved.gpu.policy.as_str()
        ));
        log.line(&format!(
            "готово: {} ({})",
            if ports.is_empty() {
                "фасад выключен".to_string()
            } else {
                ports
                    .iter()
                    .map(|(r, p)| format!("{r}:{p}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            if cfg.pid_file.is_some() {
                "резидентный режим: остановка — `llm-host stop` или Ctrl+C"
            } else {
                "разовый прогон (pid-файл не занимается)"
            }
        ));
        log.line(&format!(
            "роли и потребность: {}",
            if needs.is_empty() {
                "нет (модели не найдены)".to_string()
            } else {
                needs
                    .iter()
                    .map(|(r, n)| format!("{r}={n} МиБ"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));

        Ok(Host {
            cfg,
            resolved,
            server,
            host,
            mode,
            log,
            pid,
            stop,
            handles,
            arbiter: Some(arbiter),
            heartbeat: Some(heartbeat),
            backend: Some(backend),
            ids,
            cluster,
            engine,
            _cwd: cwd,
            needs,
            pause,
            engine_dir,
            ports,
            cleanup: Vec::new(),
            started,
        })
    }
}

/// Чтение состояния и остановка — вторая часть `impl`, чтобы `start` не тонул
/// в мелочах.
impl Host {
    /// Режим работы (`embedded` | `facade` | `off`).
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Порты фасада по ролям (`chat`/`embedding`/`rerank`).
    pub fn ports(&self) -> &[(String, u16)] {
        &self.ports
    }

    /// Адрес, на котором слушает фасад (`llm_server.host`).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Настройки фасада (thinking/max_tokens/temperature/internal).
    pub fn server(&self) -> &ServerConfig {
        &self.server
    }

    /// Инстансы по ролям (`id` из `create_instance`).
    pub fn ids(&self) -> &BTreeMap<String, i64> {
        &self.ids
    }

    /// Оценка «модель + KV» по ролям, МиБ.
    pub fn needs(&self) -> &BTreeMap<String, u64> {
        &self.needs
    }

    /// Каталог движка (`None` в режимах `facade`/`off`).
    pub fn engine_dir(&self) -> Option<&Path> {
        self.engine_dir.as_deref()
    }

    /// Конфигурация запуска (пути, порты, флаги).
    pub fn config(&self) -> &HostConfig {
        &self.cfg
    }

    /// Разобранный `config.yaml` (роли, gpu, warnings).
    pub fn resolved(&self) -> &LlmHostConfig {
        &self.resolved
    }

    /// Лог (консоль + файл) — тот же, что пишут backend и уборка.
    pub fn log(&self) -> &Arc<Log> {
        &self.log
    }

    /// Путь лог-файла (даже если лог не ведётся).
    pub fn log_path(&self) -> PathBuf {
        self.cfg.log_path()
    }

    /// Путь pid-файла (даже если резидентность выключена).
    pub fn pid_path(&self) -> PathBuf {
        self.cfg.pid_path()
    }

    /// Наш pid (`None` — pid-файл не занимался).
    pub fn pid(&self) -> Option<u32> {
        self.pid.as_ref().map(|p| p.pid())
    }

    /// Сколько хост уже работает (для `status`).
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Журнал решений диспетчера (строки «что решили» — для `--json`/логов).
    pub fn dispatcher_lines(&self) -> Vec<String> {
        self.backend
            .as_ref()
            .and_then(|b| b.decisions.lock().ok().map(|d| d.clone()))
            .unwrap_or_default()
    }

    /// Стоит ли `index.pause` сейчас (в т.ч. пауза пользователя).
    pub fn pause_present(&self) -> bool {
        self.pause.is_paused()
    }

    /// Журнал уборки (заполняется в [`Host::stop`]).
    pub fn cleanup_log(&self) -> Vec<String> {
        self.cleanup.clone()
    }

    /// Готовый отчёт резидента (транспорт — тот же, что у CLI: `/internal/status`).
    pub fn status(&self) -> Result<Value> {
        match &self.backend {
            Some(b) => b.internal_status(),
            None => Err(EngineError::Other("хост уже остановлен".to_string())),
        }
    }

    /// Ждать остановки: `hold_secs > 0` — ограниченный прогон (проверки/скрипты),
    /// `0` — до команды `llm-host stop` или Ctrl+C.
    pub fn wait(&self, hold_secs: u64) {
        let deadline = if hold_secs > 0 {
            Some(Instant::now() + Duration::from_secs(hold_secs))
        } else {
            None
        };
        while !self.stop.load(Ordering::Relaxed) {
            if let Some(d) = deadline {
                if Instant::now() >= d {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

impl Host {
    /// Остановить хост: фасад → инстансы → своя пауза → pid-файл.
    ///
    /// Порядок продиктован реальными граблями:
    /// 1. сначала `stop`-флаг и `join` потоков сокетов — иначе уборка пойдёт
    ///    параллельно с обработкой запроса;
    /// 2. потом инстансы: `unload` (освобождает VRAM) и `remove` (убирает из
    ///    кластера) — так делает и Python-менеджер ролей;
    /// 3. пауза: отпускаем **только свои аренды** (`resume` по счётчику
    ///    вложенности). `force_resume` здесь не зовём: он удаляет и паузу
    ///    пользователя, поставленную кнопкой в UI (грабля §9.7 п.8 — регресс был
    ///    пойман живым прогоном);
    /// 4. в конце — pid-файл: пока он наш, второй `llm-host` не поднимется.
    pub fn stop(&mut self) {
        if self.backend.is_none() && self.handles.is_empty() {
            return; // уже остановлен (повторный вызов из `Drop` безвреден)
        }
        self.stop.store(true, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
        if let Some(h) = self.arbiter.take() {
            let _ = h.join();
        }
        if let Some(h) = self.heartbeat.take() {
            let _ = h.join();
        }

        if let Some(cluster) = self.cluster.clone() {
            let mut cleanup = Vec::new();
            for (role, id) in &self.ids {
                let _ = cluster.with(|c| c.unload(*id));
                let removed = cluster.with(|c| c.remove_instance(*id));
                cleanup.push(format!("{role}: remove_instance -> {removed:?}"));
            }
            if !cleanup.is_empty() {
                self.log.line(&format!(
                    "инстансы сняты: {} ({})",
                    cleanup.len(),
                    cleanup.join("; ")
                ));
            }
            self.cleanup.extend(cleanup);
        }

        let mut released = 0;
        while self.pause.depth() > 0 {
            match self.pause.resume() {
                Ok(_) => released += 1,
                Err(e) => {
                    self.log
                        .note(&format!("пауза: не удалось отпустить аренду: {e}"));
                    break;
                }
            }
        }
        if released > 0 {
            self.log
                .line(&format!("index.pause: отпущено своих аренд: {released}"));
        }
        self.log.line(&format!(
            "index.pause сейчас: {}",
            if self.pause.is_paused() {
                "стоит (не наша — не снимаем)"
            } else {
                "нет"
            }
        ));

        // порядок освобождения: backend → cluster → engine (см. комментарий у структуры)
        self.backend = None;
        self.cluster = None;
        self.engine = None;
        if let Some(pid) = self.pid.take() {
            let released = pid.release();
            self.log.line(&format!(
                "pid-файл {}: {}",
                pid.path().display(),
                if released {
                    "освобождён"
                } else {
                    "не наш — оставлен"
                }
            ));
        }
        self.log.line(&format!(
            "остановлен (проработал {} с)",
            self.started.elapsed().as_secs()
        ));
    }
}

impl Drop for Host {
    /// Уборка при выходе из `main` (в т.ч. по `Ctrl+C`, если бинарь её поймал):
    /// `stop` идемпотентен, поэтому повторный вызов ничего не портит.
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod whisper_device_tests {
    use super::names_match;
    use super::pick_replica;
    use super::strip_index_self_pause;
    use super::whisper_device;
    use crate::dispatch::{Action, Plan};

    /// Страж брошенных запросов: срабатывает только на «engine» дольше порога —
    /// служебные операции (status/arbiter/dispatcher) и короткие вызовы не трогаем.
    #[test]
    fn abandoned_request_guard_semantics() {
        use super::abandoned_request_guard;
        use crate::gate::Busy;
        use std::time::Duration;
        let mk = |what: &str, secs: u64| Busy {
            what: what.to_string(),
            since: std::time::Instant::now() - Duration::from_secs(secs),
            // M4 T2: при параллельной генерации держателей больше одного —
            // на семантику стража (метка `engine`) это не влияет.
            holders: 1,
        };
        // запрос чата висит дольше порога → остановка
        assert!(abandoned_request_guard(&mk("engine", 901), "engine", 900));
        // ровно на пороге → тоже срабатываем
        assert!(abandoned_request_guard(&mk("engine", 900), "engine", 900));
        // ещё держится недолго → терпим
        assert!(!abandoned_request_guard(&mk("engine", 899), "engine", 900));
        // служебные операции — не запросы, их не сбрасываем
        assert!(!abandoned_request_guard(
            &mk("arbiter", 5_000),
            "engine",
            900
        ));
        assert!(!abandoned_request_guard(
            &mk("status:devices", 5_000),
            "engine",
            900
        ));
        // страж выключен (порог 0) → никогда
        assert!(!abandoned_request_guard(&mk("engine", 5_000), "engine", 0));
    }

    /// M6 T2 §3.6: выбор чат-реплики — least-busy, при равенстве round-robin
    /// (чистая логика: загрузки передаются аргументом).
    #[test]
    fn replica_pick_is_least_busy_then_round_robin() {
        use std::collections::BTreeMap;
        use std::sync::atomic::AtomicUsize;
        let roles = vec!["chat".to_string(), "chat-2".to_string()];
        let rr = AtomicUsize::new(0);
        // least-busy: свободная реплика важнее порядка
        let mut loads = BTreeMap::new();
        loads.insert("chat".to_string(), 3);
        loads.insert("chat-2".to_string(), 0);
        assert_eq!(pick_replica(&roles, &loads, &rr), Some("chat-2"));
        // равенство (в т.ч. «нет замера») → round-robin по кругу
        let empty = BTreeMap::new();
        let rr2 = AtomicUsize::new(0);
        assert_eq!(pick_replica(&roles, &empty, &rr2), Some("chat"));
        assert_eq!(pick_replica(&roles, &empty, &rr2), Some("chat-2"));
        assert_eq!(pick_replica(&roles, &empty, &rr2), Some("chat"));
        // одна реплика — она и есть; ролей нет — None
        assert_eq!(pick_replica(&roles[..1], &empty, &rr), Some("chat"));
        assert_eq!(pick_replica(&[], &empty, &rr), None);
    }

    /// Роли индексации не ставят паузу индексации сами себе (фликер в UI).
    #[test]
    fn index_roles_do_not_pause_indexing() {
        let mut plan = Plan::new(Some(7000), 726, 1024);
        plan.actions.push(Action::PauseIndex {
            reason: "запрос роли 'embedding'".to_string(),
        });
        plan.actions.push(Action::EnsureLoaded {
            role: "embedding".to_string(),
        });
        strip_index_self_pause(&mut plan, "embedding");
        assert!(
            !plan
                .actions
                .iter()
                .any(|a| matches!(a, Action::PauseIndex { .. })),
            "пауза снята для embedding"
        );
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, Action::EnsureLoaded { .. })),
            "остальные действия целы"
        );

        // Пользовательские роли — пауза остаётся (запрос важнее индексации).
        let mut plan = Plan::new(Some(7000), 9036, 1024);
        plan.actions.push(Action::PauseIndex {
            reason: "запрос роли 'chat'".to_string(),
        });
        strip_index_self_pause(&mut plan, "chat");
        assert!(plan
            .actions
            .iter()
            .any(|a| matches!(a, Action::PauseIndex { .. })));
    }

    #[test]
    fn enough_vram_keeps_gpu() {
        let (d, fb) = whisper_device(0, 1549, 1024, Some(11255));
        assert_eq!(d, 0);
        assert!(fb.is_none());
    }

    #[test]
    fn tight_vram_falls_back_to_cpu() {
        // 1549 + 256 + 1024 = 2829 > 2000 → CPU с причиной
        let (d, fb) = whisper_device(0, 1549, 1024, Some(2000));
        assert_eq!(d, -1);
        assert!(fb.unwrap().contains("2829"));
    }

    #[test]
    fn explicit_cpu_is_respected() {
        let (d, fb) = whisper_device(-1, 1549, 1024, Some(10));
        assert_eq!(d, -1);
        assert!(fb.is_none(), "запрос на CPU не считается фолбэком");
    }

    #[test]
    fn no_nvml_keeps_gpu() {
        let (d, fb) = whisper_device(0, 1549, 1024, None);
        assert_eq!(d, 0);
        assert!(fb.is_none());
    }

    // --- сопоставление пробы VRAM с устройством движка (общий случай: iGPU+dGPU) ---
    #[test]
    fn names_match_engine_desc_and_probe_names() {
        // Замеры 05.10.2026: описания движка и имена NVML/DXGI совпадают.
        assert!(names_match(
            "NVIDIA GeForce RTX 3060",
            "NVIDIA GeForce RTX 3060"
        ));
        assert!(names_match(
            "AMD Radeon(TM) Graphics",
            "AMD Radeon(TM) Graphics"
        ));
        // Пунктуация/регистр не должны мешать.
        assert!(names_match(
            "AMD Radeon(TM) Graphics",
            "amd radeon graphics"
        ));
        // Разные карты не совпадают; имя без описания (`Vulkan0`) — тоже нет.
        assert!(!names_match(
            "AMD Radeon(TM) Graphics",
            "NVIDIA GeForce RTX 3060"
        ));
        assert!(!names_match("Vulkan0", "NVIDIA GeForce RTX 3060"));
        // Страж от «мягкого» сопоставления: другая ревизия той же линейки — НЕ та карта.
        assert!(!names_match(
            "NVIDIA GeForce RTX 3060",
            "NVIDIA GeForce RTX 3060 Ti"
        ));
        assert!(!names_match("", "NVIDIA GeForce RTX 3060"));
        assert!(!names_match("NVIDIA GeForce RTX 3060", ""));
    }

    // --- M6 T3 §3.3: строки VRAM по картам ---

    /// Строки по картам: карта, бэкенд, устройство, свободно и её роли. Картами
    /// считаются ускорители движка плюс карты из раскладки ролей; CPU в карты не
    /// попадает. Функция чистая — проверяется без движка и GPU.
    #[test]
    fn cards_lines_show_per_card_roles_and_free() {
        use super::cards_lines;
        use crate::cluster::Device;
        use std::collections::BTreeMap;
        let dev = |idx: i32, backend: &str, name: &str, desc: &str| Device {
            bridge_device_index: idx,
            device_type: 1,
            memory_free: 3 * 1024 * 1024 * 1024,
            memory_total: 12 * 1024 * 1024 * 1024,
            backend: backend.to_string(),
            name: name.to_string(),
            description: desc.to_string(),
        };
        let devices = vec![
            dev(0, "CUDA", "CUDA0", "NVIDIA GeForce RTX 3060"),
            dev(1, "CPU", "CPU", "AMD Ryzen 7 5700G"),
        ];
        let mut roles = BTreeMap::new();
        roles.insert("chat".to_string(), 0);
        roles.insert("chat-2".to_string(), 0);
        roles.insert("embedding".to_string(), 1);
        let lines = cards_lines(&devices, &roles, &|c| {
            if c == 0 {
                Some(3269)
            } else {
                None
            }
        });
        assert_eq!(
            lines.len(),
            2,
            "CPU-устройство картой не считается: {lines:?}"
        );
        assert!(
            lines[0].contains("карта 0 (CUDA, NVIDIA GeForce RTX 3060)"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("свободно 3269 МиБ"), "{}", lines[0]);
        assert!(lines[0].contains("chat, chat-2"), "{}", lines[0]);
        assert!(lines[1].contains("нет замера"), "{}", lines[1]);
        assert!(lines[1].contains("embedding"), "{}", lines[1]);
        // Нет ни ускорителей, ни раскладки — нет и строк (не выдумываем карты).
        assert!(cards_lines(&[], &BTreeMap::new(), &|_| None).is_empty());
    }
}
