//! Единственное ONNX-окружение процесса для диаризации.
//!
//! `ort 2.0.0-rc.15` разрешает **один** `Environment` на процесс: второй
//! `ort::init().build()` вернёт `only one environment is allowed per process`.
//! Поэтому окружение создаётся через [`OnceLock`] и отдаётся клонами (`Environment`
//! — это `Arc`, клонирование бесплатное): так его получают и `Sortformer`, и
//! (при необходимости) `hds-clip` через `Clip::load_with_env`.
//!
//! Сейчас `hds` (CLIP) и `llm_host` (диаризация) — разные бинари, поэтому
//! коллизии нет; правило на будущее — если бинарь слинкует оба крейта, передавать
//! окружение снаружи, а не инициализировать ORT повторно.

use std::sync::OnceLock;

use parakeet_rs::ort::environment::Environment;

use crate::{DiarizeError, Result};

type EnvResult = std::result::Result<Environment, String>;

static ENV: OnceLock<EnvResult> = OnceLock::new();

/// Общее ONNX-окружение процесса (создаётся при первом обращении, ошибка кэшируется).
pub fn ort_env() -> Result<&'static Environment> {
    match ENV.get_or_init(|| {
        parakeet_rs::ort::init()
            .with_name("hds-diarize")
            .build()
            .map_err(|e| format!("ONNX Runtime environment: {e}"))
    }) {
        Ok(env) => Ok(env),
        Err(e) => Err(DiarizeError::Tool(e.clone())),
    }
}

/// Клон общего окружения — его принимает `ExecutionConfig::with_environment`.
pub fn ort_env_clone() -> Result<Environment> {
    Ok(ort_env()?.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Повторный вызов возвращает **то же** окружение (OnceLock), а не падает на
    /// «only one environment is allowed per process».
    #[test]
    fn env_is_singleton() {
        let a = match ort_env() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[skip] ONNX Runtime недоступен: {e}");
                return;
            }
        };
        let b = ort_env().expect("повторный вызов");
        assert!(std::ptr::eq(a, b), "окружение должно быть одним на процесс");
        // Клон — отдельная обёртка над тем же `Arc`, адрес другой; важно, что он
        // создаётся без повторной инициализации ORT.
        let _ = ort_env_clone().expect("клон окружения");
    }
}
