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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::budget::estimate_need_mib;
use crate::cluster::{Cluster, InstanceSpec};
use crate::config::{self, GpuConfig, LlmHostConfig, Mode};
use crate::dispatch::{self, instance_use, plan_query, Demand, InstanceUse, Plan};
use crate::error::{EngineError, Result};
use crate::facade::{self, Backend, ChatRequest, ServerConfig, Thinking, Usage};
use crate::gguf::{read_meta, KvBits};
use crate::pause::{read_heartbeat, IndexPause, PauseLease};
use crate::registry::{self, RolePlan};
use crate::resident::{self, Log, PidFile};
use crate::runtime::RuntimePaths;
use crate::status::{device_line, StatusInput, StatusReport};
use crate::vram::{NvmlProbe, VramProbe};
use crate::{Engine, engine::EngineCwd};

/// Корень репозитория (относительно крейта: `crates/hds-llama/../..`).
///
/// Нужен для путей по умолчанию: `config.yaml`, `index.pause`, `data/`.
pub fn repo_root() -> PathBuf {
    clean_path(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(".."),
    )
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
        if n_gpu_layers == 0 && rc.role != "whisper" {
            needs.insert(rc.role.clone(), 0);
            continue;
        }
        let parallel = cfg.parallel.max(1) as i64;
        let need = match read_meta(&model_path) {
            Ok(meta) => estimate_need_mib(
                &meta,
                file_mib,
                rc.n_ctx.max(0) as i64,
                parallel,
                KvBits::F16,
            ),
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
            instance_use(i, &i.name.clone(), own, idle.get(&i.name).copied().unwrap_or(0))
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
        HostConfig {
            config: std::env::var("HDS_CONFIG")
                .map(PathBuf::from)
                .unwrap_or_else(|_| root.join("config.yaml")),
            runtime: None,
            engine_dir: None,
            host: None,
            port_base: None,
            ngl: None,
            thinking: None,
            dispatcher: true,
            pause_dir: root.clone(),
            pid_file: Some(resident::default_pid_path(&root)),
            log_file: Some(resident::default_log_path(&root)),
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
}

impl LocalStatus {
    /// Человеческие строки для консоли (`llm_host_status`, `llm_host status`).
    pub fn lines(&self) -> Vec<String> {
        let mut out = self.report.lines();
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
    let nvml = NvmlProbe::open(args.nvml_index).ok();
    let vram = nvml.as_ref().and_then(|p| p.snapshot());

    // Прогноз диспетчера: что будет, если запрос чата придёт прямо сейчас.
    let idle: BTreeMap<String, u64> = BTreeMap::new();
    let uses = instance_uses(&instances, &needs, &idle);
    let forecast = cfg.role("chat").map(|_| {
        plan_query(
            &cfg.gpu,
            vram.map(|v| v.free_mib),
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
        if pause.is_paused() { "стоит" } else { "нет" },
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

    Ok(LocalStatus {
        report,
        pause_line,
        instances_line,
    })
}


/// Кластер, доступный из потоков фасада (§3 плана W2: «один владелец GPU»).
///
/// `Cluster` держит сырой указатель движка (FFI), поэтому сам по себе не `Send`;
/// фасад обслуживает запросы в потоках сокетов — значит доступ сериализуем
/// мьютексом: движок вызывается **из одного потока за раз**.
pub struct ClusterShared(Mutex<Cluster>);

// SAFETY: доступ к кластеру идёт только через `with()`, то есть под мьютексом —
// параллельных вызовов одного объекта кластера не бывает.
unsafe impl Send for ClusterShared {}
unsafe impl Sync for ClusterShared {}

impl ClusterShared {
    pub fn new(cluster: Cluster) -> ClusterShared {
        ClusterShared(Mutex::new(cluster))
    }

    /// Вызвать движок под мьютексом (единственный путь к кластеру).
    pub fn with<T>(&self, f: impl FnOnce(&Cluster) -> T) -> T {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }
}

/// Живой backend фасада: инстансы по конфигу + диспетчер VRAM (A4) + пауза
/// индексации + внутренний API управления (A6).
///
/// Основная часть — перенос из `bin/llm_host_facade.rs` (A5), поэтому поля и
/// порядок решений совпадают с проверенным живым прогоном (§7.3 `W2_REPORT.md`).
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
    nvml: Option<Arc<NvmlProbe>>,
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
    /// W3: ленивый транскрибатор whisper (роль whisper, bridge-API движка).
    whisper: Mutex<Option<WhisperCell>>,
}


/// Ленивый транскрибатор whisper под `Mutex` (raw-указатели bridge ⇒ `!Send/!Sync`;
/// доступ строго под `Mutex`, поэтому `Send+Sync` помечаем вручную — как
/// [`ClusterShared`]).
struct WhisperCell(Mutex<crate::whisper::Whisper>);
unsafe impl Send for WhisperCell {}
unsafe impl Sync for WhisperCell {}

/// whisper-модель из общего каталога движка: `*.bin`/`*.gguf` в каталоге `*whisper*`.
fn default_whisper_model() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    let root = base.config_dir().join("OpenResearchTools").join("models");
    for e in std::fs::read_dir(&root).ok()?.flatten() {
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

impl ClusterBackend {
    /// Кластер движка или объяснение, почему его нет (режим `llm_server.mode`).
    fn cl(&self) -> Result<&Arc<ClusterShared>> {
        self.cluster.as_ref().ok_or_else(|| {
            EngineError::Other(match self.mode {
                Mode::Off => {
                    "LLM выключен (llm_server.mode: off) — инстансы не создаются, \
                     поиск работает только по FTS"
                        .to_string()
                }
                Mode::Facade => {
                    "режим llm_server.mode: facade — своих инстансов нет, роли держит \
                     внешний OpenAI-совместимый сервер"
                        .to_string()
                }
                Mode::Embedded => "кластер движка не создан".to_string(),
            })
        })
    }

    /// Свободная VRAM (NVML) — источник истины (R29).
    fn free_mib(&self) -> Option<u64> {
        self.nvml.as_ref().and_then(|p| p.snapshot()).map(|v| v.free_mib)
    }

    fn id_of(&self, role: &str) -> Result<i64> {
        self.cl()?;
        self.ids.get(role).copied().ok_or_else(|| {
            EngineError::Other(format!(
                "инстанс роли '{role}' не создан (нет модели или роли нет в конфиге)"
            ))
        })
    }

    /// Загрузить роль, если она выгружена, и дождаться готовности.
    ///
    /// Нужен не только для `LOAD_ON_DEMAND`: диспетчер может выгрузить чат, чтобы
    /// отдать память другой роли (§8.6.2), и следующий запрос обязан его вернуть.
    fn ensure_loaded(&self, role: &str) -> Result<i64> {
        let cl = self.cl()?;
        let id = self.id_of(role)?;
        let loaded = cl.with(|c| {
            c.instance_by_id(id)
                .ok()
                .flatten()
                .map(|i| i.is_loaded())
                .unwrap_or(false)
        });
        if !loaded {
            cl.with(|c| c.load(id))?;
            cl.with(|c| c.wait_loaded(id, Duration::from_secs(300), Duration::from_millis(500)))?;
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
    fn uses(&self) -> Vec<InstanceUse> {
        let instances = self
            .cluster
            .as_ref()
            .map(|c| c.with(|x| x.instances()).unwrap_or_default())
            .unwrap_or_default();
        instance_uses(&instances, &self.needs, &self.idle_map())
    }

    /// Перед запросом: решить по VRAM и применить решение (ARB-1/ARB-2).
    ///
    /// Возвращает `PauseLease`, который держится до конца запроса: на `Drop` пауза
    /// снимается (если ставили её мы) — индексные роли вернутся сами.
    ///
    /// `Err` — после вытеснения памяти всё ещё не хватает: запрос надо отклонить с
    /// отчётом (`Verdict::NotEnough`), **без авто-деградации** (§8.6.2).
    fn prepare(&self, role: &str) -> Result<Option<PauseLease>> {
        if !self.dispatch_enabled {
            return Ok(None);
        }
        let need = self.needs.get(role).copied().unwrap_or(0);
        if need == 0 {
            return Ok(None);
        }
        let plan = {
            // сколько освободит каждый инстанс — по ЕГО собственной роли, а не по
            // запрошенной: иначе арифметика вытеснения врёт (поймано живым прогоном)
            let uses = self.uses();
            plan_query(&self.gpu, self.free_mib(), &Demand::new(role, need), &uses)
        };
        let log = self
            .cl()?
            .with(|cluster| dispatch::apply(cluster, &self.pause, &plan));
        for line in plan.lines().iter().chain(log.iter()) {
            self.log.line(&format!("[dispatcher] {line}"));
        }
        if let Ok(mut d) = self.decisions.lock() {
            d.push(format!("{role}: {}", plan.verdict.as_str()));
            d.extend(plan.lines());
        }
        let verdict_ok = plan.verdict.is_ok();
        if let Ok(mut last) = self.last_plan.lock() {
            *last = Some(plan);
        }
        if !verdict_ok {
            // Паузу мы поставили «под запрос» (действие PauseIndex), но запрос
            // отклонён — снимаем её здесь же: иначе неудавшийся запрос оставит
            // индексацию стоящей навсегда (R30: «индексация встала»). Чужую паузу
            // `resume` не трогает — только нашу.
            let _ = self.pause.resume();
            return Err(EngineError::Other(format!(
                "не хватает VRAM для роли '{role}': нужно {need} МиБ, свободно {}; \
                 вытеснение не помогло — авто-деградации нет (gpu.model_policy: {})",
                self.free_mib()
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                self.cfg.model_policy
            )));
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
        let heartbeat = read_heartbeat(&self.pause_dir);
        let indexing_live = heartbeat.as_ref().map(|h| h.is_live()).unwrap_or(false);
        let mut plans: Vec<(&str, Plan)> = Vec::new();

        // ARB-5: простой роли (свой предохранитель + grace роли)
        let uses = self.uses();
        let idle_actions = dispatch::idle_evictions(&self.gpu, &uses);
        if !idle_actions.is_empty() {
            let mut plan = Plan::new(self.free_mib(), 0, self.gpu.reserve_mb);
            plan.actions = idle_actions;
            plans.push(("idle", plan));
        }

        // ARB-3: идёт индексация, памяти не хватает — резидент (chat) уступает ей
        if indexing_live {
            let need = self.needs.get("embedding").copied().unwrap_or(0);
            if need > 0 {
                let plan = dispatch::plan_indexing(
                    &self.gpu,
                    self.free_mib(),
                    &Demand::new("embedding", need),
                    &uses,
                );
                if !plan.actions.is_empty() {
                    plans.push(("indexing", plan));
                }
            }
        }

        for (why, plan) in plans {
            let log = self
                .cl()
                .map(|cl| cl.with(|cluster| dispatch::apply(cluster, &self.pause, &plan)))
                .unwrap_or_default();
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
        let _lease = self.prepare("chat")?;
        let id = self.ensure_loaded("chat")?;
        let out = self.cl()?.with(|c| {
            c.chat_complete(
                id,
                &req.prompt,
                req.n_predict,
                req.temperature,
                req.reasoning(),
            )
        })?;
        self.note_used("chat");
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
        let _lease = self.prepare("embedding")?;
        let id = self.ensure_loaded("embedding")?;
        let out = self
            .cl()?
            .with(|c| c.embeddings_json(id, body_json, true))?;
        self.note_used("embedding");
        out.ensure_ok()
            .map_err(|e| EngineError::Other(e.to_string()))?;
        Ok(out.json)
    }

    fn rerank(&self, body_json: &str) -> Result<String> {
        let _lease = self.prepare("rerank")?;
        let id = self.ensure_loaded("rerank")?;
        let out = self.cl()?.with(|c| c.rerank_json(id, body_json))?;
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
        let state = self
            .cluster
            .as_ref()
            .and_then(|c| {
                c.with(|x| x.instance_by_id(id).ok().flatten().map(|i| i.state_name.clone()))
            })
            .unwrap_or_default();
        Some(json!({
            "model_path": self.model_path.get(role).cloned().unwrap_or_default(),
            "n_ctx": self.n_ctx.get(role).copied().unwrap_or(0),
            "total_slots": self.parallel.max(1),
            "state": state,
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
        let devices = self
            .cluster
            .as_ref()
            .and_then(|c| c.with(|x| x.devices()).ok())
            .unwrap_or_default();
        let instances = self
            .cluster
            .as_ref()
            .and_then(|c| c.with(|x| x.instances()).ok())
            .unwrap_or_default();
        let vram = self.nvml.as_ref().and_then(|p| p.snapshot());
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
            map.insert("lines".to_string(), json!(report.lines()));
            map.insert("mode".to_string(), json!(self.mode.as_str()));
            map.insert("pid".to_string(), json!(self.pid));
            map.insert("uptime_sec".to_string(), json!(self.started.elapsed().as_secs()));
            map.insert("pid_file".to_string(), json!(self.pid_path.display().to_string()));
            map.insert("log_file".to_string(), json!(self.log_path.display().to_string()));
            map.insert("dispatcher_enabled".to_string(), json!(self.dispatch_enabled));
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
        }
        Ok(json)
    }

    /// `/internal/devices` — устройства движка (bridge-индексы, память, бэкенд).
    fn internal_devices(&self) -> Result<Value> {
        let cl = self.cl()?;
        let devices = cl.with(|c| c.devices())?;
        Ok(json!({
            "lines": devices.iter().map(device_line).collect::<Vec<_>>(),
            "devices": devices
                .iter()
                .map(|d| json!({
                    "bridge_device_index": d.bridge_device_index,
                    "backend": d.backend,
                    "name": d.name,
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
        cl.with(|c| c.load(id))?;
        let inst =
            cl.with(|c| c.wait_loaded(id, Duration::from_secs(300), Duration::from_millis(500)))?;
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
        cl.with(|c| c.unload(id))?;
        let state = cl
            .with(|c| c.instance_by_id(id).ok().flatten().map(|i| i.state_name.clone()))
            .unwrap_or_default();
        self.log
            .line(&format!("[internal] роль '{role}': выгружена ({state}), id={id}"));
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

    /// W3: транскрибация аудио/видео через bridge-API движка (роль `whisper`).
    ///
    /// Тело: `{"path": ..., "mode": "subtitle|speech", "custom": "4.5", "gpu": 0,
    /// "model": ...}`. Транскрибатор создаётся лениво и переиспользуется.
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
        let gpu = body.get("gpu").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .or_else(default_whisper_model)
            .ok_or_else(|| {
                EngineError::Other("не найдена whisper-модель (index.whisper_model)".to_string())
            })?;

        let mut slot = self.whisper.lock().unwrap();
        if slot.is_none() {
            self.log.line(&format!(
                "[whisper] создаю транскрибатор: модель {} (gpu {gpu})",
                model.display()
            ));
            let api = crate::bridge_audio::BridgeAudio::load(engine_dir)?;
            let w = crate::whisper::Whisper::new(api, &model, gpu, -1)?;
            *slot = Some(WhisperCell(Mutex::new(w)));
        }
        let w = slot.as_ref().unwrap().0.lock().unwrap();
        let tr = w.transcribe_file(Path::new(path), mode, &custom)?;
        let segments: Vec<Value> = tr
            .segments
            .iter()
            .map(|s| {
                json!({ "text": s.text, "t_start": s.t_start, "t_end": s.t_end })
            })
            .collect();
        Ok(json!({
            "path": path,
            "mode": mode,
            "custom": custom,
            "segments": segments,
            "stats": tr.json.get("stats").cloned().unwrap_or(Value::Null),
        }))
    }
}

/// Записать JSON-отчёт (каталог создаётся) — для бинарей `--json`.
pub fn write_json_file(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| EngineError::Other(format!("json: {e}")))?;
    std::fs::write(path, text).map_err(|e| EngineError::Other(format!("{}: {e}", path.display())))?;
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
            if inst.role == "whisper" {
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
            format!("CPU{}", cpu.map(|c| format!(" (bridge index {c})")).unwrap_or_default())
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
fn upstream_map(
    yaml: &serde_yaml::Value,
    cfg: &LlmHostConfig,
) -> BTreeMap<String, String> {
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
        let yaml: serde_yaml::Value = serde_yaml::from_str(
            &std::fs::read_to_string(&cfg.config).map_err(|e| {
                EngineError::Other(format!("конфиг {}: {e}", cfg.config.display()))
            })?,
        )
        .map_err(|e| EngineError::Other(format!("конфиг {}: {e}", cfg.config.display())))?;
        for w in &resolved.warnings {
            log.note(&format!("конфиг: {w}"));
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
                Thinking::parse(&dig_str(&yaml, "chat.thinking").unwrap_or_else(|| "off".to_string()))
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
        let upstream = upstream_map(&yaml, &resolved);
        let nvml = NvmlProbe::open(cfg.nvml_index).ok().map(Arc::new);

        if let Some(p) = &nvml {
            if let Some(v) = p.snapshot() {
                log.line(&format!(
                    "NVML {}: занято {} / {} МиБ, свободно {} МиБ",
                    p.name(),
                    v.used_mib,
                    v.total_mib,
                    v.free_mib
                ));
                // baseline: наша занятость = NVML used − это значение (R29)
                baseline_used_mib = Some(v.used_mib);
            }
        } else {
            log.note("NVML недоступен: бюджет VRAM не проверяется (R29)");
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
                planned = registry::plan(&resolved, &paths.root, &devices);
                if let Some(ngl) = cfg.ngl {
                    apply_ngl_override(&mut planned, ngl, &devices, &log);
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
                    let file_mib = std::fs::metadata(&inst.model_path)
                        .map(|m| m.len() >> 20)
                        .unwrap_or(0);
                    if let Ok(meta) = read_meta(&inst.model_path) {
                        let n_ctx = spec.n_ctx.unwrap_or(0) as i64;
                        let parallel = resolved.parallel.max(1) as i64;
                        log.line(&format!(
                            "роль {}: модель {file_mib} МиБ, KV f16 {:.0} МиБ (KV-слоёв {} из {}), \
                             n_batch {} n_ubatch {}, нужно {} МиБ",
                            inst.role,
                            crate::budget::kv_cache_mib(&meta, n_ctx, parallel, KvBits::F16),
                            meta.kv_layer_count(),
                            meta.block_count,
                            spec.n_batch.unwrap_or(2048),
                            spec.n_ubatch.unwrap_or(2048),
                            estimate_need_mib(&meta, file_mib, n_ctx, parallel, KvBits::F16)
                        ));
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
                    let inst = cls.wait_loaded(chat, Duration::from_secs(300), Duration::from_millis(500))?;
                    log.line(&format!("  чат: {}", inst.state_name));
                }
                cluster = Some(Arc::new(ClusterShared::new(cls)));
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
        let backend = Arc::new(ClusterBackend {
            cluster: cluster.clone(),
            ids: ids.clone(),
            model_path,
            n_ctx: n_ctx_map,
            needs: needs.clone(),
            gpu: resolved.gpu.clone(),
            nvml,
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
            whisper: Mutex::new(None),
        });

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

        if let Some(cluster) = self.cluster.clone() {
            let mut cleanup = Vec::new();
            for (role, id) in &self.ids {
                let _ = cluster.with(|c| c.unload(*id));
                let removed = cluster.with(|c| c.remove_instance(*id));
                cleanup.push(format!("{role}: remove_instance -> {removed:?}"));
            }
            if !cleanup.is_empty() {
                self.log
                    .line(&format!("инстансы сняты: {} ({})", cleanup.len(), cleanup.join("; ")));
            }
            self.cleanup.extend(cleanup);
        }

        let mut released = 0;
        while self.pause.depth() > 0 {
            match self.pause.resume() {
                Ok(_) => released += 1,
                Err(e) => {
                    self.log.note(&format!("пауза: не удалось отпустить аренду: {e}"));
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
                if released { "освобождён" } else { "не наш — оставлен" }
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

