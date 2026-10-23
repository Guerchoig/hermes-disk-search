//! Типы запроса/ответа воркера транскрибации (§5.2).
//!
//! Имена полей — snake_case: их читает `sidecar/hds_transcribe/params.py`.
//! Дефолты совпадают с решениями P0, чтобы неполный конфиг не включал
//! `transcribe_longform`/hotwords/LLM.

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_true() -> bool {
    true
}
fn default_language() -> String {
    "ru".to_string()
}
fn default_variant() -> String {
    "e2e_rnnt".to_string()
}
fn default_asr_model() -> String {
    "ai-sage/GigaAM-v3".to_string()
}
fn default_backend() -> String {
    "torch".to_string()
}
fn default_device() -> String {
    "auto".to_string()
}
fn default_batch_size() -> u32 {
    8
}
fn default_chunk_sec() -> f64 {
    25.0
}
fn default_vad() -> String {
    "silero".to_string()
}
fn default_longform() -> String {
    "off".to_string()
}
fn default_channels() -> String {
    "auto".to_string()
}
fn default_sample_rate() -> u32 {
    16000
}
fn default_align_mode() -> String {
    "native".to_string()
}
fn default_align_model() -> &'static str {
    "jonatasgrosman/wav2vec2-large-xlsr-53-russian"
}
fn default_diar_model() -> String {
    "pyannote/speaker-diarization-community-1".to_string()
}
fn default_min_speakers() -> u32 {
    1
}
fn default_max_speakers() -> u32 {
    20
}
fn default_block_minutes() -> f64 {
    20.0
}
fn default_block_overlap() -> f64 {
    60.0
}
fn default_hf_token_env() -> String {
    "HF_TOKEN".to_string()
}
fn default_max_duration_hours() -> f64 {
    6.0
}

/// Пороги флагов слов (`transcribe.align.quality.*`).
///
/// Значения по умолчанию совпадают с `sidecar/hds_transcribe/params.py`
/// (`AlignQuality`), поэтому неполный конфиг не меняет разметку P1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AlignQuality {
    /// Разрыв между словами внутри фразы, после которого слово — `low_confidence`.
    pub max_gap_sec: f64,
    /// Минимальная длительность слова (короче — `low_confidence`).
    pub min_duration_sec: f64,
    /// Допуск конфликта таймкодов соседних слов (кадр GigaAM 40 мс; решение
    /// владельца — 3 кадра = 0,12 с).
    pub overlap_tolerance_sec: f64,
    /// Минимальная доля `aligned` для `alignment_quality.ok`.
    pub min_aligned_ratio: f64,
    /// Максимальная доля `low_confidence + failed` для `ok` (гейт P2: ≤ 2 %).
    pub max_unreliable_ratio: f64,
}

