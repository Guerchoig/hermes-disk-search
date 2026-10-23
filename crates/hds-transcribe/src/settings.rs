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

/// Нормализовать явный профиль (`cuda|cpu|dml`); `None` — `auto`/неизвестно.
fn normalize_profile(value: &str) -> Option<String> {
    match value.trim().to_lowercase().as_str() {
        "cuda" | "gpu" => Some("cuda".to_string()),
        "cpu" => Some("cpu".to_string()),
        "dml" | "directml" => Some("dml".to_string()),
        _ => None,
    }
}

/// Разрешить `transcribe.profile`, включая `auto` (§4.7).
///
/// `auto` выбирает профиль по **фактически собранным** venv: если есть только
/// `venv-transcribe-dml` — это AMD/Intel-машина; если оба — CUDA (основной).
/// Без venv остаётся `cuda` (P5-установщик всё равно кладёт ровно один профиль).
/// `HDS_TRANSCRIBE_PROFILE` имеет приоритет над конфигом и резолвится здесь же.
pub fn resolve_profile(raw: &str, root: &Path) -> String {
    if let Some(profile) = normalize_profile(raw) {
        return profile;
    }
    let has = |name: &str| root.join("sidecar").join(name).join("pyvenv.cfg").exists();
    match (has("venv-transcribe"), has("venv-transcribe-dml")) {
        (true, _) => "cuda".to_string(),
        (false, true) => "dml".to_string(),
        _ => "cuda".to_string(),
    }
}

