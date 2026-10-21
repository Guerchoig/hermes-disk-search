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

/// Максимум сэмплов PCM 16-bit в RIFF/WAV: размеры чанков — `u32`, поэтому для
/// 16 кГц mono предел ≈37 ч (F8, ревью M5). Сам продуктовый предел короче —
/// [`crate::MAX_DIAR_SEC`] (6 ч, решение владельца Q4).
pub const MAX_RIFF_SAMPLES: usize = (u32::MAX as usize - 36) / 2;

/// Разобрать WAV из памяти (без файловой системы — удобно тестировать).
pub fn parse_wav(buf: &[u8]) -> Result<Wav> {
    if buf.len() >= 4 && &buf[0..4] == b"RF64" {
        return Err(DiarizeError::Format(
            "WAV/RF64 не поддерживается (запись > 4 ГиБ)".to_string(),
        ));
    }
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
    encode_wav_with_limit(samples, sample_rate, usize::MAX)
        .expect("лимит usize::MAX не превышается")
}

/// Собрать WAV с проверкой предела RIFF (`u32`) — F8 (ревью M5).
///
/// Раньше `data_len` усекался до `u32`, и запись длиннее ≈6.7 ч давала **битый**
/// файл; теперь — понятная ошибка вместо тихой порчи данных.
pub fn encode_wav_checked(samples: &[i16], sample_rate: u32) -> Result<Vec<u8>> {
    encode_wav_with_limit(samples, sample_rate, MAX_RIFF_SAMPLES)
}

/// Общая реализация `encode_wav*`: лимит параметром — чтобы тест проверил
/// арифметику на маленьком массиве, не выделяя > 4 ГиБ.
fn encode_wav_with_limit(samples: &[i16], sample_rate: u32, max: usize) -> Result<Vec<u8>> {
    if samples.len() > max {
        let hours = samples.len() as f64 / sample_rate.max(1) as f64 / 3600.0;
        let limit = max as f64 / sample_rate.max(1) as f64 / 3600.0;
        return Err(DiarizeError::Format(format!(
            "WAV > 4 ГиБ: RIFF не поддерживается — запись слишком длинная \
             ({hours:.1} ч при {sample_rate} Гц mono; предел RIFF ≈{limit:.1} ч)"
        )));
    }
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
    Ok(out)
}

/// Прочитать WAV с диска.
pub fn read_wav(path: &Path) -> Result<Wav> {
    // F8: RF64 (запись > 4 ГиБ) не читаем в память — понятная ошибка сразу.
    {
        use std::io::Read;
        let mut head = [0u8; 4];
        if let Ok(mut f) = fs::File::open(path) {
            if f.read_exact(&mut head).is_ok() && head == *b"RF64" {
                return Err(DiarizeError::Format(format!(
                    "{}: WAV/RF64 не поддерживается (запись > 4 ГиБ)",
                    path.display()
                )));
            }
        }
    }
    let buf = fs::read(path).map_err(|e| DiarizeError::Io(format!("{}: {e}", path.display())))?;
    parse_wav(&buf)
}

/// Записать WAV (PCM 16-bit, mono) на диск.
///
/// F8: предел RIFF проверяется **до** записи — файл не создаётся битым.
pub fn write_wav(path: &Path, samples: &[i16], sample_rate: u32) -> Result<()> {
    let bytes = encode_wav_checked(samples, sample_rate)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| DiarizeError::Io(format!("{}: {e}", dir.display())))?;
    }
    fs::write(path, bytes).map_err(|e| DiarizeError::Io(format!("{}: {e}", path.display())))
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

    /// F8 (ревью M5): запись длиннее предела RIFF — понятная ошибка, а не битый
    /// файл. Арифметику проверяем на маленьком лимите (4 ГиБ не выделяем).
    #[test]
    fn encode_wav_checked_rejects_overflow() {
        let samples = [0i16; 10];
        assert!(encode_wav_with_limit(&samples, 16_000, 10).is_ok());
        let err = encode_wav_with_limit(&samples, 16_000, 9).unwrap_err();
        assert!(matches!(err, DiarizeError::Format(_)), "{err}");
        assert!(err.to_string().contains("RIFF"), "{err}");
        // Предел RIFF по умолчанию — для 16 кГц mono это ≈37 ч (4 ГиБ / 32 КБ/с).
        let limit_hours = MAX_RIFF_SAMPLES as f64 / 16_000.0 / 3600.0;
        assert!(
            (36.0..38.0).contains(&limit_hours),
            "предел {limit_hours:.2} ч"
        );
        // Продуктовый предел короче (6 ч, решение Q4) и REACHED раньше RIFF.
        assert!(crate::MAX_DIAR_SEC < limit_hours * 3600.0);
        assert!(encode_wav_checked(&samples, 16_000).is_ok());
    }

    /// F8: RF64 (ffmpeg пишет его для > 4 ГиБ) отвергается с подсказкой —
    /// и через `parse_wav`, и через `read_wav` (без чтения файла целиком).
    #[test]
    fn parse_wav_rejects_rf64_with_hint() {
        let mut buf = vec![0u8; 44];
        buf[0..4].copy_from_slice(b"RF64");
        buf[8..12].copy_from_slice(b"WAVE");
        let err = parse_wav(&buf).unwrap_err();
        assert!(err.to_string().contains("RF64"), "{err}");

        let p = std::env::temp_dir().join(format!("hds-rf64-{}.wav", std::process::id()));
        fs::write(&p, &buf).unwrap();
        let err = read_wav(&p).unwrap_err();
        assert!(err.to_string().contains("RF64"), "{err}");
        fs::remove_file(&p).ok();
    }
}
