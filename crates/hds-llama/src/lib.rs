//! `hds-llama` — хост LLM/эмбеддингов (трек A плана W2).
//!
//! Состав на текущий момент (шаги A1–A2):
//! * [`engine_dir`] — поиск каталога движка (конфиг → `%APPDATA%` → рядом с `.exe`);
//! * [`engine`] — загрузка `multi-node-server.dll` через `libloading`
//!   с обязательным `SetDllDirectoryW`, вендорскими каталогами и `EngineCwd`;
//! * [`ffi`] — точные структуры cluster API движка;
//! * [`cluster`] — устройства, инстансы (`create/load/unload/remove`),
//!   `embeddings`/`rerank`/`chat_complete`;
//! * [`device`] — правила выбора устройства и маппинг `gpu.device_index`;
//! * [`vram`] — бюджет VRAM по NVML (`VramProbe`, `VramSampler`) и фолбэк;
//! * [`runtime`] — общий llama-рантайм машины (пути, `current.json`, `shared:<role>`);
//! * [`config`] — чтение `config.yaml` (`llm_server.*`/`llm.*`/`gpu.*`);
//! * [`registry`] — план инстансов по ролям (A2).
//!
//! Дальше по треку A: A3 адресация клиентов, A4 диспетчер VRAM, A5 фасад `:8010–8012`.

pub mod budget;
pub mod cluster;
pub mod config;
pub mod device;
pub mod dispatch;
pub mod engine;
pub mod engine_dir;
pub mod error;
pub mod ffi;
pub mod gguf;
pub mod pause;
pub mod registry;
pub mod runtime;
pub mod status;
pub mod vram;

pub use budget::{check_fit, estimate_need_mib, kv_cache_mib, Fit};
pub use cluster::{ChatOutcome, Cluster, Device, Instance, InstanceSpec, JsonOutcome, Metrics};
pub use config::{GpuConfig, GpuPolicy, LlmHostConfig, Mode, RoleConfig};
pub use device::{selection_from_config_index, DeviceSelection};
pub use dispatch::{plan_indexing, plan_query, Action, Demand, InstanceUse, Plan, Verdict};
pub use engine::{ClusterApi, Engine};
pub use engine_dir::{find_engine_dir, ENGINE_LIB};
pub use error::{EngineError, Result};
pub use gguf::{read_meta, GgufMeta, KvBits};
pub use pause::{read_heartbeat, IndexPause, PauseLease};
pub use registry::{plan, plan_strict, PlannedInstance, RolePlan};
pub use runtime::{read_current, resolve_model, runtime_dir, RuntimePaths};
pub use status::StatusReport;
pub use vram::{NvmlProbe, VramProbe, VramSampler, VramSnapshot, VramSource};
