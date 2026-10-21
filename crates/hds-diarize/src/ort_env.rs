//! Единственное ONNX-окружение процесса для диаризации.
//!
//! `ort 2.0.0-rc.15` разрешает **один** `Environment` на процесс: второй
//! `ort::init().build()` вернёт `only one environment is allowed per process`.
//!
//! F7 (ревью M4): окружение общее с CLIP и живёт в `hds-ort` (один `OnceLock` на
//! процесс). Здесь — совместимые обёртки (`ort_env`/`ort_env_clone`), чтобы
//! внешний код не менялся. **Не создавайте** второе окружение через `ort::init()`
//! ни в `hds-diarize`, ни в других крейтах продукта — иначе `hds` (CLIP +
//! диаризация в одном процессе) снова упадёт.

use parakeet_rs::ort::environment::Environment;

use crate::{DiarizeError, Result};

/// Общее ONNX-окружение процесса (создаётся при первом обращении, ошибка кэшируется).
pub fn ort_env() -> Result<&'static Environment> {
    hds_ort::environment().map_err(DiarizeError::Tool)
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
