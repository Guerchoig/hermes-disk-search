//! Устаревшее имя `hds whisper-check` — алиас `transcribe-check` (P1).
//!
//! Сама проверка — в [`crate::cmd::transcribe_check`]: новый стек GigaAM/pyannote,
//! а старые ключи `index.whisper_*` работают при `transcribe.engine: legacy`.
//! Файл оставлен, чтобы не ломать внешние вызовы (`hds whisper-check`, тесты).

pub use super::transcribe_check::{
    cmd_transcribe_check as cmd_whisper_check, cmd_transcribe_check_cfg as cmd_whisper_check_cfg,
};
