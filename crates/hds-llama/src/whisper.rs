//! W3: высокоуровневый транскрибатор поверх bridge-API: файл → сегменты с таймкодами.
//!
//! Схема (из SDK `docs/bridge-audio-dll.md` + `README.md`): bridge создаётся без
//! модели; metadata задаёт `whisper_model` (`.bin`), `whisper_gpu_device`, `mode`
//! и `custom`. `mode: subtitle` пишет **`.srt`** с таймкодами (окно `custom`, сек),
//! поэтому для индексации берём его и парсим в сегменты `{text,t_start,t_end}`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::bridge_audio::BridgeAudio;
use crate::error::{EngineError, Result};

/// Сегмент транскрипта с таймкодами (секунды).
#[derive(Debug, Clone, PartialEq)]
pub struct WhisperSegment {
    pub text: String,
    pub t_start: f64,
    pub t_end: f64,
}

/// Результат транскрибации файла.
#[derive(Debug, Clone)]
pub struct Transcript {
    /// Сырой JSON-ответ движка (метаданные, статистика, путь вывода).
    pub json: Value,
    pub segments: Vec<WhisperSegment>,
}

/// `HH:MM:SS,mmm` (или `HH:MM:SS.mmm`) → секунды.
fn srt_time(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Парсер SRT (`subtitle`-режим движка) → сегменты. Текст многострочный склеивается.
pub fn parse_srt(text: &str) -> Vec<WhisperSegment> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            continue;
        }
        // первый непустой — номер (может отсутствовать); ищем строку с "-->"
        let arrow = lines.iter().position(|l| l.contains("-->"));
        let arrow = match arrow {
            Some(i) => i,
            None => continue,
        };
        let (a, b) = match lines[arrow].split_once("-->") {
            Some((a, b)) => (a.trim(), b.trim()),
            None => continue,
        };
        let (t_start, t_end) = match (srt_time(a), srt_time(b)) {
            (Some(s), Some(e)) => (s, e),
            _ => continue,
        };
        let body = lines[arrow + 1..].join(" ");
        if body.trim().is_empty() {
            continue;
        }
        out.push(WhisperSegment {
            text: body.trim().to_string(),
            t_start,
            t_end,
        });
    }
    out
}

/// Скопировать файл в ASCII-каталог с ASCII-именем; вернуть путь к копии.
fn stage_ascii(src: &Path, ascii_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(ascii_dir)
        .map_err(|e| EngineError::Other(format!("{ascii_dir:?}: {e}")))?;
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("wav")
        .to_ascii_lowercase();
    let dst = ascii_dir.join(format!("input.{ext}"));
    std::fs::copy(src, &dst).map_err(|e| EngineError::Other(format!("стейджинг {src:?}: {e}")))?;
    Ok(dst)
}

/// Транскрибатор: bridge + модель whisper + устройство.
pub struct Whisper {
    bridge: BridgeAudio,
    model: PathBuf,
    gpu: i32,
    n_gpu_layers: i32,
}

impl Whisper {
    /// Собрать транскрибатор (bridge — из [`BridgeAudio::load`]).
    pub fn new(bridge: BridgeAudio, model: &Path, gpu: i32, n_gpu_layers: i32) -> Self {
        Whisper {
            bridge,
            model: model.to_path_buf(),
            gpu,
            n_gpu_layers,
        }
    }

    /// Путь к модели whisper.
    pub fn model(&self) -> &Path {
        &self.model
    }

    /// Транскрибация файла.
    ///
    /// `mode`: `"subtitle"` (таймкоды → `.srt`) или `"speech"` (сплошной текст → `.md`).
    /// `custom`: `"default"`/`"auto"`/число (окно `subtitle`, сек).
    pub fn transcribe_file(&self, src: &Path, mode: &str, custom: &str) -> Result<Transcript> {
        let ascii_dir = std::env::temp_dir().join("hds_whisper");
        let staged = stage_ascii(src, &ascii_dir)?;
        let ext = staged
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("wav")
            .to_string();
        let bytes =
            std::fs::read(&staged).map_err(|e| EngineError::Other(format!("{staged:?}: {e}")))?;
        let meta = json!({
            "mode": mode,
            "custom": custom,
            "whisper_model": self.model.to_string_lossy(),
            "whisper_gpu_device": self.gpu,
            "whisper_word_time_offset_sec": 0.73,
            "output_dir": ascii_dir.to_string_lossy(),
            "audio_source_path": staged.to_string_lossy(),
        });
        let out = self.bridge.transcribe_raw(
            None,
            Some(self.gpu),
            self.n_gpu_layers,
            &bytes,
            &ext,
            &meta.to_string(),
            true,
        )?;
        if !out.ok {
            return Err(EngineError::Other(format!(
                "whisper вернул ok=0 (status={}): {}",
                out.status, out.error
            )));
        }
        let v: Value = serde_json::from_str(&out.json).unwrap_or(Value::Null);
        let out_path = v
            .get("output")
            .and_then(|o| o.get("path"))
            .and_then(|p| p.as_str());
        let mut segments = Vec::new();
        if let Some(p) = out_path {
            let fp = PathBuf::from(p);
            if let Ok(text) = std::fs::read_to_string(&fp) {
                let is_srt = fp
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("srt"))
                    .unwrap_or(false);
                if is_srt {
                    segments = parse_srt(&text);
                } else {
                    let t = text.trim();
                    if !t.is_empty() {
                        segments.push(WhisperSegment {
                            text: t.to_string(),
                            t_start: 0.0,
                            t_end: 0.0,
                        });
                    }
                }
            }
        }
        Ok(Transcript { json: v, segments })
    }
}
