//! Сырой FFI-слой cluster API движка — точное соответствие
//! `bridge/llama_server_cluster.h` из `github.com/openresearchtools/engine`
//! (структуры выписаны в `PLAN_W2_LLM_HOST.md` §11).
//!
//! Правила SDK, которые соблюдаются вызывающим кодом:
//! * структуры всегда инициализируются хелперами движка
//!   (`llama_server_cluster_default_*` / `*_empty_*`) — руками поля не заполняем;
//! * строки, выделенные движком, освобождаются только его `*_free_*`;
//! * `rc == 0` означает лишь успешный путь вызова: у результатов дополнительно
//!   проверяется `ok`/`error` (`JsonResult::ensure_ok`).

use std::os::raw::c_char;

/// Непрозрачный `struct llama_server_cluster`.
#[repr(C)]
pub struct ClusterRaw {
    _private: [u8; 0],
}

/// Идентификатор инстанса (`int64_t`).
pub type InstanceId = i64;

/// `enum llama_server_cluster_instance_retention_mode`.
pub mod retention {
    /// Инстанс остаётся загруженным (роль чата).
    pub const KEEP_LOADED: i32 = 1;
    /// Загружается по запросу и выгружается по grace-таймауту.
    pub const LOAD_ON_DEMAND: i32 = 2;
}

/// `enum llama_server_cluster_instance_state`.
pub mod state {
    pub const UNLOADED: i32 = 0;
    pub const LOADING: i32 = 1;
    pub const LOADED: i32 = 2;
    pub const SERVING: i32 = 3;
    pub const GRACE: i32 = 4;
    pub const FAILED: i32 = 5;

    /// Имя состояния для логов/`status --json`.
    pub fn name(v: i32) -> &'static str {
        match v {
            UNLOADED => "UNLOADED",
            LOADING => "LOADING",
            LOADED => "LOADED",
            SERVING => "SERVING",
            GRACE => "GRACE",
            FAILED => "FAILED",
            _ => "UNKNOWN",
        }
    }

    /// Загружен ли инстанс (инстанс в момент запроса переходит в SERVING).
    pub fn is_loaded(v: i32) -> bool {
        v == LOADED || v == SERVING
    }
}

/// `enum llama_server_cluster_instance_model_kind`.
pub mod model_kind {
    pub const TEXT: i32 = 0;
    pub const VISION: i32 = 1;
    pub const EMBEDDINGS: i32 = 2;
    pub const RERANK: i32 = 3;
    pub const WHISPER: i32 = 4;
    pub const REALTIME_AUDIO: i32 = 5;
    pub const DIARIZATION: i32 = 6;
}

/// `struct llama_server_cluster_device_info`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceInfoRaw {
    pub bridge_device_index: i32,
    pub type_: i32,
    pub memory_free: u64,
    pub memory_total: u64,
    pub backend: *mut c_char,
    pub name: *mut c_char,
    pub description: *mut c_char,
}

/// `struct llama_server_cluster_instance_params`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InstanceParamsRaw {
    pub name: *const c_char,
    pub model_path: *const c_char,
    pub mmproj_path: *const c_char,
    pub diarization_model_path: *const c_char,
    pub execution_group_id: *const c_char,
    pub rpc_servers: *const c_char,
    /// Упорядоченные bridge-индексы устройств инстанса (NULL — выбор движка).
    pub manual_devices_csv: *const c_char,
    pub manual_tensor_split: *const c_char,
    pub retention_mode: i32,
    pub load_on_demand_grace_seconds: i32,
    pub embedding: i32,
    pub reranking: i32,
    pub model_kind: i32,
    pub allow_cpu: i32,
    pub allow_integrated_gpu: i32,
    pub n_ctx: i32,
    pub n_batch: i32,
    pub n_ubatch: i32,
    pub n_parallel: i32,
    pub n_threads: i32,
    pub n_threads_batch: i32,
    pub n_gpu_layers: i32,
}

/// `struct llama_server_cluster_instance_info`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InstanceInfoRaw {
    pub instance_id: InstanceId,
    pub name: *mut c_char,
    pub model_path: *mut c_char,
    pub mmproj_path: *mut c_char,
    pub diarization_model_path: *mut c_char,
    pub execution_group_id: *mut c_char,
    pub rpc_servers: *mut c_char,
    pub retention_mode: i32,
    pub load_on_demand_grace_seconds: i32,
    pub model_kind: i32,
    pub state: i32,
    pub active_request_count: i32,
    pub queued_request_count: i32,
    pub n_parallel: i32,
    pub grace_deadline_unix_ms: i64,
    pub last_error: *mut c_char,
}

/// `struct llama_server_cluster_inference_metrics`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InferenceMetricsRaw {
    pub loaded_this_call: i32,
    pub used_rpc: i32,
    pub rpc_server_count: i32,
    pub prompt_tokens: i32,
    pub decoded_tokens: i32,
    pub request_bytes: u64,
    pub model_bytes: u64,
    pub mmproj_bytes: u64,
    pub queue_wait_ms: f64,
    pub load_ms: f64,
    pub prompt_ms: f64,
    pub predicted_ms: f64,
    pub request_total_ms: f64,
    pub prompt_tokens_per_second: f64,
    pub decode_tokens_per_second: f64,
    pub total_tokens_per_second: f64,
}

/// `struct llama_server_cluster_embeddings_request`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EmbeddingsRequestRaw {
    pub instance_id: InstanceId,
    pub body_json: *const c_char,
    /// `1` — тело в формате `/v1/embeddings`, `0` — `/embeddings`.
    pub oai_compat: i32,
}

/// `struct llama_server_cluster_rerank_request`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RerankRequestRaw {
    pub instance_id: InstanceId,
    pub body_json: *const c_char,
}

/// `struct llama_server_cluster_audio_raw_request` (batch-транскрибация, §8.3).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioRawRequestRaw {
    pub instance_id: InstanceId,
    pub audio_bytes: *const u8,
    pub audio_bytes_len: usize,
    pub audio_format: *const c_char,
    pub metadata_json: *const c_char,
    pub ffmpeg_convert: i32,
    pub enable_diarization: i32,
    pub diarization_model_path: *const c_char,
}

/// `struct llama_server_cluster_json_result` (embeddings/rerank/audio).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JsonResultRaw {
    pub ok: i32,
    pub status: i32,
    pub json: *mut c_char,
    pub error: *mut c_char,
    pub metrics: InferenceMetricsRaw,
}

/// `struct llama_server_cluster_chat_request`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ChatRequestRaw {
    pub instance_id: InstanceId,
    pub prompt: *const c_char,
    pub n_predict: i32,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: i32,
    pub min_p: f32,
    pub repeat_last_n: i32,
    pub repeat_penalty: f32,
    pub reasoning: *const c_char,
    pub reasoning_budget: i32,
    pub reasoning_format: *const c_char,
}

/// `struct llama_server_cluster_chat_result`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ChatResultRaw {
    pub ok: i32,
    pub text: *mut c_char,
    pub error: *mut c_char,
    pub metrics: InferenceMetricsRaw,
}
