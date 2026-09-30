//! Загрузка движка и безопасная обёртка над cluster API.
//!
//! Проверенный факт W0 (спайк 6а, `tools/parity/SPIKES.md` §8): `multi-node-server.dll`
//! грузится только если каталог движка есть в DLL search path — иначе
//! `LoadLibraryExW failed`. Отсюда обязательный `SetDllDirectoryW(<engine dir>)`
//! перед `libloading::Library::new`.

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::os::raw::c_char;

use libloading::Library;

use crate::engine_dir;
use crate::error::{EngineError, Result};
use crate::ffi::{
    ChatRequestRaw, ChatResultRaw, ClusterRaw, DeviceInfoRaw, EmbeddingsRequestRaw, InstanceId,
    InstanceInfoRaw, InstanceParamsRaw, JsonResultRaw, RerankRequestRaw,
};

/// Указатели на экспорты cluster API + библиотека, которая держит их живыми.
pub struct ClusterApi {
    _lib: Arc<Library>,
    lib_name: String,
    pub cluster_create: unsafe extern "C" fn() -> *mut ClusterRaw,
    pub cluster_destroy: unsafe extern "C" fn(*mut ClusterRaw),
    pub cluster_last_error: unsafe extern "C" fn(*const ClusterRaw) -> *const c_char,
    pub list_devices:
        unsafe extern "C" fn(*mut ClusterRaw, *mut *mut DeviceInfoRaw, *mut usize) -> i32,
    pub free_devices: unsafe extern "C" fn(*mut DeviceInfoRaw, usize),
    pub default_instance_params: unsafe extern "C" fn() -> InstanceParamsRaw,
    pub create_instance:
        unsafe extern "C" fn(*mut ClusterRaw, *const InstanceParamsRaw) -> InstanceId,
    pub find_instance_by_name: unsafe extern "C" fn(*mut ClusterRaw, *const c_char) -> InstanceId,
    pub remove_instance: unsafe extern "C" fn(*mut ClusterRaw, InstanceId) -> i32,
    pub list_instances:
        unsafe extern "C" fn(*mut ClusterRaw, *mut *mut InstanceInfoRaw, *mut usize) -> i32,
    pub free_instances: unsafe extern "C" fn(*mut InstanceInfoRaw, usize),
    pub set_instance_retention_mode: unsafe extern "C" fn(*mut ClusterRaw, InstanceId, i32) -> i32,
    pub load_instance: unsafe extern "C" fn(*mut ClusterRaw, InstanceId) -> i32,
    pub unload_instance: unsafe extern "C" fn(*mut ClusterRaw, InstanceId) -> i32,
    pub default_embeddings_request: unsafe extern "C" fn() -> EmbeddingsRequestRaw,
    pub embeddings: unsafe extern "C" fn(
        *mut ClusterRaw,
        *const EmbeddingsRequestRaw,
        *mut JsonResultRaw,
    ) -> i32,
    pub default_rerank_request: unsafe extern "C" fn() -> RerankRequestRaw,
    pub rerank: unsafe extern "C" fn(
        *mut ClusterRaw,
        *const RerankRequestRaw,
        *mut JsonResultRaw,
    ) -> i32,
    pub empty_json_result: unsafe extern "C" fn() -> JsonResultRaw,
    pub json_result_free: unsafe extern "C" fn(*mut JsonResultRaw),
    pub default_chat_request: unsafe extern "C" fn() -> ChatRequestRaw,
    pub chat_complete: unsafe extern "C" fn(
        *mut ClusterRaw,
        *const ChatRequestRaw,
        *mut ChatResultRaw,
    ) -> i32,
    pub empty_chat_result: unsafe extern "C" fn() -> ChatResultRaw,
    pub chat_result_free: unsafe extern "C" fn(*mut ChatResultRaw),
    load_notes: Vec<String>,
}

/// Разрешить символ библиотеки в указатель-функцию (тип выводится из поля).
fn resolve<T: Copy>(lib: &Library, lib_name: &str, name: &str) -> Result<T> {
    let cname = CString::new(name).map_err(|_| EngineError::Other("имя символа с NUL".into()))?;
    unsafe {
        let sym: libloading::Symbol<'_, T> =
            lib.get(cname.as_bytes_with_nul())
                .map_err(|source| EngineError::MissingSymbol {
                    lib: lib_name.to_string(),
                    symbol: name.to_string(),
                    source,
                })?;
        Ok(*sym)
    }
}

