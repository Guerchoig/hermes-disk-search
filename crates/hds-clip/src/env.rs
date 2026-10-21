//! Единственное ONNX-окружение процесса (`ort::Environment`).
//!
//! `ort 2.0.0-rc.15` разрешает **один** environment на процесс: повторный
//! `ort::init().build()` возвращает `only one environment is allowed per process`,
//! а `Session::builder(&Environment)` требует окружение явно.
//!
//! F7 (ревью M4): окружение общее для CLIP и диаризации и живёт в `hds-ort`
//! (тонкий крейт с одним `OnceLock`). Здесь — только совместимая обёртка:
//! `environment()` возвращает то же окружение, что `hds_diarize::ort_env()`, и
//! `hds` (CLIP + диаризация в одном процессе) не получает двух окружений.
//! Своё окружение через `ort::init()` в продукте **не создавать**.

use hds_core::error::{CoreError, Result};
use ort::environment::Environment;

/// Общее ONNX-окружение этого процесса (создаётся при первом обращении).
pub fn environment() -> Result<&'static Environment> {
    hds_ort::environment().map_err(CoreError::Other)
}
