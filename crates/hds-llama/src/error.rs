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

    #[error("движок вернул не-UTF-8 строку")]
    Utf8,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;