/// Настройки транскрибации, собранные из конфига.
#[derive(Debug, Clone)]
pub struct Settings {
    /// `transcribe.engine`: `python` | `legacy` (по умолчанию **python**, P6).
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
    /// `transcribe.models_dir` (`HDS_MODELS_DIR`): база для HF-кэша и ONNX.
    pub models_dir: PathBuf,
    /// `transcribe.hf_token_env`: имя переменной с HF-токеном.
    pub hf_token_env: String,
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

/// Потолок правок LLM на 1000 слов (`transcribe.llm.max_edits_per_1000`).
///
/// Это предохранитель от «переписывания» транскрипта: при 10 правках на 1000
/// слов 18-минутная встреча получает единицы правок, а не десятки.
pub const LLM_MAX_EDITS_PER_1000_LIMIT: f64 = 100.0;
/// Границы таймаута одного HTTP-запроса к `llm_host` (`transcribe.llm.timeout_sec`).
pub const LLM_TIMEOUT_MIN_SEC: f64 = 5.0;
pub const LLM_TIMEOUT_MAX_SEC: f64 = 3600.0;
/// Границы `max_tokens` ответа LLM (патчи — компактный JSON).
pub const LLM_MAX_TOKENS_MIN: i64 = 64;
pub const LLM_MAX_TOKENS_MAX: i64 = 32768;

/// `transcribe.llm.*` → [`LlmParams`] (P4.0).
///
/// Rust — единственный источник истины: воркер получает готовые значения и
/// `config.yaml` не читает (§5.3). Инференс живёт в `llm_host`
/// (`/v1/chat/completions`), Python лишь предлагает/валидирует патчи (§4.6).
/// Некорректные значения не роняют конфиг: они приводятся к безопасным с
/// честным notice — иначе «LLM включена», а фактически молча выключена.
fn llm_from_config(cfg: &Config, notices: &mut Vec<String>) -> LlmParams {
    let enabled = flag(cfg, "transcribe.llm.enabled", false);
    let mut base_url = text(cfg, "transcribe.llm.base_url", "http://127.0.0.1:8010/v1");
    while base_url.ends_with('/') {
        base_url.pop();
    }
    let model = text(cfg, "transcribe.llm.model", "chat");

    let raw_temperature = number(cfg, "transcribe.llm.temperature", 0.0);
    // Постобработка детерминирована: temperature всегда 0 (§4.6 п. 6).
    if (raw_temperature - 0.0).abs() > f64::EPSILON {
        notices.push(format!(
            "transcribe.llm.temperature={raw_temperature} не поддерживается: \
             постобработка детерминирована — используется 0"
        ));
    }

    let raw_max_tokens = integer(cfg, "transcribe.llm.max_tokens", 2048);
    let max_tokens = raw_max_tokens.clamp(LLM_MAX_TOKENS_MIN, LLM_MAX_TOKENS_MAX);
    if max_tokens != raw_max_tokens {
        notices.push(format!(
            "transcribe.llm.max_tokens={raw_max_tokens} вне диапазона \
             {LLM_MAX_TOKENS_MIN}..{LLM_MAX_TOKENS_MAX} — используется {max_tokens}"
        ));
    }

    let raw_timeout = number(cfg, "transcribe.llm.timeout_sec", 300.0);
    let timeout_sec = raw_timeout.clamp(LLM_TIMEOUT_MIN_SEC, LLM_TIMEOUT_MAX_SEC);
    if (timeout_sec - raw_timeout).abs() > f64::EPSILON {
        notices.push(format!(
            "transcribe.llm.timeout_sec={raw_timeout} вне диапазона \
             {LLM_TIMEOUT_MIN_SEC}..{LLM_TIMEOUT_MAX_SEC} — используется {timeout_sec}"
        ));
    }

    let raw_edits = number(cfg, "transcribe.llm.max_edits_per_1000", 10.0);
    let max_edits_per_1000 = raw_edits.clamp(0.0, LLM_MAX_EDITS_PER_1000_LIMIT);
    if (max_edits_per_1000 - raw_edits).abs() > f64::EPSILON {
        notices.push(format!(
            "transcribe.llm.max_edits_per_1000={raw_edits} вне диапазона \
             0..{LLM_MAX_EDITS_PER_1000_LIMIT} — используется {max_edits_per_1000}"
        ));
    }

    let valid_url = base_url.starts_with("http://") || base_url.starts_with("https://");
    let enabled = if enabled && !valid_url {
        notices.push(format!(
            "transcribe.llm.base_url={base_url:?} не http(s)-URL — LLM-патчи выключены \
             (деградация до глоссария)"
        ));
        false
    } else {
        enabled
    };

    if enabled {
        notices.push(format!(
            "transcribe.llm: включён — host={base_url}, model={model}, temperature=0, \
             ≤{max_edits_per_1000} правок/1000 слов, max_tokens={max_tokens}, \
             timeout={timeout_sec} с"
        ));
    } else if dig(cfg, "transcribe.llm").is_some() {
        notices.push(
            "transcribe.llm.enabled=false — LLM-патчи выключены (работает только глоссарий)"
                .to_string(),
        );
    }

    LlmParams {
        enabled,
        base_url,
        model,
        temperature: 0.0,
        max_tokens: max_tokens as u32,
        timeout_sec,
        max_edits_per_1000,
    }
}

impl Settings {
    /// Собрать настройки из конфига (недостающее — дефолты P0).
    pub fn from_config(cfg: &Config) -> Self {
        let mut notices = Vec::new();
        let root = project_root();
        let profile_raw = {
            let env = env_path("HDS_TRANSCRIBE_PROFILE");
            if env.trim().is_empty() {
                text(cfg, "transcribe.profile", "auto")
            } else {
                env
            }
        };
        let profile = resolve_profile(&profile_raw, &root);

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
        asr.device_id = integer(cfg, "transcribe.asr.device_id", 0).max(0) as i32;
        asr.intra_threads = integer(cfg, "transcribe.asr.intra_threads", 0).max(0) as u32;
        asr.fp16 = flag(cfg, "transcribe.asr.fp16", profile == "cuda") && asr.device != "cpu";
        let vad = text(cfg, "transcribe.asr.vad", "silero").to_lowercase();
        asr.vad = match vad.as_str() {
            "silero" | "off" => vad,
            other => {
                notices.push(format!(
                    "transcribe.asr.vad={other} неизвестен (silero|off) — используется silero"
                ));
                "silero".to_string()
            }
        };
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
        let llm = llm_from_config(cfg, &mut notices);

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

        let requested_rate = integer(cfg, "transcribe.decode.sample_rate", 16000);
        if requested_rate != 16000 {
            notices.push(format!(
                "transcribe.decode.sample_rate={requested_rate} не поддерживается: \
                 ASR/VAD/диаризация работают на 16000 Гц — используется 16000"
            ));
        }
        let decode = DecodeParams {
            channels: text(cfg, "transcribe.decode.channels", "auto").to_lowercase(),
            ffmpeg: text(
                cfg,
                "transcribe.decode.ffmpeg",
                &text(cfg, "index.ffmpeg", &env_path("HDS_FFMPEG")),
            ),
            ffprobe: text(cfg, "transcribe.decode.ffprobe", &env_path("HDS_FFPROBE")),
            sample_rate: 16000,
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
            hf_token_env: text(cfg, "transcribe.hf_token_env", "HF_TOKEN"),
            ..DiarizationParams::default()
        };

        let models_raw = text(cfg, "transcribe.models_dir", &env_path("HDS_MODELS_DIR"));
        let models_dir = {
            let raw = PathBuf::from(&models_raw);
            if raw.as_os_str().is_empty() {
                PathBuf::new()
            } else if raw.is_absolute() {
                raw
            } else {
                root.join(raw)
            }
        };
        let hf_token_env = text(cfg, "transcribe.hf_token_env", "HF_TOKEN");

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
            // P6: python-путь — боевой по умолчанию; `legacy` — явный откат
            // старой цепочки (llm_host) на переходный релиз (§12).
            engine: text(cfg, "transcribe.engine", "python").to_lowercase(),
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
            models_dir,
            hf_token_env,
            glossary,
            hotwords,
            llm,
            notices,
        }
    }

