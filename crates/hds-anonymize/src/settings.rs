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
    /// `anonymize.gpu: auto|off` — в P3 `auto` берёт lease у `llm_host`
    /// (fallback CPU при недоступности/отказе), `off` — всегда CPU.
    pub gpu: String,
    /// `anonymize.llm_host_url` — куда ходить за lease (порт роли `chat`).
    pub llm_host_url: String,
    /// `anonymize.gpu_reserve_mb` — NVML-резерв GLiNER (~2 ГБ: модель ~1 ГБ + активации).
    pub gpu_reserve_mb: u64,
    pub lease_timeout_sec: u64,
    /// Опциональные категории скрытия (по умолчанию ВСЕ открыты):
    /// `anonymize.hide_dates | hide_numbers | hide_software | hide_departments`.
    pub hide_dates: bool,
    pub hide_numbers: bool,
    pub hide_software: bool,
    pub hide_departments: bool,
    /// `anonymize.keep_terms_file` — файл списка «не анонимизировать» (D19);
    /// стартовый список подмешивается воркером всегда (vault.py).
    pub keep_terms_file: Option<String>,
    /// `anonymize.agent.images` — `strip` (по умолчанию) или `keep`: что
    /// делать с картинками тела в режиме `mode: "agent"` (TR-7.4).
    pub agent_images: String,
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
            llm_host_url: text(
                cfg,
                "anonymize.llm_host_url",
                crate::lease::DEFAULT_LLM_HOST_URL,
            ),
            gpu_reserve_mb: integer(cfg, "anonymize.gpu_reserve_mb", 2048).max(256) as u64,
            lease_timeout_sec: integer(cfg, "anonymize.lease_timeout_sec", 120).max(1) as u64,
            hide_dates: flag(cfg, "anonymize.hide_dates", false),
            hide_numbers: flag(cfg, "anonymize.hide_numbers", false),
            hide_software: flag(cfg, "anonymize.hide_software", false),
            hide_departments: flag(cfg, "anonymize.hide_departments", false),
            keep_terms_file: dig(cfg, "anonymize.keep_terms_file")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            agent_images: {
                let value = text(cfg, "anonymize.agent.images", "strip").to_lowercase();
                if value == "strip" || value == "keep" {
                    value
                } else {
                    notices.push(format!(
                        "anonymize.agent.images={value} — используется strip"
                    ));
                    "strip".to_string()
                }
            },
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
    /// только из аргументов). CPU-вариант (P2-совместимость).
    pub fn worker_args(&self, root: &Path) -> Vec<String> {
        self.worker_args_for(root, "cpu", 0)
    }

    /// Аргументы воркера с явным устройством и лимитом VRAM ORT
    /// (`--device cuda --gpu-mem-limit-mb N` — P3, lease у `llm_host`).
    /// `gpu_mem_limit_mb = 0` — лимит не выставляется.
    pub fn worker_args_for(&self, root: &Path, device: &str, gpu_mem_limit_mb: u64) -> Vec<String> {
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
            device.to_string(),
        ];
        // D19: файл списка «не анонимизировать» (стартовый список — в воркере)
        if let Some(path) = &self.keep_terms_file {
            let abs = {
                let p = PathBuf::from(path);
                if p.is_absolute() {
                    p
                } else {
                    root.join(p)
                }
            };
            args.push("--keep-terms".into());
            args.push(abs.to_string_lossy().into_owned());
        }
        if gpu_mem_limit_mb > 0 {
            args.push("--gpu-mem-limit-mb".into());
            args.push(gpu_mem_limit_mb.to_string());
        }
        // Опциональные категории скрытия: флаг уходит воркеру ТОЛЬКО когда
        // включён (по умолчанию категории открыты — аргументов нет)
        for (name, enabled) in [
            ("--hide-dates", self.hide_dates),
            ("--hide-numbers", self.hide_numbers),
            ("--hide-software", self.hide_software),
            ("--hide-departments", self.hide_departments),
        ] {
            if enabled {
                args.push(name.into());
                args.push("1".into());
            }
        }
        // TR-7.4: что делать с картинками тела в режиме agent
        args.push("--agent-images".into());
        args.push(self.agent_images.clone());
        args
    }
}
