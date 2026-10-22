//! `hds-transcribe` — типизированный клиент Python-воркера транскрибации
//! (PLAN_PYTHON_SIDECAR_TRANSCRIBE.md §5.4).
//!
//! Состав:
//! * [`params`] — `TranscribeParams`/`PostprocessParams` и результаты (serde);
//! * [`settings`] — чтение `transcribe.*` + маппинг старых ключей;
//! * [`error`] — машинные коды (`model_missing`, `cuda_oom`, …) и их текст;
//! * [`client`] — запуск воркера, `hello`/`probe`/`transcribe`/`postprocess`.

pub mod client;
pub mod error;
pub mod params;
pub mod settings;

pub use client::{discover_python, Client, ClientConfig};
pub use error::{ErrorKind, TranscribeError};
pub use params::{
    AlignParams, AlignQuality, AsrParams, Block, DecodeParams, DiarizationParams, GlossaryParams,
    HelloResult, HotwordsParams, LlmParams, PostprocessParams, PostprocessResult, TranscribeParams,
    TranscribeResult, Word,
};
pub use settings::Settings;