    /// Включён ли новый python-путь (`transcribe.engine: python`).
    pub fn is_python(&self) -> bool {
        self.engine.eq_ignore_ascii_case("python")
    }

    /// Параметры одного задания (`transcribe`).
    ///
    /// `stop_file = Some(..)` — только демон `transcribe-watch` (долгоживущий
    /// процесс, отмена файлом). `transcribe-once` передаёт `None`, иначе
    /// оставшийся после `transcribe-stop` файл мгновенно отменял бы задание.
    pub fn params(
        &self,
        media: &Path,
        job_id: &str,
        diarization: bool,
        stop_file: Option<&Path>,
    ) -> TranscribeParams {
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
            stop_file: stop_file
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            max_duration_hours: self.max_duration_hours,
            fallback_cpu: self.fallback_cpu,
        }
    }

    /// Переменные окружения воркера: модели/HF-кэш из `transcribe.models_dir`.
    pub fn worker_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if self.models_dir.as_os_str().is_empty() {
            return env;
        }
        let models = self.models_dir.to_string_lossy().to_string();
        env.push(("HDS_MODELS_DIR".to_string(), models.clone()));
        if std::env::var("HF_HOME")
            .map(|v| v.trim().is_empty())
            .unwrap_or(true)
        {
            env.push((
                "HF_HOME".to_string(),
                self.models_dir
                    .join("hf-home")
                    .to_string_lossy()
                    .to_string(),
            ));
        }
        env
    }

    /// Файл-сигнал остановки — тот же, что у демона и `hds transcribe-stop`
    /// (`<корень проекта>/transcribe.stop`, см. `autotranscribe::run_transcribe_watch`).
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
    fn engine_defaults_to_python_and_legacy_is_explicit() {
        let s = Settings::from_config(&Config::Null);
        assert!(s.is_python(), "P6: python engine is the default");
        let s = Settings::from_config(&cfg("transcribe:\n  engine: legacy\n"));
        assert!(!s.is_python(), "legacy must stay an explicit rollback");
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
        // P4.0: LLM-параметры едут тем же постпроцессом.
        assert!(!params.llm.enabled);
    }

    #[test]
    fn llm_defaults_are_off_and_silent() {
        let s = Settings::from_config(&Config::Null);
        assert!(!s.llm.enabled);
        assert_eq!(s.llm.base_url, "http://127.0.0.1:8010/v1");
        assert_eq!(s.llm.model, "chat");
        assert_eq!(s.llm.temperature, 0.0);
        assert_eq!(s.llm.max_tokens, 2048);
        assert_eq!(s.llm.timeout_sec, 300.0);
        assert_eq!(s.llm.max_edits_per_1000, 10.0);
        // Отсутствие секции — это норма, а не повод шуметь в журнале.
        assert!(
            !s.notices.iter().any(|n| n.contains("transcribe.llm")),
            "notices: {:?}",
            s.notices
        );
    }

    #[test]
    fn llm_settings_are_read_and_reported() {
        let s = Settings::from_config(&cfg(r#"
transcribe:
  llm:
    enabled: true
    base_url: "http://127.0.0.1:8010/v1/"
    model: "qwen2.5-7b-instruct"
    temperature: 0.7
    max_tokens: 1024
    timeout_sec: 120
    max_edits_per_1000: 5
"#));
        assert!(s.llm.enabled);
        // Хвостовой слэш снимается: URL склеивается с `/chat/completions`.
        assert_eq!(s.llm.base_url, "http://127.0.0.1:8010/v1");
        assert_eq!(s.llm.model, "qwen2.5-7b-instruct");
        // Постобработка детерминирована: temperature принудительно 0.
        assert_eq!(s.llm.temperature, 0.0);
        assert_eq!(s.llm.max_tokens, 1024);
        assert_eq!(s.llm.timeout_sec, 120.0);
        assert_eq!(s.llm.max_edits_per_1000, 5.0);
        assert!(
            s.notices
                .iter()
                .any(|n| n.contains("transcribe.llm: включён")
                    && n.contains("qwen2.5-7b-instruct")
                    && n.contains("http://127.0.0.1:8010/v1")),
            "notices: {:?}",
            s.notices
        );
        assert!(s
            .notices
            .iter()
            .any(|n| n.contains("transcribe.llm.temperature=0.7")));
    }

    #[test]
    fn llm_disabled_section_is_honest_notice() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  llm:\n    enabled: false\n    model: chat\n",
        ));
        assert!(!s.llm.enabled);
        assert!(
            s.notices
                .iter()
                .any(|n| n.contains("transcribe.llm.enabled=false")),
            "notices: {:?}",
            s.notices
        );
    }

    #[test]
    fn llm_out_of_range_values_are_clamped_with_notices() {
        let s = Settings::from_config(&cfg(r#"
transcribe:
  llm:
    enabled: true
    max_tokens: 4
    timeout_sec: 1
    max_edits_per_1000: 500
"#));
        assert_eq!(s.llm.max_tokens, LLM_MAX_TOKENS_MIN as u32);
        assert_eq!(s.llm.timeout_sec, LLM_TIMEOUT_MIN_SEC);
        assert_eq!(s.llm.max_edits_per_1000, LLM_MAX_EDITS_PER_1000_LIMIT);
        for needle in [
            "transcribe.llm.max_tokens=4",
            "transcribe.llm.timeout_sec=1",
            "transcribe.llm.max_edits_per_1000=500",
        ] {
            assert!(
                s.notices.iter().any(|n| n.contains(needle)),
                "нет notice {needle}: {:?}",
                s.notices
            );
        }
    }

    #[test]
    fn llm_bad_base_url_disables_patches_with_notice() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  llm:\n    enabled: true\n    base_url: \"127.0.0.1:8010/v1\"\n",
        ));
        assert!(!s.llm.enabled, "некорректный URL не должен включать LLM");
        assert!(
            s.notices
                .iter()
                .any(|n| n.contains("transcribe.llm.base_url") && n.contains("не http(s)-URL")),
            "notices: {:?}",
            s.notices
        );
    }

    #[test]
    fn params_carry_llm_to_worker() {
        let s = Settings::from_config(&cfg(r#"
transcribe:
  llm:
    enabled: true
    base_url: "http://127.0.0.1:8010/v1"
    model: "qwen2.5-7b-instruct"
    max_tokens: 512
    timeout_sec: 90
    max_edits_per_1000: 3
"#));
        let params = crate::params::PostprocessParams {
            job_id: "job".to_string(),
            transcript_path: "t.transcript.json".to_string(),
            glossary: s.glossary.clone(),
            hotwords: s.hotwords.clone(),
            llm: s.llm.clone(),
        };
        assert!(params.llm.enabled);
        assert_eq!(params.llm.model, "qwen2.5-7b-instruct");
        assert_eq!(params.llm.max_tokens, 512);
        assert_eq!(params.llm.timeout_sec, 90.0);
        assert_eq!(params.llm.max_edits_per_1000, 3.0);
        // JSON, который реально уходит воркеру: поля аддитивны к P3.
        let json = serde_json::to_value(&params).expect("params -> json");
        assert_eq!(json["llm"]["enabled"], true);
        assert_eq!(json["llm"]["model"], "qwen2.5-7b-instruct");
        assert_eq!(json["llm"]["max_edits_per_1000"], 3.0);
    }

    #[test]
    fn params_carry_align_quality_to_worker() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  profile: cuda\n  align:\n    quality:\n      max_unreliable_ratio: 0.03\n",
        ));
        let params = s.params(Path::new("D:/media/a.mp3"), "job", true, None);
        assert_eq!(params.align.quality.max_unreliable_ratio, 0.03);
        assert_eq!(params.align.mode, "native");
        assert!(params.stop_file.is_empty());
    }

    #[test]
    fn stop_file_is_passed_only_when_requested() {
        let s = Settings::from_config(&Config::Null);
        let stop = Path::new("D:/root/transcribe.stop");
        let params = s.params(Path::new("D:/media/a.mp3"), "job", true, Some(stop));
        assert!(params.stop_file.ends_with("transcribe.stop"));
    }

    #[test]
    fn profile_auto_uses_only_dml_venv() {
        let root = std::env::temp_dir().join(format!("hds-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sidecar").join("venv-transcribe-dml")).unwrap();
        std::fs::write(
            root.join("sidecar")
                .join("venv-transcribe-dml")
                .join("pyvenv.cfg"),
            "",
        )
        .unwrap();
        assert_eq!(resolve_profile("auto", &root), "dml");
        assert_eq!(resolve_profile("cpu", &root), "cpu");
        assert_eq!(resolve_profile("bogus", &root), "dml");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_vad_and_sample_rate_are_notices() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  asr:\n    vad: webrtc\n  decode:\n    sample_rate: 44100\n",
        ));
        assert_eq!(s.asr.vad, "silero");
        assert_eq!(s.decode.sample_rate, 16000);
        assert!(s.notices.iter().any(|n| n.contains("asr.vad=webrtc")));
        assert!(s
            .notices
            .iter()
            .any(|n| n.contains("decode.sample_rate=44100")));
    }

    #[test]
    fn asr_device_and_threads_are_read() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  asr:\n    device_id: 2\n    intra_threads: 6\n",
        ));
        assert_eq!(s.asr.device_id, 2);
        assert_eq!(s.asr.intra_threads, 6);
    }

    #[test]
    fn models_dir_and_hf_token_env_reach_worker_env() {
        let s = Settings::from_config(&cfg(
            "transcribe:\n  models_dir: models\n  hf_token_env: HDS_HF_TOKEN\n",
        ));
        assert!(s.models_dir.ends_with("models"));
        assert_eq!(s.hf_token_env, "HDS_HF_TOKEN");
        assert_eq!(s.diarization.hf_token_env, "HDS_HF_TOKEN");
        let env = s.worker_env();
        assert!(env
            .iter()
            .any(|(k, v)| k == "HDS_MODELS_DIR" && v.ends_with("models")));
    }
}
