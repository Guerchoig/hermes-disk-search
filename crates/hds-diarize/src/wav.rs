//! Чтение и запись WAV (RIFF/PCM 16-bit).
//!
//! Формат зафиксирован декодером варианта A: `ffmpeg -vn -ac 1 -ar 16000
//! -c:a pcm_s16le` → mono 16 кГц 16-bit. Другие варианты (float, 24-bit,
//! multichannel) сознательно **не** поддерживаются: их не бывает на этом пути,
//! а тихое «пересемплирование на лету» скрыло бы ошибку декодера.
//!
//! `hound`/`symphonia` не подключаем (правило «никаких новых зависимостей»,
//! `DIARIZATION_NEMOTRON_HANDOFF.md` п. 7): разбор RIFF здесь — 80 строк.

use std::fs;
use std::path::Path;

use crate::{DiarizeError, Result};

/// Разобранный WAV: сэмплы PCM 16-bit (interleaved, для mono — как есть).
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    /// Частота дискретизации, Гц.
    pub sample_rate: u32,
    /// Число каналов (на пути диаризации/ASR — всегда 1).
    pub channels: u16,
    /// Сэмплы в порядке файла.
    pub samples: Vec<i16>,
}

impl Wav {
    /// Длительность, секунды.
    pub fn duration_sec(&self) -> f64 {
        if self.sample_rate == 0 || self.channels == 0 {
            return 0.0;
        }
        self.samples.len() as f64 / (self.sample_rate as f64 * self.channels as f64)
    }

    /// Сэмплы как `f32` в диапазоне −1.0…1.0 (вход `parakeet-rs::Sortformer`).
    pub fn to_f32(&self) -> Vec<f32> {
        self.samples.iter().map(|s| *s as f32 / 32768.0).collect()
    }

    /// Срез сэмплов начиная с `start` (для входа ASR; копия, не второй декод).
    pub fn slice_from(&self, start: usize) -> &[i16] {
        let start = start.min(self.samples.len());
        &self.samples[start..]
    }
}

/// Разобрать WAV из памяти (без файловой системы — удобно тестировать).
pub fn parse_wav(buf: &[u8]) -> Result<Wav> {
    if buf.len() < 44 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return Err(DiarizeError::Format("не RIFF/WAVE".to_string()));
    }
    let mut pos = 12usize;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut format = 0u16;
    let mut data: Option<(usize, usize)> = None;
    while pos + 8 <= buf.len() {
        let id = &buf[pos..pos + 4];
        let size =
            u32::from_le_bytes([buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]]) as usize;
        let body = pos + 8;
        match id {
            b"fmt " => {
                if body + 16 > buf.len() {
                    return Err(DiarizeError::Format("обрыв fmt-чанка".to_string()));
                }
                format = u16::from_le_bytes([buf[body], buf[body + 1]]);
                channels = u16::from_le_bytes([buf[body + 2], buf[body + 3]]);
                rate = u32::from_le_bytes([
                    buf[body + 4],
                    buf[body + 5],
                    buf[body + 6],
                    buf[body + 7],
                ]);
                bits = u16::from_le_bytes([buf[body + 14], buf[body + 15]]);
            }
            b"data" => {
                // Некоторые кодировщики пишут размер «до конца файла» — обрезаем.
                let end = (body + size).min(buf.len());
                data = Some((body, end));
            }
            _ => {}
        }
        // Чанки выровнены по чётной границе.
        pos = body + size + (size & 1);
    }
    // `format == 1` — PCM; `0xFFFE` (WAVE_FORMAT_EXTENSIBLE) на пути ffmpeg не встречается.
    if format != 1 {
        return Err(DiarizeError::Format(format!(
            "ожидался PCM (format 1), получено {format}"
        )));
    }
    if bits != 16 {
        return Err(DiarizeError::Format(format!(
            "ожидался PCM 16-bit, получено {bits}"
        )));
    }
    let (start, end) = data.ok_or_else(|| DiarizeError::Format("нет data-чанка".to_string()))?;
    let mut samples = Vec::with_capacity((end - start) / 2);
    for c in buf[start..end].chunks_exact(2) {
        samples.push(i16::from_le_bytes([c[0], c[1]]));
    }
    Ok(Wav {
        sample_rate: rate,
        channels,
        samples,
    })
}

/// Собрать WAV (PCM 16-bit, mono) в память.
pub fn encode_wav(samples: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // размер fmt-чанка
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Прочитать WAV с диска.
pub fn read_wav(path: &Path) -> Result<Wav> {
    let buf = fs::read(path).map_err(|e| DiarizeError::Io(format!("{}: {e}", path.display())))?;
    parse_wav(&buf)
}

/// Записать WAV (PCM 16-bit, mono) на диск.
pub fn write_wav(path: &Path, samples: &[i16], sample_rate: u32) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| DiarizeError::Io(format!("{}: {e}", dir.display())))?;
    }
    fs::write(path, encode_wav(samples, sample_rate))
        .map_err(|e| DiarizeError::Io(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_samples_and_rate() {
        let samples: Vec<i16> = (0..1600)
            .map(|i| ((i as f32 * 0.1).sin() * 12_000.0) as i16)
            .collect();
        let w = parse_wav(&encode_wav(&samples, 16_000)).unwrap();
        assert_eq!(w.sample_rate, 16_000);
        assert_eq!(w.channels, 1);
        assert_eq!(w.samples, samples);
        assert!((w.duration_sec() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn rejects_non_riff() {
        let err = parse_wav(b"not a wave file at all, definitely not 44 bytes").unwrap_err();
        assert!(matches!(err, DiarizeError::Format(_)), "{err}");
    }

    #[test]
    fn rejects_8bit_and_float() {
        let mut buf = encode_wav(&[1, 2, 3], 16_000);
        // bits per sample в fmt-чанке: 44-байтный заголовок, смещение 34.
        buf[34] = 8;
        buf[35] = 0;
        let err = parse_wav(&buf).unwrap_err();
        assert!(err.to_string().contains("16-bit"), "{err}");

        let mut buf = encode_wav(&[1, 2, 3], 16_000);
        buf[20] = 3; // IEEE float
        buf[21] = 0;
        let err = parse_wav(&buf).unwrap_err();
        assert!(err.to_string().contains("PCM"), "{err}");
    }

    #[test]
    fn tolerates_data_size_beyond_eof() {
        // data-чанк «до конца файла»: заявленный размер больше фактического.
        let mut buf = encode_wav(&[7; 8], 16_000);
        let n = buf.len() as u32;
        buf[40..44].copy_from_slice(&(n * 2).to_le_bytes());
        let w = parse_wav(&buf).unwrap();
        assert_eq!(w.samples.len(), 8);
    }

    #[test]
    fn slice_from_clamps_and_to_f32_scales() {
        let w = Wav {
            sample_rate: 16_000,
            channels: 1,
            samples: vec![1, 2, 3, 4],
        };
        assert_eq!(w.slice_from(2), &[3, 4]);
        assert!(w.slice_from(99).is_empty());
        assert_eq!(w.to_f32()[0], 1.0 / 32768.0);
    }
}
