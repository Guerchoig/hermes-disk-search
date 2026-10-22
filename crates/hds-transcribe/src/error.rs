//! Машинные коды ошибок воркера и их превращение в ошибку ядра.

use std::fmt;

use hds_core::error::CoreError;

/// Код ошибки задания (`error.kind` в JSON-RPC; §5.2 плана).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    ModelMissing,
    HfTokenMissing,
    FfmpegMissing,
    CudaOom,
    DmlOom,
    OnnxError,
    DiarizationFailed,
    Cancelled,
    BadAudio,
    AsrFailed,
    /// `align.mode: whisperx` без whisperx в venv профиля (P2).
    AlignFailed,
    BadRequest,
    Internal,
    Unknown,
}

impl ErrorKind {
    /// Разбор строки из `error.kind` (неизвестное → `Unknown`).
    pub fn from_code(code: &str) -> Self {
        match code {
            "model_missing" => ErrorKind::ModelMissing,
            "hf_token_missing" => ErrorKind::HfTokenMissing,
            "ffmpeg_missing" => ErrorKind::FfmpegMissing,
            "cuda_oom" => ErrorKind::CudaOom,
            "dml_oom" => ErrorKind::DmlOom,
            "onnx_error" => ErrorKind::OnnxError,
            "diarization_failed" => ErrorKind::DiarizationFailed,
            "cancelled" => ErrorKind::Cancelled,
            "bad_audio" => ErrorKind::BadAudio,
            "asr_failed" => ErrorKind::AsrFailed,
            "align_failed" => ErrorKind::AlignFailed,
            "bad_request" => ErrorKind::BadRequest,
            "internal_error" => ErrorKind::Internal,
            _ => ErrorKind::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::ModelMissing => "model_missing",
            ErrorKind::HfTokenMissing => "hf_token_missing",
            ErrorKind::FfmpegMissing => "ffmpeg_missing",
            ErrorKind::CudaOom => "cuda_oom",
            ErrorKind::DmlOom => "dml_oom",
            ErrorKind::OnnxError => "onnx_error",
            ErrorKind::DiarizationFailed => "diarization_failed",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::BadAudio => "bad_audio",
            ErrorKind::AsrFailed => "asr_failed",
            ErrorKind::AlignFailed => "align_failed",
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Internal => "internal_error",
            ErrorKind::Unknown => "unknown",
        }
    }

    /// Стоит ли повторить стадию на CPU (`transcribe.fallback_cpu`).
    pub fn is_cpu_fallback(self) -> bool {
        matches!(
            self,
            ErrorKind::CudaOom | ErrorKind::DmlOom | ErrorKind::OnnxError
        )
    }
}

/// Ошибка транскрибации: код + текст + подсказка.
#[derive(Debug, Clone)]
pub struct TranscribeError {
    pub kind: ErrorKind,
    pub message: String,
    pub hint: Option<String>,
}

impl TranscribeError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        TranscribeError {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    /// Из ошибки транспорта + объектов `kind`/`hint` последнего ответа.
    pub fn from_transport(err: &CoreError, kind: Option<&str>, hint: Option<&str>) -> Self {
        let message = err.message();
        let kind = kind
            .map(ErrorKind::from_code)
            .unwrap_or_else(|| infer_kind(&message));
        TranscribeError {
            kind,
            message,
            hint: hint.map(|h| h.to_string()),
        }
    }

    /// Текст задания (как `str(e)` в старом пути): код + сообщение + подсказка.
    pub fn to_core(&self) -> CoreError {
        let mut text = format!("[{}] {}", self.kind.as_str(), self.message);
        if let Some(hint) = &self.hint {
            text.push_str(" — ");
            text.push_str(hint);
        }
        CoreError::Other(text)
    }
}

impl fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " — {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TranscribeError {}

/// Эвристика по тексту, если воркер не прислал `kind` (старый протокол/падение).
fn infer_kind(message: &str) -> ErrorKind {
    let low = message.to_lowercase();
    if low.contains("out of memory") || low.contains("cuda error") {
        if low.contains("dml") || low.contains("directml") {
            ErrorKind::DmlOom
        } else {
            ErrorKind::CudaOom
        }
    } else if low.contains("ffmpeg") || low.contains("ffprobe") {
        ErrorKind::FfmpegMissing
    } else if low.contains("403") || low.contains("gated") || low.contains("hf_token") {
        ErrorKind::HfTokenMissing
    } else {
        ErrorKind::Unknown
    }
}
