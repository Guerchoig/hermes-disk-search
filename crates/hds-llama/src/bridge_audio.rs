//! W3: bridge API движка (`llama-server-bridge.dll`) — direct-model путь для
//! batch-транскрибации (`llama_server_bridge_audio_transcriptions_raw`).
//!
//! Почему bridge, а не cluster: cluster-аудио уходит в «native transcription»,
//! которой нужен **execution group** (single-node manual-инстанс даёт
//! `unknown execution_group_id: cluster:manual`, прогон `audio_probe` 01.10.2026).
//! Bridge работает с моделью напрямую (plan §8.3 — канонический путь).
//!
//! Раскладки — из открытого SDK (`bridge/llama_server_bridge.h`, MIT).

use std::ffi::CString;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libloading::Library;

use crate::engine_dir;
use crate::error::{EngineError, Result};

/// `struct llama_server_bridge_params` (порядок полей — как в заголовке SDK).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BridgeParamsRaw {
    pub model_path: *const c_char,
    pub mmproj_path: *const c_char,
    pub cluster_instance_name: *const c_char,
    pub n_ctx: i32,
    pub n_batch: i32,
    pub n_ubatch: i32,
    pub n_parallel: i32,
    pub n_threads: i32,
    pub n_threads_batch: i32,
    pub n_gpu_layers: i32,
    pub main_gpu: i32,
    pub gpu: i32,
    pub no_kv_offload: i32,
    pub mmproj_use_gpu: i32,
    pub cache_ram_mib: i32,
    pub seed: i32,
    pub ctx_shift: i32,
    pub kv_unified: i32,
    pub use_mmap: i32,
    pub use_direct_io: i32,
    pub use_mlock: i32,
    pub no_host: i32,
    pub no_extra_bufts: i32,
    pub devices: *const c_char,
    pub tensor_split: *const c_char,
    pub split_mode: i32,
    pub embedding: i32,
    pub reranking: i32,
    pub pooling_type: i32,
}

/// `struct llama_server_bridge_audio_raw_request`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BridgeAudioRawRequestRaw {
    pub audio_bytes: *const u8,
    pub audio_bytes_len: usize,
    pub audio_format: *const c_char,
    pub metadata_json: *const c_char,
    pub ffmpeg_convert: i32,
}

/// `struct llama_server_bridge_json_result`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BridgeJsonResultRaw {
    pub ok: i32,
    pub status: i32,
    pub json: *mut c_char,
    pub error_json: *mut c_char,
}

/// Итог bridge JSON-вызова.
pub struct BridgeOutcome {
    pub ok: bool,
    pub status: i32,
    pub json: String,
    pub error: String,
}

fn cstr_or_empty(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

/// Загруженная bridge-библиотека и её символы.
pub struct BridgeAudio {
    _lib: Arc<Library>,
    create: unsafe extern "C" fn(*const BridgeParamsRaw) -> *mut std::ffi::c_void,
    destroy: unsafe extern "C" fn(*mut std::ffi::c_void),
    default_params: unsafe extern "C" fn() -> BridgeParamsRaw,
    default_audio_raw: unsafe extern "C" fn() -> BridgeAudioRawRequestRaw,
    audio_raw: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *const BridgeAudioRawRequestRaw,
        *mut BridgeJsonResultRaw,
    ) -> i32,
    json_free: unsafe extern "C" fn(*mut BridgeJsonResultRaw),
    last_error: unsafe extern "C" fn(*const std::ffi::c_void) -> *const c_char,
}

/// Разрешить символ библиотеки.
fn resolve<T: Copy>(lib: &Library, name: &str) -> Result<T> {
    let cname = CString::new(name).map_err(|_| EngineError::Other("имя символа с NUL".into()))?;
    unsafe {
        let sym: libloading::Symbol<'_, T> =
            lib.get(cname.as_bytes_with_nul())
                .map_err(|source| EngineError::MissingSymbol {
                    lib: engine_dir::BRIDGE_LIB.to_string(),
                    symbol: name.to_string(),
                    source,
                })?;
        Ok(*sym)
    }
}

