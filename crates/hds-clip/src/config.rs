//! Разрешение путей к ONNX-моделям и токенайзеру (§7): конфиг → `tools/parity/out/clip_onnx`.

use std::path::PathBuf;

use hds_core::config::{dig, Config};

/// Настройки CLIP из конфига.
#[derive(Debug, Clone)]
pub struct ClipConfig {
    /// `index.clip` — включён ли CLIP.
    pub enabled: bool,
    /// vision-ONNX (`clip_vision.onnx`).
    pub vision_model: PathBuf,
    /// text-ONNX с pooling+Dense (`clip_text_dense.onnx`).
    pub text_model: PathBuf,
    /// `tokenizer.json` мультиязычного text-энкодера.
    pub tokenizer: PathBuf,
}

impl ClipConfig {
    /// Собрать из конфига; пути по умолчанию — каталог экспорта спайка 4.
    pub fn from_config(cfg: &Config) -> Self {
        let base = super::default_onnx_dir();
        let enabled = dig(cfg, "index.clip").and_then(|v| v.as_bool()).unwrap_or(true);
        let vision_model = path_or(cfg, "index.clip_vision_model", base.join("vision").join("clip_vision.onnx"));
        let text_model = path_or(cfg, "index.clip_text_model", base.join("text").join("clip_text_dense.onnx"));
        let tokenizer = path_or(cfg, "index.clip_tokenizer", resolve_tokenizer());
        ClipConfig {
            enabled,
            vision_model,
            text_model,
            tokenizer,
        }
    }

    /// Готовы ли модели физически на диске.
    pub fn models_present(&self) -> bool {
        self.vision_model.is_file() && self.text_model.is_file() && self.tokenizer.is_file()
    }
}

/// Значение-путь из конфига (непустое) либо умолчание.
fn path_or(cfg: &Config, dotted: &str, default: PathBuf) -> PathBuf {
    dig(cfg, dotted)
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(default)
}

/// `tokenizer.json` мультиязычного энкодера: сначала рядом с репо
/// (`tools/parity/out/clip_onnx/text/tokenizer.json`), затем в HF-кэше.
fn resolve_tokenizer() -> PathBuf {
    let local = super::default_onnx_dir().join("text").join("tokenizer.json");
    if local.is_file() {
        return local;
    }
    // HF-кэш: models--sentence-transformers--clip-ViT-B-32-multilingual-v1/snapshots/*/tokenizer.json
    if let Ok(home) = std::env::var("USERPROFILE") {
        let hub = PathBuf::from(home)
            .join(".cache")
            .join("huggingface")
            .join("hub");
        let dir = hub.join("models--sentence-transformers--clip-ViT-B-32-multilingual-v1").join("snapshots");
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let cand = e.path().join("tokenizer.json");
                if cand.is_file() {
                    return cand;
                }
            }
        }
    }
    local
}