impl ClusterApi {
    /// Загрузить библиотеку движка из каталога и разрешить нужные экспорты.
    pub fn load(dir: &Path) -> Result<Arc<ClusterApi>> {
        let notes = prepare_dll_search_path(dir)?;
        let lib_path = engine_dir::engine_lib_path(dir);
        let lib_name = engine_dir::ENGINE_LIB.to_string();
        let lib = unsafe { Library::new(&lib_path) }.map_err(|source| EngineError::Load {
            lib: lib_name.clone(),
            dir: dir.to_path_buf(),
            diagnostics: diagnose_load(dir).join("; "),
            source,
        })?;
        let lib = Arc::new(lib);
        let n = lib_name.as_str();
        let api = ClusterApi {
            cluster_create: resolve(&lib, n, "llama_server_cluster_create")?,
            cluster_destroy: resolve(&lib, n, "llama_server_cluster_destroy")?,
            cluster_last_error: resolve(&lib, n, "llama_server_cluster_last_error")?,
            list_devices: resolve(&lib, n, "llama_server_cluster_list_devices")?,
            free_devices: resolve(&lib, n, "llama_server_cluster_free_devices")?,
            default_instance_params: resolve(&lib, n, "llama_server_cluster_default_instance_params")?,
            create_instance: resolve(&lib, n, "llama_server_cluster_create_instance")?,
            find_instance_by_name: resolve(&lib, n, "llama_server_cluster_find_instance_by_name")?,
            remove_instance: resolve(&lib, n, "llama_server_cluster_remove_instance")?,
            list_instances: resolve(&lib, n, "llama_server_cluster_list_instances")?,
            free_instances: resolve(&lib, n, "llama_server_cluster_free_instances")?,
            set_instance_retention_mode: resolve(
                &lib,
                n,
                "llama_server_cluster_set_instance_retention_mode",
            )?,
            load_instance: resolve(&lib, n, "llama_server_cluster_load_instance")?,
            unload_instance: resolve(&lib, n, "llama_server_cluster_unload_instance")?,
            default_embeddings_request: resolve(
                &lib,
                n,
                "llama_server_cluster_default_embeddings_request",
            )?,
            embeddings: resolve(&lib, n, "llama_server_cluster_embeddings")?,
            default_rerank_request: resolve(
                &lib,
                n,
                "llama_server_cluster_default_rerank_request",
            )?,
            rerank: resolve(&lib, n, "llama_server_cluster_rerank")?,
            empty_json_result: resolve(&lib, n, "llama_server_cluster_empty_json_result")?,
            json_result_free: resolve(&lib, n, "llama_server_cluster_json_result_free")?,
            default_chat_request: resolve(&lib, n, "llama_server_cluster_default_chat_request")?,
            chat_complete: resolve(&lib, n, "llama_server_cluster_chat_complete")?,
            empty_chat_result: resolve(&lib, n, "llama_server_cluster_empty_chat_result")?,
            chat_result_free: resolve(&lib, n, "llama_server_cluster_chat_result_free")?,
            _lib: lib,
            lib_name,
            load_notes: notes,
        };
        Ok(Arc::new(api))
    }

    pub fn lib_name(&self) -> &str {
        &self.lib_name
    }

    /// Что удалось/не удалось сделать с путём поиска DLL (для логов и `hdsw check`).
    pub fn load_notes(&self) -> &[String] {
        &self.load_notes
    }
}

/// Загруженный движок: каталог + разрешённые экспорты.
pub struct Engine {
    dir: PathBuf,
    api: Arc<ClusterApi>,
}

impl Engine {
    /// Загрузить движок из конкретного каталога.
    pub fn load_from_dir(dir: &Path) -> Result<Engine> {
        let api = ClusterApi::load(dir)?;
        Ok(Engine {
            dir: dir.to_path_buf(),
            api,
        })
    }

    /// Загрузить движок, найдя каталог (см. [`engine_dir::candidate_dirs`]).
    pub fn open(config_engine_dir: Option<&Path>) -> Result<Engine> {
        let dir = engine_dir::find_engine_dir(config_engine_dir)?;
        Engine::load_from_dir(&dir)
    }