impl BridgeAudio {
    /// Загрузить `llama-server-bridge.dll` из каталога движка.
    ///
    /// Каталог движка должен быть виден загрузчику — вызывающий обязан сначала
    /// [`crate::engine::ClusterApi::load`] / `prepare_dll_search_path` (как `llm-host`).
    pub fn load(dir: &Path) -> Result<BridgeAudio> {
        let path: PathBuf = engine_dir::bridge_lib_path(dir);
        let lib = unsafe { Library::new(&path) }.map_err(|source| EngineError::Load {
            lib: engine_dir::BRIDGE_LIB.to_string(),
            dir: dir.to_path_buf(),
            diagnostics: format!("bridge path: {}", path.display()),
            source,
        })?;
        let lib = Arc::new(lib);
        let api = BridgeAudio {
            create: resolve(&lib, "llama_server_bridge_create")?,
            destroy: resolve(&lib, "llama_server_bridge_destroy")?,
            default_params: resolve(&lib, "llama_server_bridge_default_params")?,
            default_audio_raw: resolve(&lib, "llama_server_bridge_default_audio_raw_request")?,
            audio_raw: resolve(&lib, "llama_server_bridge_audio_transcriptions_raw")?,
            json_free: resolve(&lib, "llama_server_bridge_json_result_free")?,
            last_error: resolve(&lib, "llama_server_bridge_last_error")?,
            _lib: lib,
        };
        Ok(api)
    }

    /// Создать **постоянный** bridge (для роли whisper — один на владельца).
    ///
    /// Audio-only: `model` = `None` («For audio-only use, `model_path` may be omitted»,
    /// `docs/bridge-audio-dll.md`); модель whisper задаётся в `metadata_json.whisper_model`.
    pub fn create(&self, model: Option<&Path>, gpu: Option<i32>, n_gpu_layers: i32) -> Result<Bridge> {
        let mut keep: Vec<CString> = Vec::new();
        let model_ptr = match model {
            Some(m) => {
                let c = CString::new(m.to_string_lossy().as_bytes())
                    .map_err(|_| EngineError::Other("NUL в пути модели".into()))?;
                keep.push(c);
                keep.last().unwrap().as_ptr()
            }
            None => std::ptr::null(),
        };
        let mut p = unsafe { (self.default_params)() };
        p.model_path = model_ptr;
        p.gpu = gpu.unwrap_or(-1);
        p.n_gpu_layers = n_gpu_layers;
        let handle = unsafe { (self.create)(&p) };
        if handle.is_null() {
            return Err(EngineError::Other("bridge_create вернул NULL".into()));
        }
        Ok(Bridge {
            handle,
            destroy: self.destroy,
            audio_raw: self.audio_raw,
            default_audio_raw: self.default_audio_raw,
            json_free: self.json_free,
            last_error: self.last_error,
            _lib: Arc::clone(&self._lib),
            _keep: keep,
        })
    }

