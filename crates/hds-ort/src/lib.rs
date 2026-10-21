//! Единственное ONNX-окружение процесса (`F7`, ревью `M4`).
//!
//! `ort 2.0.0-rc.15` разрешает **один** `Environment` на процесс: второй
//! `ort::init().build()` возвращает `only one environment is allowed per process`.
//! Раньше окружение создавалось в двух местах — `hds_clip::env` (CLIP при
//! индексации) и `hds_diarize::ort_env` (Nemotron-3) — и это не сталкивалось
//! только потому, что `hds` и `llm_host` — разные бинари. Но `hds` уже линкует
//! оба крейта (`hds-index → hds-clip` и `hds-index → hds-diarize`), так что любой
//! будущий вызов диаризации из процесса `hds` (например, прогон из UI) падал бы.
//!
//! Теперь окружение одно и живёт здесь: `hds-clip` и `hds-diarize` — тонкие
//! обёртки над [`environment`]/[`clone_environment`]. **Не создавайте второе**
//! окружение через `ort::init()` нигде в продукте.

use std::sync::OnceLock;

use ort::environment::Environment;

type EnvResult = std::result::Result<Environment, String>;

static ENV: OnceLock<EnvResult> = OnceLock::new();

/// Общее ONNX-окружение процесса (создаётся при первом обращении, ошибка
/// кэшируется — повторные вызовы не пробуют инициализировать ORT снова).
pub fn environment() -> std::result::Result<&'static Environment, String> {
    match ENV.get_or_init(|| {
        ort::init()
            .with_name("hds")
            .build()
            .map_err(|e| format!("ONNX Runtime environment: {e}"))
    }) {
        Ok(env) => Ok(env),
        Err(e) => Err(e.clone()),
    }
}

/// Клон общего окружения — его принимают `Clip::load_with_env` /
/// `Diarizer::load_with_env` (`Environment` — это `Arc`, клон бесплатный).
pub fn clone_environment() -> std::result::Result<Environment, String> {
    environment().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Повторный вызов возвращает **то же** окружение (OnceLock), а не падает на
    /// «only one environment is allowed per process».
    #[test]
    fn env_is_singleton() {
        let a = match environment() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[skip] ONNX Runtime недоступен: {e}");
                return;
            }
        };
        let b = environment().expect("повторный вызов");
        assert!(std::ptr::eq(a, b), "окружение должно быть одним на процесс");
        let _ = clone_environment().expect("клон окружения");
    }
}