    /// Каталог движка, из которого загружена библиотека.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Сделать каталог движка текущим (`.`) на время жизни возвращённой защиты.
    ///
    /// **Обязательно** перед вызовами cluster API (находка A1): движок грузит
    /// ggml-бэкенды относительно текущего каталога, и без этого `list_devices`
    /// не видит GPU, а инференс молча идёт на CPU при `n_gpu_layers = -1`
    /// (это и есть корень R32). В `llm-host` защиту держат весь процесс.
    pub fn activate(&self) -> Result<EngineCwd> {
        EngineCwd::enter(&self.dir)
    }

    /// Имя библиотеки cluster API (для логов/`status`).
    pub fn lib_name(&self) -> &str {
        self.api.lib_name()
    }

    /// Записи о подготовке пути поиска DLL (лог/`hdsw check`).
    pub fn load_notes(&self) -> &[String] {
        self.api.load_notes()
    }

    /// Создать кластер — владельца устройств и инстансов.
    pub fn create_cluster(&self) -> Result<crate::cluster::Cluster> {
        let raw = unsafe { (self.api.cluster_create)() };
        if raw.is_null() {
            return Err(EngineError::Other(
                "llama_server_cluster_create вернул NULL".into(),
            ));
        }
        Ok(crate::cluster::Cluster::from_raw(Arc::clone(&self.api), raw))
    }

    /// Адрес непрозрачного указателя кластера (для отладочного вывода/логов).
    pub fn api(&self) -> &Arc<ClusterApi> {
        &self.api
    }
}

/// Подготовка поиска DLL: каталог движка обязан быть виден загрузчику, иначе
/// `LoadLibraryExW failed` (спайк 6а, `SPIKES.md` §8).
///
/// Рабочий механизм (проверено в A1, см. `tools/parity/W2_REPORT.md`):
/// * `SetDefaultDllDirectories(DEFAULT_DIRS | USER_DIRS)` + `AddDllDirectory(dir)` —
///   современный «safe search»: каталог движка становится user-dir, и обычный
///   `LoadLibraryExW(path, 0)`, которым пользуется `libloading`, находит
///   зависимости (`ggml-cuda.dll`, `cublasLt64_13.dll`, …). `SetDllDirectoryW`
///   в этом режиме **игнорируется** — он пути не даёт;
/// * `SetDllDirectoryW` вызываем дополнительно (требование плана §A1): он закрывает
///   случай вложенного `LoadLibrary` без флагов в сборках до safe-search.
///
/// Эффект — на весь процесс (это и нужно `llm-host`/`hdsw check`: один владелец).
/// Возвращает строки для лога/`hdsw check`.
#[cfg(windows)]
pub fn prepare_dll_search_path(dir: &Path) -> Result<Vec<String>> {
    use windows_sys::Win32::System::LibraryLoader::{
        AddDllDirectory, SetDefaultDllDirectories, SetDllDirectoryW,
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_USER_DIRS,
    };

    let mut notes = Vec::new();
    let modes = unsafe {
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_USER_DIRS)
    };
    if modes == 0 {
        notes.push(format!(
            "SetDefaultDllDirectories: ошибка {}",
            last_error_text()
        ));
    } else {
        notes.push("SetDefaultDllDirectories(DEFAULT_DIRS|USER_DIRS): ok".to_string());
    }

    // Каталог движка + вендорские каталоги (ffmpeg и пр.): без них 126 (A1)
    let dirs = engine_dir::dll_search_dirs(dir);
    let mut added = 0usize;
    for d in &dirs {
        let wide = to_wide(d);
        let cookie = unsafe { AddDllDirectory(wide.as_ptr()) };
        if cookie.is_null() {
            notes.push(format!(
                "AddDllDirectory({}): ошибка {}",
                d.display(),
                last_error_text()
            ));
        } else {
            added += 1;
            notes.push(format!("AddDllDirectory({}): ok", d.display()));
        }
    }
    // legacy-путь (требование плана §A1): вложенный LoadLibrary без флагов
    let legacy_ok = unsafe { SetDllDirectoryW(to_wide(dir).as_ptr()) } != 0;
    notes.push(if legacy_ok {
        format!("SetDllDirectoryW({}): ok", dir.display())
    } else {
        format!(
            "SetDllDirectoryW({}): ошибка {}",
            dir.display(),
            last_error_text()
        )
    });
    if added == 0 {
        return Err(EngineError::Other(format!(
            "ни один каталог движка не удалось добавить в путь поиска DLL ({})",
            notes.join("; ")
        )));
    }
    Ok(notes)
}

