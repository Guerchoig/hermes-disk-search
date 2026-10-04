//! W3: высокоуровневый транскрибатор поверх bridge-API: файл → сегменты с таймкодами.
//!
//! Схема (из SDK `docs/bridge-audio-dll.md` + `README.md`): bridge создаётся без
//! модели; metadata задаёт `whisper_model` (`.bin`), `whisper_gpu_device`, `mode`
//! и `custom`. `mode: subtitle` пишет **`.srt`** с таймкодами (окно `custom`, сек),
//! поэтому для индексации берём его и парсим в сегменты `{text,t_start,t_end}`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::bridge_audio::{Bridge, BridgeAudio};
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
    /// Полный текст файла движка **как есть** (для `transcript` — `.md` с метками
    /// спикеров `SPEAKER_NN`/`UNASSIGNED`); пишется в `out_dir` без конвертации
    /// (`PLAN_AUTO_TRANSCRIBE` §5.3).
    pub raw_text: String,
    /// Расширение выходного файла движка без точки (`md`, `srt`); пусто, если путь
    /// вывода неизвестен.
    pub out_ext: String,
}

/// Параметры диаризации (sortformer) для офлайн-режима `mode: transcript`.
///
/// Движок включает диаризацию **только** при `mode: transcript` и **требует**
/// модель: без неё отвечает `400 Missing diarization model source` —
/// деградации в `speech` нет (`PLAN_AUTO_TRANSCRIBE` §0 п.6; подтверждено
/// спайком T0.1, `tools/parity/T0_1_DIARIZATION_FORMAT.md` §5).
#[derive(Debug, Clone, PartialEq)]
pub struct DiarizationParams {
    /// Путь к `.gguf` sortformer-модели.
    pub model_path: PathBuf,
    /// `diarization_backend` движка (референс: `sortformer`).
    pub backend: String,
    /// `diarization_feed_ms` — окно подачи аудио диаризатору, мс.
    pub feed_ms: f64,
    /// `diarization_device` (`CUDA0`/`CPU`); `None` — не задавать (выберет движок).
    pub device: Option<String>,
}

impl DiarizationParams {
    /// Дефолты референса (`backend: sortformer`, `feed_ms: 10800001`, устройство
    /// не задано).
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        DiarizationParams {
            model_path: model_path.into(),
            backend: "sortformer".to_string(),
            feed_ms: 10_800_001.0,
            device: None,
        }
    }

    /// Задать устройство диаризации (`CUDA0`, `CUDA1`, `CPU`, …).
    pub fn with_device(mut self, device: impl Into<String>) -> Self {
        self.device = Some(device.into());
        self
    }
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

/// Транскрибатор: постоянный bridge (audio-only) + модель whisper.
pub struct Whisper {
    bridge: Bridge,
    model: PathBuf,
    gpu: i32,
}

impl Whisper {
    /// Собрать транскрибатор: создаёт **постоянный** bridge (audio-only) для роли whisper.
    pub fn new(
        bridge_audio: BridgeAudio,
        model: &Path,
        gpu: i32,
        n_gpu_layers: i32,
    ) -> Result<Self> {
        let bridge = bridge_audio.create(None, Some(gpu), n_gpu_layers)?;
        Ok(Whisper {
            bridge,
            model: model.to_path_buf(),
            gpu,
        })
    }

    /// Путь к модели whisper.
    pub fn model(&self) -> &Path {
        &self.model
    }

    /// Устройство инференса (`>=0` — GPU-индекс, `<0` — CPU).
    pub fn gpu(&self) -> i32 {
        self.gpu
    }