    /// Создать bridge под модель, выполнить raw-транскрибацию, освободить ресурсы.
    ///
    /// Для **audio-only** `model` = `None` (официальный пример `docs/bridge-audio-dll.md`:
    /// «For audio-only use, `model_path` may be omitted»); модель whisper задаётся в
    /// `metadata_json` ключом `whisper_model` (путь к `.bin` GGML — маршрут whisper.cpp,
    /// не llama.cpp GGUF).
    #[allow(clippy::too_many_arguments)]
    pub fn transcribe_raw(
        &self,
        model: Option<&Path>,
        gpu: Option<i32>,
        n_gpu_layers: i32,
        bytes: &[u8],
        audio_format: &str,
        metadata_json: &str,
        ffmpeg_convert: bool,
    ) -> Result<BridgeOutcome> {
        let model_c = match model {
            Some(m) => Some(
                CString::new(m.to_string_lossy().as_bytes())
                    .map_err(|_| EngineError::Other("NUL в пути модели".into()))?,
            ),
            None => None,
        };
        let fmt_c = CString::new(audio_format.as_bytes())
            .map_err(|_| EngineError::Other("NUL в audio_format".into()))?;
        let meta_c = CString::new(metadata_json.as_bytes())
            .map_err(|_| EngineError::Other("NUL в metadata_json".into()))?;

        let mut p = unsafe { (self.default_params)() };
        p.model_path = model_c
            .as_ref()
            .map(|c| c.as_ptr())
            .unwrap_or(std::ptr::null());
        p.gpu = gpu.unwrap_or(-1);
        p.n_gpu_layers = n_gpu_layers;

        let bridge = unsafe { (self.create)(&p) };
        if bridge.is_null() {
            return Err(EngineError::Other("bridge_create вернул NULL".into()));
        }

        let mut req = unsafe { (self.default_audio_raw)() };
        req.audio_bytes = bytes.as_ptr();
        req.audio_bytes_len = bytes.len();
        req.audio_format = fmt_c.as_ptr();
        req.metadata_json = meta_c.as_ptr();
        req.ffmpeg_convert = ffmpeg_convert as i32;

        let mut out = BridgeJsonResultRaw {
            ok: 0,
            status: 0,
            json: std::ptr::null_mut(),
            error_json: std::ptr::null_mut(),
        };
        let rc = unsafe { (self.audio_raw)(bridge, &req, &mut out) };
        let last = cstr_or_empty(unsafe { (self.last_error)(bridge) });
        let outcome = BridgeOutcome {
            ok: out.ok == 1,
            status: out.status,
            json: cstr_or_empty(out.json),
            error: if !out.error_json.is_null() {
                cstr_or_empty(out.error_json)
            } else {
                last
            },
        };
        unsafe {
            (self.json_free)(&mut out);
            (self.destroy)(bridge);
        }
        if rc != 0 && outcome.error.is_empty() {
            return Err(EngineError::Call {
                rc,
                last_error: format!("bridge rc={rc}"),
            });
        }
        Ok(outcome)
    }
}

/// Постоянный bridge движка: создаётся один раз (роль `whisper`), переиспользуется
/// между запросами; уничтожается в [`Drop`].
pub struct Bridge {
    handle: *mut std::ffi::c_void,
    destroy: unsafe extern "C" fn(*mut std::ffi::c_void),
    audio_raw: unsafe extern "C" fn(
        *mut std::ffi::c_void,
        *const BridgeAudioRawRequestRaw,
        *mut BridgeJsonResultRaw,
    ) -> i32,
    default_audio_raw: unsafe extern "C" fn() -> BridgeAudioRawRequestRaw,
    json_free: unsafe extern "C" fn(*mut BridgeJsonResultRaw),
    last_error: unsafe extern "C" fn(*const std::ffi::c_void) -> *const c_char,
    _lib: Arc<Library>,
    _keep: Vec<CString>,
}

impl Bridge {
    /// Raw-транскрибация через уже созданный bridge (модель — из `metadata_json`).
    pub fn transcribe_raw(
        &self,
        bytes: &[u8],
        audio_format: &str,
        metadata_json: &str,
        ffmpeg_convert: bool,
    ) -> Result<BridgeOutcome> {
        let fmt_c = CString::new(audio_format.as_bytes())
            .map_err(|_| EngineError::Other("NUL в audio_format".into()))?;
        let meta_c = CString::new(metadata_json.as_bytes())
            .map_err(|_| EngineError::Other("NUL в metadata_json".into()))?;
        let mut req = unsafe { (self.default_audio_raw)() };
        req.audio_bytes = bytes.as_ptr();
        req.audio_bytes_len = bytes.len();
        req.audio_format = fmt_c.as_ptr();
        req.metadata_json = meta_c.as_ptr();
        req.ffmpeg_convert = ffmpeg_convert as i32;
        let mut out = BridgeJsonResultRaw {
            ok: 0,
            status: 0,
            json: std::ptr::null_mut(),
            error_json: std::ptr::null_mut(),
        };
        let rc = unsafe { (self.audio_raw)(self.handle, &req, &mut out) };
        let last = cstr_or_empty(unsafe { (self.last_error)(self.handle) });
        let outcome = BridgeOutcome {
            ok: out.ok == 1,
            status: out.status,
            json: cstr_or_empty(out.json),
            error: if !out.error_json.is_null() {
                cstr_or_empty(out.error_json)
            } else {
                last
            },
        };
        unsafe {
            (self.json_free)(&mut out);
        }
        if rc != 0 && outcome.error.is_empty() {
            return Err(EngineError::Call {
                rc,
                last_error: format!("bridge rc={rc}"),
            });
        }
        Ok(outcome)
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.destroy)(self.handle) };
        }
    }
}
