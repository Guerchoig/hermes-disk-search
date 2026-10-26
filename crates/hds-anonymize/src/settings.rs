//! Чтение `anonymize.*` (§2.6) → аргументы воркера.
//!
//! Rust — единственный источник истины: воркеру уходят абсолютные пути и
//! готовые значения, `config.yaml` он не читает (§5.2). `lease_timeout_sec`/
//! `gpu` принимаются, но используются только в P3 (lease.rs).

use std::path::{Path, PathBuf};

use hds_core::config::{dig, Config};

fn text(cfg: &Config, key: &str, default: &str) -> String {
    dig(cfg, key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn number(cfg: &Config, key: &str, default: f64) -> f64 {
    dig(cfg, key).and_then(|v| v.as_f64()).unwrap_or(default)
}

fn integer(cfg: &Config, key: &str, default: i64) -> i64 {
    dig(cfg, key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn flag(cfg: &Config, key: &str, default: bool) -> bool {
    dig(cfg, key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// Каталог моделей: `HDS_MODELS_DIR` → `<dir>/anonymize`; иначе `<root>/models/anonymize`.
pub fn models_dir(root: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("HDS_MODELS_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("anonymize");
        }
    }
    root.join("models").join("anonymize")
}

/// Настройки анонимизации, собранные из конфига.
#[derive(Debug, Clone)]
pub struct Settings {
    /// `anonymize.output_dir` (по умолчанию `data/anonymized`).
    pub output_dir: String,
    /// `anonymize.db_path` (`data/anonymize.db`).
    pub db_path: String,
    /// `anonymize.model` (по умолчанию — как в anonymizer_proxy).
    pub model: String,
    /// `anonymize.threshold` (0.3 — НЕ 0.5, §2.6).
    pub threshold: f64,
    pub max_input_chars: i64,
    pub chunk_overlap: i64,
    pub timeout_sec: u64,
    pub natasha: bool,
    /// Пропуск гигантов с warning (§2.6).
    pub max_file_mb: i64,
    /// `anonymize.gpu: auto|off` — в P2 всегда CPU (lease — P3).
    pub gpu: String,
    pub lease_timeout_sec: u64,
    /// RPC на файл (`WorkerConfig.request_timeout`).
    pub request_timeout_sec: u64,
    /// Простой воркера до выгрузки (§2.2: CPU — 60 с).
    pub idle_timeout_sec: u64,
    /// Заметки о некорректных значениях конфига (для `hds check`).
    pub notices: Vec<String>,
}

impl Settings {
    pub fn from_config(cfg: &Config) -> Self {
        let threshold = number(cfg, "anonymize.threshold", 0.3);
        let timeout_sec = integer(cfg, "anonymize.timeout_sec", 300).max(30) as u64;
        let max_input_chars = integer(cfg, "anonymize.max_input_chars", 1500);
        let chunk_overlap = integer(cfg, "anonymize.chunk_overlap", 200);
        let mut notices = Vec::new();
        if !(0.0..=1.0).contains(&threshold) {
            notices.push(format!(
                "anonymize.threshold={threshold} вне [0..1] — используется 0.3"
            ));
        }
        if max_input_chars < 200 {
            notices.push(format!(
                "anonymize.max_input_chars={max_input_chars} < 200 — используется 1500"
            ));
        }
        Settings {
            output_dir: text(cfg, "anonymize.output_dir", "data/anonymized"),
            db_path: text(cfg, "anonymize.db_path", "data/anonymize.db"),
            model: text(cfg, "anonymize.model", "knowledgator/gliner-pii-large-v1.0"),
            threshold: if (0.0..=1.0).contains(&threshold) {
                threshold
            } else {
                0.3
            },
            max_input_chars: if max_input_chars < 200 {
                1500
            } else {
                max_input_chars
            },
            chunk_overlap: chunk_overlap.max(0),
            timeout_sec,
            natasha: flag(cfg, "anonymize.natasha", true),
            max_file_mb: integer(cfg, "anonymize.max_file_mb", 200),
            gpu: text(cfg, "anonymize.gpu", "auto"),
            lease_timeout_sec: integer(cfg, "anonymize.lease_timeout_sec", 120).max(1) as u64,
            request_timeout_sec: integer(cfg, "anonymize.request_timeout_sec", 600).max(60) as u64,
            idle_timeout_sec: integer(cfg, "anonymize.idle_timeout_sec", 60).max(5) as u64,
            notices,
        }
    }

    /// Абсолютный путь БД токенов (относительный — от корня проекта).
    pub fn db_abs(&self, root: &Path) -> PathBuf {
        let p = PathBuf::from(&self.db_path);
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    }

    /// Абсолютный выходной каталог (относительный — от корня проекта).
    pub fn output_dir_abs(&self, root: &Path) -> PathBuf {
        let p = PathBuf::from(&self.output_dir);
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    }

    /// Аргументы воркера (в `WorkerConfig.extra_args`, §3: параметры —
    /// только из аргументов).
    pub fn worker_args(&self, root: &Path) -> Vec<String> {
        let mut args = vec![
            "--model".to_string(),
            self.model.clone(),
            "--threshold".to_string(),
            format!("{}", self.threshold),
            "--max-input-chars".to_string(),
            self.max_input_chars.to_string(),
            "--chunk-overlap".to_string(),
            self.chunk_overlap.to_string(),
            "--timeout".to_string(),
            self.timeout_sec.to_string(),
            "--natasha".to_string(),
            if self.natasha { "1" } else { "0" }.to_string(),
            "--models-dir".to_string(),
            models_dir(root).to_string_lossy().into_owned(),
            "--db".to_string(),
            self.db_abs(root).to_string_lossy().into_owned(),
            "--device".to_string(),
            // P2: всегда CPU; P3 (lease) включит cuda
            "cpu".to_string(),
        ];
        if self.gpu == "off" {
            args.push("--gpu-mem-limit-mb".into());
            args.push("0".into());
        }
        args
    }
}