    /// Транскрибация файла.
    ///
    /// `mode`: `"subtitle"` (таймкоды → `.srt`), `"speech"` (сплошной текст → `.md`)
    /// или `"transcript"` (транскрибация **и** диаризация одним вызовом → `.md` с
    /// метками `SPEAKER_NN`; требует `diar` — см. [`DiarizationParams`]).
    /// `custom`: `"default"`/`"auto"`/число (окно `subtitle`, сек).
    ///
    /// При `gpu < 0` движку выставляется `whisper_no_gpu: true` (CPU) — так требует
    /// SDK (§8.3: не задавать оба, устройство задавать явно).
    pub fn transcribe_file(
        &self,
        src: &Path,
        mode: &str,
        custom: &str,
        diar: Option<&DiarizationParams>,
    ) -> Result<Transcript> {
        let ascii_dir = std::env::temp_dir().join("hds_whisper");
        let staged = stage_ascii(src, &ascii_dir)?;
        let ext = staged
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("wav")
            .to_string();
        let bytes =
            std::fs::read(&staged).map_err(|e| EngineError::Other(format!("{staged:?}: {e}")))?;
        let meta = build_metadata(
            &self.model,
            self.gpu,
            mode,
            custom,
            &ascii_dir,
            &staged,
            diar,
        );
        let out = self
            .bridge
            .transcribe_raw(&bytes, &ext, &meta.to_string(), true)?;
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
        let mut raw_text = String::new();
        let mut out_ext = String::new();
        let mut segments = Vec::new();
        if let Some(p) = out_path {
            let fp = PathBuf::from(p);
            out_ext = fp
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if let Ok(text) = std::fs::read_to_string(&fp) {
                let is_srt = out_ext == "srt";
                if is_srt {
                    segments = parse_srt(&text);
                } else {
                    // `speech`/`transcript`: текст «как есть» (§5.2–5.3 плана).
                    raw_text = text;
                    let t = raw_text.trim();
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
        Ok(Transcript {
            json: v,
            segments,
            raw_text,
            out_ext,
        })
    }
}

/// `metadata_json` для bridge-audio (отдельная функция — тестируется без GPU).
///
/// Ключи диаризации кладём **только** при `Some(diar)`: их наличие само по себе
/// ничего не включает (режим задаётся `mode`), но при `mode: transcript` без
/// модели движок ответит `400` — это и есть «жёсткий отказ» из §0 п.6.
fn build_metadata(
    model: &Path,
    gpu: i32,
    mode: &str,
    custom: &str,
    output_dir: &Path,
    staged: &Path,
    diar: Option<&DiarizationParams>,
) -> Value {
    let mut meta = json!({
        "mode": mode,
        "custom": custom,
        "whisper_model": model.to_string_lossy(),
        "whisper_word_time_offset_sec": 0.73,
        "output_dir": output_dir.to_string_lossy(),
        "audio_source_path": staged.to_string_lossy(),
    });
    if gpu >= 0 {
        meta["whisper_gpu_device"] = json!(gpu);
    } else {
        meta["whisper_no_gpu"] = json!(true);
    }
    if let Some(d) = diar {
        meta["diarization_model_path"] = json!(d.model_path.to_string_lossy());
        meta["diarization_backend"] = json!(d.backend);
        meta["diarization_feed_ms"] = json!(d.feed_ms);
        if let Some(dev) = &d.device {
            meta["diarization_device"] = json!(dev);
        }
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Без диаризации ключей `diarization*` в metadata нет, а устройство whisper
    /// задано явно (SDK §8.3: `gpu` и `no_gpu` не вместе).
    #[test]
    fn metadata_without_diarization_keeps_whisper_keys_only() {
        let meta = build_metadata(
            Path::new("w.bin"),
            0,
            "subtitle",
            "4.5",
            Path::new("/tmp/out"),
            Path::new("/tmp/out/input.wav"),
            None,
        );
        assert_eq!(meta["mode"], "subtitle");
        assert_eq!(meta["whisper_gpu_device"], 0);
        assert!(meta.get("whisper_no_gpu").is_none(), "не оба ключа сразу");
        for key in [
            "diarization_model_path",
            "diarization_backend",
            "diarization_feed_ms",
            "diarization_device",
        ] {
            assert!(meta.get(key).is_none(), "лишний ключ {key}: {meta}");
        }
    }

    /// С `DiarizationParams` появляются ровно четыре ключа движка (референс),
    /// значения — как в спайке T0.1.
    #[test]
    fn metadata_with_diarization_adds_sortformer_keys() {
        let diar = DiarizationParams::new("C:/models/sortformer.gguf").with_device("CUDA0");
        let meta = build_metadata(
            Path::new("w.bin"),
            0,
            "transcript",
            "auto",
            Path::new("/tmp/out"),
            Path::new("/tmp/out/input.wav"),
            Some(&diar),
        );
        assert_eq!(meta["mode"], "transcript");
        assert_eq!(meta["custom"], "auto");
        assert_eq!(meta["diarization_backend"], "sortformer");
        assert_eq!(meta["diarization_feed_ms"], 10_800_001.0);
        assert_eq!(meta["diarization_device"], "CUDA0");
        assert!(
            meta["diarization_model_path"]
                .as_str()
                .unwrap_or_default()
                .ends_with("sortformer.gguf"),
            "путь модели: {}",
            meta["diarization_model_path"]
        );
    }

    /// `gpu < 0` ⇒ CPU-ключ, без `whisper_gpu_device`; `DiarizationParams` без
    /// устройства не добавляет `diarization_device`.
    #[test]
    fn metadata_cpu_path_and_default_diarization_device() {
        let diar = DiarizationParams::new("m.gguf");
        let meta = build_metadata(
            Path::new("w.bin"),
            -1,
            "transcript",
            "auto",
            Path::new("/tmp/out"),
            Path::new("/tmp/out/input.wav"),
            Some(&diar),
        );
        assert_eq!(meta["whisper_no_gpu"], true);
        assert!(meta.get("whisper_gpu_device").is_none());
        assert!(
            meta.get("diarization_device").is_none(),
            "устройство не задано — ключ не добавляем"
        );
    }

    /// Дефолты референса: `sortformer` + `10800001` мс (спайк T0.1 §1).
    #[test]
    fn diarization_params_defaults_match_reference() {
        let d = DiarizationParams::new("m.gguf");
        assert_eq!(d.backend, "sortformer");
        assert_eq!(d.feed_ms, 10_800_001.0);
        assert_eq!(d.device, None);
        assert_eq!(d.with_device("CPU").device.as_deref(), Some("CPU"));
    }
}
