//! Ошибки обвязки движка `openresearchtools/engine`.

use std::path::PathBuf;

use thiserror::Error;

/// Ошибка загрузки/вызова движка.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("каталог движка не найден (искали: {searched})")]
    EngineDirNotFound { searched: String },

    #[error(
        "не удалось загрузить {lib} из {dir}: {source}; чаще всего это не найденная \
         ЗАВИСИМАЯ DLL (ggml-cuda.dll, ggml-cpu-*.dll) — проверьте, что каталог движка \
         полный; диагностика режимов загрузки: {diagnostics}"
    )]
    Load {
        lib: String,
        dir: PathBuf,
        diagnostics: String,
        #[source]
        source: libloading::Error,
    },

    #[error("в {lib} нет экспорта {symbol}: {source}")]
    MissingSymbol {
        lib: String,
        symbol: String,
        #[source]
        source: libloading::Error,
    },

    /// `rc != 0` от cluster API. У bridge-вызовов дополнительно проверяется
    /// `out.ok` (см. `JsonResult::ensure_ok`).
    #[error("движок вернул ошибку (rc={rc}): {last_error}")]
    Call { rc: i32, last_error: String },

    #[error("инстанс '{name}' не найден в кластере")]
    InstanceNotFound { name: String },

    #[error("NVML недоступен: {0}")]
    Nvml(#[from] nvml_wrapper::error::NvmlError),

    /// Модель роли не найдена: в сообщении обязаны быть **оба** пути — где искали
    /// и куда положить/скачать (`SPIKES.md` §14.7: нестыковка путей ломала старт).
    #[error(
        "GGUF-модель роли '{role}' не найдена. Искали: {searched}. Скачайте её в общий \
         рантайм (`installers/ensure_llama_runtime.ps1 -Models {role}`) или укажите явный \
         путь в `llm.{role}.model` / `llm_server.{role}.model`"
    )]
    ModelNotFound { role: String, searched: String },

    #[error("конфиг {path}: {detail}")]
    Config { path: PathBuf, detail: String },

    #[error("движок вернул не-UTF-8 строку")]
    Utf8,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;
