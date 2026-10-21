//! `hds-diarize` — диаризация Nemotron-3 (Sortformer v3) и аудио-примитивы
//! (`PLAN_DIARIZATION_NEMOTRON.md` W0/W3).
//!
//! Состав на шаге **W0** (без ONNX, тестируется без GPU/модели):
//! * [`wav`] — разбор и запись WAV (RIFF/PCM 16-bit) — формат выхода `ffmpeg`
//!   (`-ac 1 -ar 16000 -c:a pcm_s16le`); срез сэмплов для входа ASR;
//! * [`vad`] — энергетический VAD на RMS-кадрах: обрезка **ведущей** тишины
//!   (вариант A — режем только вход ASR, диаризация идёт по полному WAV) и доля
//!   речи в окне сегмента (отбраковка галлюцинаций в паузах);
//! * [`decode`] — декодирование медиа в WAV одним проходом через CLI `ffmpeg`.
//!
//! На шаге **W3** добавится `sortformer` (обёртка `parakeet-rs`: EP по платформе,
//! чанкинг `feed()`/`flush()`), `merge` (сегменты ASR × turns → блоки) и
//! `render` (`.md`/`.srt` в текущем формате `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`).
//!
//! Инварианты таймлайна (`PLAN_DIARIZATION_NEMOTRON.md` §3.4): все времена —
//! абсолютные, от начала исходного медиа; `trim_offset` из [`vad::TrimConfig`]
//! прибавляется к таймкодам ASR **ровно один раз**.

pub mod decode;
pub mod vad;
pub mod wav;

pub use decode::decode_to_wav;
pub use vad::{leading_silence_samples, speech_stats, SpeechStats, TrimConfig};
pub use wav::{read_wav, write_wav, Wav};

/// Частота дискретизации аудио-ветки (как у `ffmpeg -ar 16000`).
pub const SAMPLE_RATE: u32 = 16_000;

/// Ошибки крейта: ошибки ввода-вывода, разбора контейнера и внешних CLI.
#[derive(Debug, thiserror::Error)]
pub enum DiarizeError {
    /// Ошибка файловой системы/процесса; текст уже содержит путь.
    #[error("{0}")]
    Io(String),
    /// Контейнер/формат не тот, что ожидался (не RIFF/WAVE, не PCM 16-bit, …).
    #[error("{0}")]
    Format(String),
    /// Внешняя утилита вернула ошибку (stderr в тексте).
    #[error("{0}")]
    Tool(String),
}

/// Результат операций крейта.
pub type Result<T> = std::result::Result<T, DiarizeError>;
