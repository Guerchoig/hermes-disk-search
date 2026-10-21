//! Единственное ONNX-окружение процесса (`ort::Environment`).
//!
//! `ort 2.0.0-rc.15` разрешает **один** environment на процесс: повторный
//! `ort::init().build()` возвращает `only one environment is allowed per process`,
//! а `Session::builder(&Environment)` теперь требует окружение явно. Поэтому
//! окружение создаётся один раз через [`OnceLock`] и переиспользуется всеми
//! сессиями CLIP; неудача кэшируется (повторные вызовы не пробуют снова).
//!
//! Если бинарь слинкует и `hds-clip`, и `hds-diarize` (сейчас `hds` и `llm_host` —
//! разные бинари), общее окружение передаётся снаружи: `Clip::load_with_env` и
//! `Diarizer::load_with_env` (W3). Тогда этот модуль не вызывается.

use std::sync::OnceLock;

use hds_core::error::{CoreError, Result};
use ort::environment::Environment;

type EnvResult = std::result::Result<Environment, String>;

static ENV: OnceLock<EnvResult> = OnceLock::new();

/// Общее ONNX-окружение этого процесса (создаётся при первом обращении).
pub fn environment() -> Result<&'static Environment> {
    match ENV.get_or_init(|| {
        ort::init()
            .with_name("hds-clip")
            .build()
            .map_err(|e| format!("ONNX Runtime environment: {e}"))
    }) {
        Ok(env) => Ok(env),
        Err(e) => Err(CoreError::Other(e.clone())),
    }
}
