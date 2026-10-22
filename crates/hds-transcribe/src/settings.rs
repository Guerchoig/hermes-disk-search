//! Чтение `transcribe.*` (с маппингом старых ключей) → параметры воркера.
//!
//! Rust — единственный источник истины: воркеру уходят абсолютные пути и
//! готовые значения, `config.yaml` он не читает (§5.3). Старые ключи
//! `auto_transcribe.diarization*`, `index.whisper_*`, `diarization.*`
//! читаются и маппятся; заметки о маппинге — в [`Settings::notices`].

use std::path::{Path, PathBuf};

use hds_core::config::{dig, project_root, Config};

use crate::params::{
    AlignParams, AlignQuality, AsrParams, DecodeParams, DiarizationParams, GlossaryParams,
    HotwordsParams, LlmParams, TranscribeParams,
};

/// Ворота P3 (G1/G2): `true` — только если в `P3_REPORT.md` зафиксированы
/// пройденные ворота. Пока `false`, любой `transcribe.hotwords.backend` кроме
/// `none` **принудительно** выключается с честным notice (biasing — не
/// настройка официального GigaAM, а другой ASR-рантайм, §4.2).
pub const HOTWORDS_GATES_PASSED: bool = false;

fn text(cfg: &Config, key: &str, default: &str) -> String {
    dig(cfg, key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn flag(cfg: &Config, key: &str, default: bool) -> bool {
    dig(cfg, key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn number(cfg: &Config, key: &str, default: f64) -> f64 {
    dig(cfg, key).and_then(|v| v.as_f64()).unwrap_or(default)
}

fn integer(cfg: &Config, key: &str, default: i64) -> i64 {
    dig(cfg, key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn env_path(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

/// Настройки транскрибации, собранные из конфига.
#[derive(Debug, Clone)]
pub struct Settings {
    /// `transcribe.engine`: `python` | `legacy` (по умолчанию `legacy`).
    pub engine: String,
    /// `transcribe.profile`: `cuda|cpu|dml` (`auto` → `cuda`).
    pub profile: String,
    pub request_timeout_sec: u64,
    pub idle_unload_sec: u64,
    pub max_requests: u64,
    pub fallback_cpu: bool,
    pub max_duration_hours: f64,
    /// Каталог служебных артефактов (`transcript.json`, `diarization.json`).
    pub state_dir: PathBuf,
    pub language: String,
    pub asr: AsrParams,
    pub align: AlignParams,
    pub decode: DecodeParams,
    pub diarization: DiarizationParams,
    pub cache_diarization: bool,
    /// `transcribe.glossary.*` (P3): путь к `glossary.yaml` и явные термины.
    pub glossary: GlossaryParams,
    /// `transcribe.hotwords.*` (P3.6): лимиты 64×64; biasing — только после ворот.
    pub hotwords: HotwordsParams,
    pub llm: LlmParams,
    /// Заметки о маппинге старых ключей (в журнал задания).
    pub notices: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings::from_config(&Config::Null)
    }
}

/// `transcribe.glossary.*` → [`GlossaryParams`]; относительный путь — от корня проекта.
fn glossary_from_config(cfg: &Config, root: &Path) -> GlossaryParams {
    let raw = text(cfg, "transcribe.glossary.path", "");
    let path = if raw.is_empty() {
        String::new()
    } else {
        let candidate = PathBuf::from(&raw);
        let absolute = if candidate.is_absolute() {
            candidate
        } else {
            root.join(candidate)
        };
        absolute.to_string_lossy().to_string()
    };
    let terms = dig(cfg, "transcribe.glossary.terms")
        .and_then(|v| v.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::to_value(item).ok())
                .collect()
        })
        .unwrap_or_default();
    GlossaryParams { path, terms }
}

/// `transcribe.hotwords.*` → [`HotwordsParams`] (значения читаются как есть;
/// лимиты 64×64 — жёсткие, проверяются и здесь, и в Python до вызова gigastt).
fn hotwords_from_config(cfg: &Config) -> HotwordsParams {
    let backend = text(cfg, "transcribe.hotwords.backend", "none").to_lowercase();
    let phrases = dig(cfg, "transcribe.hotwords.phrases")
        .and_then(|v| v.as_sequence())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    HotwordsParams {
        enabled: flag(cfg, "transcribe.hotwords.enabled", false),
        backend: if backend.is_empty() {
            "none".to_string()
        } else {
            backend
        },
        phrases,
        max_phrases: integer(cfg, "transcribe.hotwords.max_phrases", 64).clamp(1, 64) as u32,
        max_phrase_chars: integer(cfg, "transcribe.hotwords.max_phrase_chars", 64).clamp(1, 64)
            as u32,
        boost: number(cfg, "transcribe.hotwords.boost", 5.0),
        url: text(cfg, "transcribe.hotwords.url", "http://127.0.0.1:9876"),
    }
}

impl Settings {
    /// Собрать настройки из конфига (недостающее — дефолты P0).
    pub fn from_config(cfg: &Config) -> Self {
        let mut notices = Vec::new();
        let root = project_root();
        let profile_raw = text(
            cfg,
            "transcribe.profile",
            &env_path("HDS_TRANSCRIBE_PROFILE"),
        );
        let profile = match profile_raw.trim().to_lowercase().as_str() {
            "cpu" => "cpu".to_string(),
            "dml" | "directml" => "dml".to_string(),
            "cuda" | "gpu" => "cuda".to_string(),
            _ => "cuda".to_string(),
        };

        let state_rel = text(
            cfg,
            "transcribe.state_dir",
            &text(cfg, "auto_transcribe.state_dir", "data/auto-transcribe"),
        );
        let state_dir = {
            let p = PathBuf::from(&state_rel);
            if p.is_absolute() {
                p
            } else {
                root.join(p)
            }
        };

        let mut asr = AsrParams {
            model: text(cfg, "transcribe.asr.model", "ai-sage/GigaAM-v3"),
            variant: text(cfg, "transcribe.asr.variant", "e2e_rnnt"),
            revision: dig(cfg, "transcribe.asr.revision")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            batch_size: integer(cfg, "transcribe.asr.batch_size", 8).clamp(1, 64) as u32,
            max_chunk_sec: number(cfg, "transcribe.asr.max_chunk_sec", 25.0),
            cache_dir: text(
                cfg,
                "transcribe.asr.cache_dir",
                &env_path("HDS_GIGAAM_CACHE"),
            ),
            onnx_dir: text(cfg, "transcribe.asr.onnx_dir", &env_path("HDS_GIGAAM_ONNX")),
            ..AsrParams::default()
        };

        let backend = text(cfg, "transcribe.asr.backend", "auto").to_lowercase();
        asr.backend = match backend.as_str() {
            "torch" | "onnx" => backend,
            _ => {
                if profile == "dml" {
                    "onnx".to_string()
                } else {
                    "torch".to_string()
                }
            }
        };
        asr.device = match text(cfg, "transcribe.asr.device", "auto")
            .to_lowercase()
            .as_str()
        {
            "cuda" | "gpu" => "cuda".to_string(),
            "cpu" => "cpu".to_string(),
            _ => {
                if profile == "cpu" || (profile == "dml" && asr.backend == "torch") {
                    "cpu".to_string()
                } else {
                    "auto".to_string()
                }
            }
        };
        asr.provider = text(cfg, "transcribe.asr.provider", "auto").to_lowercase();
        asr.fp16 = flag(cfg, "transcribe.asr.fp16", profile == "cuda") && asr.device != "cpu";
        asr.vad = text(cfg, "transcribe.asr.vad", "silero").to_lowercase();
        asr.longform = "off".to_string();
        if let Some(v) = dig(cfg, "transcribe.asr.longform").and_then(|v| v.as_str()) {
            if v.trim() != "off" {
                notices.push(format!(
                    "transcribe.asr.longform={v} не поддерживается в P1 — используется Silero VAD"
                ));
            }
        }
        let glossary = glossary_from_config(cfg, &root);
        let mut hotwords = hotwords_from_config(cfg);
        if hotwords.backend != "none" {
            if !HOTWORDS_GATES_PASSED {
                notices.push(format!(
                    "transcribe.hotwords.backend={} не включён: ворота G1/G2 (P3) не пройдены — \
                     biasing выключен, сущности правит глоссарий",
                    hotwords.backend
                ));
                hotwords.backend = "none".to_string();
                hotwords.enabled = false;
            } else if hotwords.backend != "gigastt" {
                notices.push(format!(
                    "transcribe.hotwords.backend={} неизвестен — используется none",
                    hotwords.backend
                ));
                hotwords.backend = "none".to_string();
                hotwords.enabled = false;
            }
        }
        if hotwords.enabled && hotwords.phrases.len() > hotwords.max_phrases as usize {
            notices.push(format!(
                "transcribe.hotwords.phrases: {} фраз > лимита {} (gigastt: too_many_hotwords)",
                hotwords.phrases.len(),
                hotwords.max_phrases
            ));
        }
        let too_long = hotwords
            .phrases
            .iter()
            .filter(|p| p.chars().count() > hotwords.max_phrase_chars as usize)
            .count();
        if too_long > 0 {
            notices.push(format!(
                "transcribe.hotwords.phrases: {too_long} фраз длиннее {} символов \
                 (gigastt: hotword_phrase_too_long)",
                hotwords.max_phrase_chars
            ));
        }
        if flag(cfg, "transcribe.llm.enabled", false) {
            notices.push("transcribe.llm.enabled=true игнорируется в P1 (LLM — P4)".to_string());
        }

        // P2: поддерживаются native | whisperx | hybrid; неизвестное значение —
        // не молча, а notice + native (решение владельца §6 п. 1–2).
        let align_raw = text(cfg, "transcribe.align.mode", "native").to_lowercase();
        let align_mode = match align_raw.as_str() {
            "whisperx" => "whisperx".to_string(),
            "hybrid" => "hybrid".to_string(),
            "native" => "native".to_string(),
            other => {
                notices.push(format!(
                    "align.mode={other} неизвестен — используется native"
                ));
                "native".to_string()
            }
        };
        let align_device = match text(cfg, "transcribe.align.device", "auto")
            .to_lowercase()
            .as_str()
        {
            "cuda" | "gpu" => "cuda".to_string(),
            "cpu" => "cpu".to_string(),
            _ => {
                if profile == "cuda" {
                    "auto".to_string()
                } else {
                    "cpu".to_string()
                }
            }
        };
        let align = AlignParams {
            mode: align_mode,
            model: text(
                cfg,
                "transcribe.align.model",
                "jonatasgrosman/wav2vec2-large-xlsr-53-russian",
            ),
            device: align_device,
            max_shift_sec: number(cfg, "transcribe.align.max_shift_sec", 0.35),
            quality: AlignQuality {
                max_gap_sec: number(cfg, "transcribe.align.quality.max_gap_sec", 1.5),
                min_duration_sec: number(cfg, "transcribe.align.quality.min_duration_sec", 0.02),
                overlap_tolerance_sec: number(
                    cfg,
                    "transcribe.align.quality.overlap_tolerance_sec",
                    0.12,
                ),
                min_aligned_ratio: number(cfg, "transcribe.align.quality.min_aligned_ratio", 0.80),
                max_unreliable_ratio: number(
                    cfg,
                    "transcribe.align.quality.max_unreliable_ratio",
                    0.02,
                ),
            },
        };

        let decode = DecodeParams {
            channels: text(cfg, "transcribe.decode.channels", "auto").to_lowercase(),
            ffmpeg: text(
                cfg,
                "transcribe.decode.ffmpeg",
                &text(cfg, "index.ffmpeg", &env_path("HDS_FFMPEG")),
            ),
            ffprobe: text(cfg, "transcribe.decode.ffprobe", &env_path("HDS_FFPROBE")),
            sample_rate: integer(cfg, "transcribe.decode.sample_rate", 16000) as u32,
        };

        // --- диаризация: новые ключи → старые (legacy) ---
        let enabled = dig(cfg, "transcribe.diarization.enabled")
            .and_then(|v| v.as_bool())
            .or_else(|| dig(cfg, "auto_transcribe.diarization").and_then(|v| v.as_bool()))
            .or_else(|| dig(cfg, "diarization.enabled").and_then(|v| v.as_bool()))
            .unwrap_or(true);
        let required = dig(cfg, "transcribe.diarization.required")
            .and_then(|v| v.as_bool())
            .or_else(|| dig(cfg, "auto_transcribe.diarization_required").and_then(|v| v.as_bool()))
            .unwrap_or(true);
        if dig(cfg, "auto_transcribe.diarization_backend").is_some() {
            notices.push(
                "auto_transcribe.diarization_backend (legacy nemotron3/sortformer) игнорируется \
                 в python-режиме: используется pyannote community-1"
                    .to_string(),
            );
        }
        let diar_device = match text(cfg, "transcribe.diarization.device", "auto")
            .to_lowercase()
            .as_str()
        {
            "cuda" | "gpu" => "cuda".to_string(),
            "cpu" => "cpu".to_string(),
            _ => {
                if profile == "cuda" {
                    "auto".to_string()
                } else {
                    "cpu".to_string()
                }
            }
        };
        let diarization = DiarizationParams {
            enabled,
            required,
            model: text(
                cfg,
                "transcribe.diarization.model",
                "pyannote/speaker-diarization-community-1",
            ),
            device: diar_device,
            min_speakers: integer(cfg, "transcribe.diarization.min_speakers", 1).max(1) as u32,
            max_speakers: integer(cfg, "transcribe.diarization.max_speakers", 20).max(1) as u32,
            num_speakers: integer(cfg, "transcribe.diarization.num_speakers", 0).max(0) as u32,
            block_minutes: number(cfg, "transcribe.diarization.block_minutes", 20.0),
            cache: flag(cfg, "transcribe.cache.diarization", true),
            cache_dir: state_dir.to_string_lossy().to_string(),
            ..DiarizationParams::default()
        };

        let language = dig(cfg, "transcribe.asr.language")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .or_else(|| {
                dig(cfg, "index.whisper_language")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
            })
            .unwrap_or_else(|| "ru".to_string());

        Settings {
            engine: text(cfg, "transcribe.engine", "legacy").to_lowercase(),
            profile,
            request_timeout_sec: integer(cfg, "transcribe.request_timeout_sec", 25_200).max(60)
                as u64,
            idle_unload_sec: integer(cfg, "transcribe.idle_unload_sec", 600).max(0) as u64,
            max_requests: integer(cfg, "transcribe.max_requests", 20).max(0) as u64,
            fallback_cpu: flag(cfg, "transcribe.fallback_cpu", true),
            max_duration_hours: number(cfg, "transcribe.max_duration_hours", 6.0),
            state_dir,
            language,
            asr,
            align,
            decode,
            diarization,
            cache_diarization: flag(cfg, "transcribe.cache.diarization", true),
            glossary,
            hotwords,
            llm: LlmParams::default(),
            notices,
        }
    }

    /// Включён ли новый python-путь (`transcribe.engine: python`).
    pub fn is_python(&self) -> bool {
        self.engine == "python"
    }

    /// Параметры одного задания (`transcribe`).
    pub fn params(&self, media: &Path, job_id: &str, diarization: bool) -> TranscribeParams {
        let mut diar = self.diarization.clone();
        diar.enabled = diar.enabled && diarization;
        TranscribeParams {
            job_id: job_id.to_string(),
            path: media.to_string_lossy().to_string(),
            language: self.language.clone(),
            align: self.align.clone(),
            decode: self.decode.clone(),
            asr: self.asr.clone(),
            diarization: diar,
            state_dir: self.state_dir.to_string_lossy().to_string(),
            stop_file: self.stop_file().to_string_lossy().to_string(),
            max_duration_hours: self.max_duration_hours,
            fallback_cpu: self.fallback_cpu,
        }
    }

    /// Файл-сигнал остановки — тот же, что у демона и `hds transcribe-stop`
    /// (`<корень проекта>/transcribe.stop`, см. `autotranscribe::run_transcribe_watch`).
    pub fn stop_file(&self) -> PathBuf {
        project_root().join("transcribe.stop")
    }

    /// Интерпретатор воркера: env → `sidecar/venv-transcribe[-dml]` → `sidecar/python`.
    pub fn python(&self, root: &Path) -> Option<PathBuf> {
        let venv = if self.profile == "dml" {
            "venv-transcribe-dml"
        } else {
            "venv-transcribe"
        };
        hds_extract::discover_python_with(root, "HDS_TRANSCRIBE_PYTHON", Some(venv))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hds_core::config::Config;
    use std::path::Path;

    /// Тестовый конфиг: `Config` — это `serde_yaml::Value` (как `config.yaml`).
    fn cfg(yaml: &str) -> Config {
        serde_yaml::from_str(yaml).expect("тестовый конфиг")
    }

    #[test]
    fn align_defaults_match_python() {
        let s = Settings::from_config(&Config::Null);
        assert_eq!(s.align.mode, "native");
        assert_eq!(
            s.align.model,
            "jonatasgrosman/wav2vec2-large-xlsr-53-russian"
        );
        assert_eq!(s.align.max_shift_sec, 0.35);
        assert_eq!(s.align.quality.max_gap_sec, 1.5);
        assert_eq!(s.align.quality.min_duration_sec, 0.02);
        assert_eq!(s.align.quality.overlap_tolerance_sec, 0.12);
        assert_eq!(s.align.quality.min_aligned_ratio, 0.80);
        assert_eq!(s.align.quality.max_unreliable_ratio, 0.02);
        assert!(s.notices.is_empty(), "дефолты не должны давать notice");
    }

    #[test]
    fn align_quality_read_from_config() {
        let s = Settings::from_config(&cfg(r#"
transcribe:
  profile: cuda
  align:
    mode: HYBRID
    model: custom/aligner
    device: CPU
    max_shift_sec: 0.5
    quality:
      max_gap_sec: 2.0
      min_duration_sec: 0.05
      overlap_tolerance_sec: 0.12
      min_aligned_ratio: 0.9
      max_unreliable_ratio: 0.01
"#));
        assert_eq!(s.align.mode, "hybrid");
        assert_eq!(s.align.model, "custom/aligner");
        assert_eq!(s.align.device, "cpu");
        assert_eq!(s.align.max_shift_sec, 0.5);
        assert_eq!(s.align.quality.max_gap_sec, 2.0);
        assert_eq!(s.align.quality.min_duration_sec, 0.05);
        assert_eq!(s.align.quality.overlap_tolerance_sec, 0.12);
        assert_eq!(s.align.quality.min_aligned_ratio, 0.9);
        assert_eq!(s.align.quality.max_unreliable_ratio, 0.01);
    }

    #[test]
    fn unknown_align_mode_is_notice_and_native() {
        let s = Settings::from_config(&cfg("transcribe:\n  align:\n    mode: magic\n"));
        assert_eq!(s.align.mode, "native");
        assert!(s.notices.iter().any(|n| n.contains("align.mode=magic")));
    }

    #[test]
    fn align_device_defaults_to_cpu_outside_cuda_profile() {
        let s = Settings::from_config(&cfg("transcribe:\n  profile: dml\n"));
        assert_eq!(s.profile, "dml");
        assert_eq!(s.align.device, "cpu");
    }

    #[test]
    fn glossary_defaults_are_empty_and_silent() {
        let s = Settings::from_config(&Config::Null);
        assert!(s.glossary.path.is_empty());
        assert!(s.glossary.terms.is_empty());
        assert!(s.notices.is_empty());
    }

    #[test]
    fn glossary_path_is_resolved_from_project_root() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  glossary:\n    path: data/glossary.yaml\n",
        ));
        let path = std::path::Path::new(&s.glossary.path);
        assert!(
            path.is_absolute(),
            "ожидался абсолютный путь: {}",
            s.glossary.path
        );
        assert!(
            s.glossary.path.ends_with("data/glossary.yaml")
                || s.glossary.path.ends_with("data\\glossary.yaml")
        );
    }

    #[test]
    fn glossary_terms_are_passed_as_json() {
        let s = Settings::from_config(&cfg(r#"
transcribe:
  glossary:
    terms:
      - canonical: "1С"
        variants: ["индес"]
        priority: high
      - "МЧД"
"#));
        assert_eq!(s.glossary.terms.len(), 2);
        assert_eq!(s.glossary.terms[0]["canonical"], "1С");
        assert_eq!(s.glossary.terms[1], serde_json::json!("МЧД"));
    }

    #[test]
    fn hotwords_defaults_are_off_with_gigastt_limits() {
        let s = Settings::from_config(&Config::Null);
        assert!(!s.hotwords.enabled);
        assert_eq!(s.hotwords.backend, "none");
        assert_eq!(s.hotwords.max_phrases, 64);
        assert_eq!(s.hotwords.max_phrase_chars, 64);
        assert_eq!(s.hotwords.boost, 5.0);
        assert!(s.hotwords.phrases.is_empty());
    }

    #[test]
    fn hotwords_backend_is_forced_to_none_before_gates() {
        // Ворота P3 не пройдены (константа), поэтому backend принудительно none,
        // а notice честно говорит причину (§4.2, гейт P3).
        let s = Settings::from_config(&cfg(
            "transcribe:\n  hotwords:\n    enabled: true\n    backend: gigastt\n    phrases: [\"1С\"]\n",
        ));
        assert_eq!(s.hotwords.backend, "none");
        assert!(!s.hotwords.enabled);
        assert!(s
            .notices
            .iter()
            .any(|n| n.contains("hotwords.backend=gigastt")));
    }

    #[test]
    fn hotwords_limits_are_clamped_to_64() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  hotwords:\n    max_phrases: 200\n    max_phrase_chars: 200\n",
        ));
        assert_eq!(s.hotwords.max_phrases, 64);
        assert_eq!(s.hotwords.max_phrase_chars, 64);
    }

    #[test]
    fn params_carry_postprocess_settings_to_worker() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  glossary:\n    path: data/glossary.yaml\n  hotwords:\n    boost: 8\n",
        ));
        // Собираем так же, как `hds-index::transcribe` перед вызовом postprocess.
        let params = crate::params::PostprocessParams {
            job_id: "job".to_string(),
            transcript_path: "t.transcript.json".to_string(),
            glossary: s.glossary.clone(),
            hotwords: s.hotwords.clone(),
            llm: s.llm.clone(),
        };
        assert!(params.glossary.path.ends_with("glossary.yaml"));
        assert_eq!(params.hotwords.boost, 8.0);
        assert_eq!(params.hotwords.backend, "none");
        assert_eq!(params.hotwords.max_phrases, 64);
    }

    #[test]
    fn params_carry_align_quality_to_worker() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  profile: cuda\n  align:\n    quality:\n      max_unreliable_ratio: 0.03\n",
        ));
        let params = s.params(Path::new("D:/media/a.mp3"), "job", true);
        assert_eq!(params.align.quality.max_unreliable_ratio, 0.03);
        assert_eq!(params.align.mode, "native");
    }
}
