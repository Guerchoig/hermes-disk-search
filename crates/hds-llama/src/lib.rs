//! `hds-llama` — хост LLM/эмбеддингов (трек A плана W2).
//!
//! Состав:
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
//! * [`registry`] — план инстансов по ролям (A2);
//! * [`budget`]/[`gguf`] — метаданные GGUF и оценка «модель + KV» (A4 шаг 1);
//! * [`pause`]/[`dispatch`]/[`status`] — `index.pause`, диспетчер VRAM (A4 шаг 2)
//!   и отчёт `llm-host status`;
//! * [`http`]/[`facade`] — свой мини-HTTP и OpenAI-совместимые маршруты (A5),
//!   включая внутренний API `/internal/*` для CLI (A6);
//! * [`host`] — резидентный `llm-host`: инстансы + фасад + диспетчер + режимы
//!   `embedded`/`facade`/`off` (A6);
//! * [`resident`] — `data/llm-host.pid` и `data/logs/llm-host.log` (A6);
//! * [`gate`] — шлюз к движку: метка занятости (`кто держит и с какого времени`)
//!   и ожидание по бюджету вместо бесконечного (L1, `W4_REPORT.md` §15).
//!
//! Запуск (из корня репозитория): `cargo run -p hds-llama --release --bin llm_host -- run`
//! (подробности — `tools/parity/W2_REPORT.md` §9.10).

pub mod bridge_audio;
pub mod budget;
pub mod cluster;
pub mod config;
pub mod device;
pub mod dispatch;
pub mod engine;
pub mod engine_dir;
pub mod error;
pub mod facade;
pub mod ffi;
pub mod gate;
pub mod gguf;
pub mod gpuattr;
pub mod host;
pub mod http;
pub mod pause;
pub mod registry;
pub mod resident;
pub mod runtime;
pub mod status;
pub mod vram;
pub mod whisper;

pub use bridge_audio::{BridgeAudio, BridgeOutcome};
pub use budget::{check_fit, estimate_need_mib, kv_cache_mib, Fit};
pub use cluster::{ChatOutcome, Cluster, Device, Instance, InstanceSpec, JsonOutcome, Metrics};
pub use config::{GpuConfig, GpuPolicy, LlmHostConfig, Mode, RoleConfig};
pub use device::{selection_from_config_index, DeviceSelection};
pub use dispatch::{plan_indexing, plan_query, Action, Demand, InstanceUse, Plan, Verdict};
pub use engine::{ClusterApi, Engine};
pub use engine_dir::{find_engine_dir, ENGINE_LIB};
pub use error::{EngineError, Result};
pub use facade::{route, ChatRequest, Route, Thinking, Usage};
pub use gguf::{read_meta, GgufMeta, KvBits};
pub use host::{Host, HostConfig, LocalStatus, LocalStatusArgs};
pub use http::client_json;
pub use pause::{read_heartbeat, IndexPause, PauseLease};
pub use registry::{plan, plan_strict, PlannedInstance, RolePlan};
pub use resident::{Log, PidFile};
pub use runtime::{read_current, resolve_model, runtime_dir, RuntimePaths};
pub use status::StatusReport;
pub use vram::{NvmlProbe, VramProbe, VramSampler, VramSnapshot, VramSource};
pub use whisper::{parse_srt, Transcript, Whisper, WhisperSegment};