#[cfg(not(windows))]
pub fn prepare_dll_search_path(_dir: &Path) -> Result<Vec<String>> {
    // macOS/Linux: зависимости разрешаются через @loader_path/rpath
    Ok(vec!["@loader_path/rpath (спец-шага не требуется)".to_string()])
}

fn to_wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// Текст последней ошибки WinAPI вместе с кодом.
#[cfg(windows)]
fn last_error_text() -> String {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() {
        Some(code) => format!("{code} ({e})"),
        None => e.to_string(),
    }
}

/// Диагностика загрузки: перебирает режимы `LoadLibraryExW` и показывает код
/// ошибки. Вызывается автоматически, когда обычная загрузка не удалась, — чтобы
/// «LoadLibraryExW failed» сразу превращалось в разбор причины (первый прогон A1
/// упал именно так). Хендлы не освобождаются: путь диагностический.
#[cfg(windows)]
pub fn diagnose_load(dir: &Path) -> Vec<String> {
    use windows_sys::Win32::System::LibraryLoader::{
        LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        LOAD_LIBRARY_SEARCH_USER_DIRS, LOAD_WITH_ALTERED_SEARCH_PATH,
    };

    let lib = engine_dir::engine_lib_path(dir);
    if !lib.is_file() {
        return vec![format!("нет файла библиотеки: {}", lib.display())];
    }
    let wide = to_wide(&lib);
    let modes: [(&str, u32); 4] = [
        ("flags=0 (libloading)", 0),
        ("LOAD_WITH_ALTERED_SEARCH_PATH", LOAD_WITH_ALTERED_SEARCH_PATH),
        (
            "DLL_LOAD_DIR|DEFAULT_DIRS",
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        ),
        (
            "DLL_LOAD_DIR|USER_DIRS|DEFAULT_DIRS",
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                | LOAD_LIBRARY_SEARCH_USER_DIRS
                | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        ),
    ];
    modes
        .iter()
        .map(|(name, flags)| {
            let h = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), *flags) };
            if h.is_null() {
                format!("{name}: FAIL ({})", last_error_text())
            } else {
                format!("{name}: OK")
            }
        })
        .collect()
}

#[cfg(not(windows))]
pub fn diagnose_load(dir: &Path) -> Vec<String> {
    vec![format!(
        "не-Windows платформа: смотрите ldd/otool для {}",
        engine_dir::engine_lib_path(dir).display()
    )]
}


/// Удерживает каталог движка текущим (`std::env::current_dir`) и возвращает
/// прежний при `Drop`.
///
/// Зачем: движок находит ggml-бэкенды (`ggml-cuda.dll`, `ggml-cpu-*.dll`)
/// **относительно текущего каталога** — проверено в A1 (`tools/parity/W2_REPORT.md`):
/// при запуске из другого каталога грузится только `ggml-rpc.dll`, `list_devices`
/// возвращает пустой список, и инференс уходит на CPU при `n_gpu_layers = -1` (R32).
pub struct EngineCwd {
    dir: PathBuf,
    prev: PathBuf,
}

impl EngineCwd {
    /// Переключить текущий каталог на каталог движка.
    pub fn enter(dir: &Path) -> Result<EngineCwd> {
        let prev = std::env::current_dir()
            .map_err(|e| EngineError::Other(format!("не удалось прочитать текущий каталог: {e}")))?;
        std::env::set_current_dir(dir).map_err(|e| {
            EngineError::Other(format!(
                "не удалось сделать текущим каталог движка {}: {e}",
                dir.display()
            ))
        })?;
        Ok(EngineCwd {
            dir: dir.to_path_buf(),
            prev,
        })
    }

    /// Каталог движка (текущий).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Прежний текущий каталог (вернётся при `Drop`).
    pub fn previous(&self) -> &Path {
        &self.prev
    }
}

impl Drop for EngineCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prev);
    }
}

/// Прочитать C-строку движка (`NULL` → пустая строка).
pub(crate) fn cstr_or_empty(p: *const std::os::raw::c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}