impl Default for AlignQuality {
    fn default() -> Self {
        AlignQuality {
            max_gap_sec: 1.5,
            min_duration_sec: 0.02,
            overlap_tolerance_sec: 0.12,
            min_aligned_ratio: 0.80,
            max_unreliable_ratio: 0.02,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AlignParams {
    /// `native` (default) | `whisperx` | `hybrid`.
    pub mode: String,
    /// WhisperX-модель выравнивания (`jonatasgrosman/wav2vec2-large-xlsr-53-russian`).
    pub model: String,
    /// `auto` | `cuda` | `cpu` — устройство aligner'а.
    pub device: String,
    /// Порог расхождения native/whisperx для `hybrid` (секунды).
    pub max_shift_sec: f64,
    pub quality: AlignQuality,
}

impl Default for AlignParams {
    fn default() -> Self {
        AlignParams {
            mode: default_align_mode(),
            model: default_align_model().to_string(),
            device: default_device(),
            max_shift_sec: 0.35,
            quality: AlignQuality::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DecodeParams {
    pub channels: String,
    pub ffmpeg: String,
    pub ffprobe: String,
    pub sample_rate: u32,
}

impl Default for DecodeParams {
    fn default() -> Self {
        DecodeParams {
            channels: default_channels(),
            ffmpeg: String::new(),
            ffprobe: String::new(),
            sample_rate: default_sample_rate(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AsrParams {
    pub model: String,
    pub variant: String,
    pub revision: String,
    pub backend: String,
    pub device: String,
    pub provider: String,
    pub device_id: i32,
    pub batch_size: u32,
    pub fp16: bool,
    pub vad: String,
    pub longform: String,
    pub max_chunk_sec: f64,
    pub cache_dir: String,
    pub onnx_dir: String,
    pub intra_threads: u32,
}

impl Default for AsrParams {
    fn default() -> Self {
        AsrParams {
            model: default_asr_model(),
            variant: default_variant(),
            revision: String::new(),
            backend: default_backend(),
            device: default_device(),
            provider: "auto".to_string(),
            device_id: 0,
            batch_size: default_batch_size(),
            fp16: true,
            vad: default_vad(),
            longform: default_longform(),
            max_chunk_sec: default_chunk_sec(),
            cache_dir: String::new(),
            onnx_dir: String::new(),
            intra_threads: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiarizationParams {
    pub enabled: bool,
    pub required: bool,
    pub model: String,
    pub device: String,
    pub min_speakers: u32,
    pub max_speakers: u32,
    pub num_speakers: u32,
    pub use_exclusive: bool,
    pub block_minutes: f64,
    pub block_overlap_sec: f64,
    pub cache: bool,
    pub cache_dir: String,
    pub hf_token_env: String,
    pub local_files_only: bool,
}

impl Default for DiarizationParams {
    fn default() -> Self {
        DiarizationParams {
            enabled: default_true(),
            required: default_true(),
            model: default_diar_model(),
            device: default_device(),
            min_speakers: default_min_speakers(),
            max_speakers: default_max_speakers(),
            num_speakers: 0,
            use_exclusive: true,
            block_minutes: default_block_minutes(),
            block_overlap_sec: default_block_overlap(),
            cache: default_true(),
            cache_dir: String::new(),
            hf_token_env: default_hf_token_env(),
            local_files_only: default_true(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscribeParams {
    pub job_id: String,
    pub path: String,
    pub language: String,
    pub align: AlignParams,
    pub decode: DecodeParams,
    pub asr: AsrParams,
    pub diarization: DiarizationParams,
    pub state_dir: String,
    pub stop_file: String,
    pub max_duration_hours: f64,
    pub fallback_cpu: bool,
}

impl Default for TranscribeParams {
    fn default() -> Self {
        TranscribeParams {
            job_id: String::new(),
            path: String::new(),
            language: default_language(),
            align: AlignParams::default(),
            decode: DecodeParams::default(),
            asr: AsrParams::default(),
            diarization: DiarizationParams::default(),
            state_dir: String::new(),
            stop_file: String::new(),
            max_duration_hours: default_max_duration_hours(),
            fallback_cpu: true,
        }
    }
}

fn default_hotwords_backend() -> String {
    // Подтверждённое решение P0/P3: до ворот G1/G2 biasing выключен.
    "none".to_string()
}
fn default_max_hotwords() -> u32 {
    64
}
fn default_hotword_chars() -> u32 {
    64
}
fn default_boost() -> f64 {
    5.0
}

/// Глоссарий (P3.1): `transcribe.glossary.*` → воркер `postprocess`.
///
/// `terms` — для тестов/переопределений: элементы могут быть строками
/// (`canonical`) или словарями формата §4.2 (`canonical`/`variants`/…).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GlossaryParams {
    pub path: String,
    pub terms: Vec<serde_json::Value>,
}

/// Hotwords (P3.6): лимиты gigastt 64×64, backend пока только `none`.
///
/// biasing — часть декодера **gigastt**, а не настройка официального GigaAM
/// (§4.2); включается только если G1/G2 пройдены (см. `settings::HOTWORDS_GATES_PASSED`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HotwordsParams {
    pub enabled: bool,
    /// `none` | `gigastt`.
    pub backend: String,
    pub phrases: Vec<String>,
    /// Жёсткий лимит gigastt: не больше 64 фраз.
    pub max_phrases: u32,
    /// Жёсткий лимит gigastt: не больше 64 символов в фразе.
    pub max_phrase_chars: u32,
    pub boost: f64,
    pub url: String,
}

impl Default for HotwordsParams {
    fn default() -> Self {
        HotwordsParams {
            enabled: false,
            backend: default_hotwords_backend(),
            phrases: Vec::new(),
            max_phrases: default_max_hotwords(),
            max_phrase_chars: default_hotword_chars(),
            boost: default_boost(),
            url: "http://127.0.0.1:9876".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmParams {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: u32,
    pub timeout_sec: f64,
    pub max_edits_per_1000: f64,
}

impl Default for LlmParams {
    fn default() -> Self {
        LlmParams {
            // Подтверждено владельцем: LLM-постобработка выключена по умолчанию
            // (работает только глоссарий); включается `transcribe.llm.enabled`.
            enabled: false,
            base_url: "http://127.0.0.1:8010/v1".to_string(),
            model: "chat".to_string(),
            temperature: 0.0,
            max_tokens: 2048,
            timeout_sec: 300.0,
            max_edits_per_1000: 10.0,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PostprocessParams {
    pub job_id: String,
    pub transcript_path: String,
    pub glossary: GlossaryParams,
    pub hotwords: HotwordsParams,
    pub llm: LlmParams,
}

// ------------------------------------------------------------------ ответы

#[derive(Debug, Clone, Deserialize)]
pub struct HelloResult {
    #[serde(default)]
    pub protocol: i64,
    #[serde(default)]
    pub python: String,
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub versions: Value,
    #[serde(default)]
    pub devices: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Word {
    #[serde(default)]
    pub word_id: u32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub alignment: String,
    #[serde(default)]
    pub overlap: bool,
    #[serde(default)]
    pub speakers: Vec<String>,
    /// P3: слово-кандидат на ручную проверку (`transcript.json.postprocess`).
    #[serde(default)]
    pub needs_review: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Block {
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub overlap: bool,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranscribeResult {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub transcript_path: String,
    #[serde(default)]
    pub diarization_path: Option<String>,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub speakers: Vec<String>,
    #[serde(default)]
    pub stats: Value,
    #[serde(default)]
    pub alignment_quality: Value,
    #[serde(default)]
    pub diarization: Value,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PostprocessResult {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub edits: Value,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub elapsed_ms: f64,
}
