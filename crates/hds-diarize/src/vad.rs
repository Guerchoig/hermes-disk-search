//! Энергетический VAD на RMS-кадрах (без внешних зависимостей).
//!
//! Две задачи (PLAN_DIARIZATION_NEMOTRON.md §3.4):
//! 1. **Обрезка ведущей тишины** входа ASR — [`leading_silence_samples`].
//!    Режется только начало файла и только пока не найдена *устойчивая* речь:
//!    окно `min_speech_ms`, в котором доля кадров выше порога ≥ `min_speech_ratio`.
//!    Так одиночный шумовой выброс не считается речью, а тихая первая реплика
//!    («Добрый день», пик −34.8 dBFS на 18-мин файле) остаётся: порог −50 dBFS
//!    и запас `lead_in_sec` перед найденной речью.
//! 2. **Доля речи в окне сегмента** — [`speech_stats`]: по ней отбрасываются
//!    галлюцинированные реплики whisper в паузах («Продолжение следует…»).
//!
//! Внутренние паузы **не** вырезаются: иначе смещения стали бы переменными и
//! сшивка с диаризацией разъехалась бы (`stop_periods` в `ffmpeg` не используем).

/// Параметры обрезки ведущей тишины.
#[derive(Debug, Clone, PartialEq)]
pub struct TrimConfig {
    /// Порог «речь», dBFS (RMS кадра). По умолчанию −50 dBFS.
    pub threshold_db: f64,
    /// Длина кадра, мс (20 мс = 320 сэмплов при 16 кГц).
    pub frame_ms: f64,
    /// Длина окна «устойчивой речи», мс.
    pub min_speech_ms: f64,
    /// Минимальная доля кадров выше порога в окне (0.5 = половина окна).
    pub min_speech_ratio: f64,
    /// Сколько оставить перед найденной речью, секунды.
    pub lead_in_sec: f64,
}

impl Default for TrimConfig {
    fn default() -> Self {
        TrimConfig {
            threshold_db: -50.0,
            frame_ms: 20.0,
            min_speech_ms: 200.0,
            min_speech_ratio: 0.5,
            lead_in_sec: 0.3,
        }
    }
}

impl TrimConfig {
    /// Параметры из конфига `diarization.asr.*` (значения — как в образце).
    pub fn new(threshold_db: f64, lead_in_sec: f64) -> Self {
        TrimConfig {
            threshold_db,
            lead_in_sec,
            ..TrimConfig::default()
        }
    }
}

/// Статистика речи в окне `[t0, t1)`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechStats {
    /// Доля кадров выше порога (0…1).
    pub ratio: f64,
    /// Суммарная длительность кадров выше порога, секунды.
    pub speech_sec: f64,
    /// Длительность окна, по которому считали, секунды.
    pub total_sec: f64,
}

/// RMS кадра `[start, start+len)` в dBFS (`-180` для цифровой тишины).
pub fn frame_db(samples: &[i16], start: usize, len: usize) -> f64 {
    if len == 0 || start >= samples.len() {
        return -180.0;
    }
    let end = (start + len).min(samples.len());
    let mut acc = 0.0f64;
    for s in &samples[start..end] {
        let v = *s as f64 / 32768.0;
        acc += v * v;
    }
    let rms = (acc / (end - start) as f64).sqrt();
    20.0 * (rms.max(1e-9)).log10()
}

/// Индекс первого сэмпла, который следует оставить после обрезки ведущей тишины.
///
/// Возвращает `0`, если файл начинается с речи (или пуст), и `samples.len()`,
/// если речи нет вовсе (файл целиком — тишина/шум). Значение — в сэмплах, чтобы
/// offset не накапливал ошибку округления (точность 1/16000 с).
///
/// Точка обрезки = начало найденного окна минус `lead_in_sec`. Окно с долей
/// `min_speech_ratio` может начаться раньше реальной атаки речи на
/// `frame_ms × (window − нужно_кадров)` (при дефолтах — до 100 мс), то есть
/// функция ошибается **в сторону сохранения** аудио, а не отрезания речи.
pub fn leading_silence_samples(samples: &[i16], sample_rate: u32, cfg: &TrimConfig) -> usize {
    if samples.is_empty() || sample_rate == 0 {
        return 0;
    }
    let frame = ((sample_rate as f64 * cfg.frame_ms / 1000.0).round() as usize).max(1);
    let frames = samples.len() / frame;
    if frames == 0 {
        return 0;
    }
    let window = ((cfg.min_speech_ms / cfg.frame_ms).round() as usize).max(1);
    let needed = ((window as f64 * cfg.min_speech_ratio).ceil() as usize).max(1);

    let above: Vec<bool> = (0..frames)
        .map(|i| frame_db(samples, i * frame, frame) >= cfg.threshold_db)
        .collect();

    let mut onset_frame: Option<usize> = None;
    for i in 0..frames {
        let end = (i + window).min(frames);
        let hit = above[i..end].iter().filter(|v| **v).count();
        // Окно у конца файла короче — масштабируем требование.
        let need = if end - i < window {
            ((end - i) as f64 * cfg.min_speech_ratio).ceil() as usize
        } else {
            needed
        };
        if hit >= need.max(1) {
            onset_frame = Some(i);
            break;
        }
    }

    let Some(i) = onset_frame else {
        return samples.len();
    };
    let lead = (cfg.lead_in_sec.max(0.0) * sample_rate as f64).round() as usize;
    (i * frame).saturating_sub(lead)
}

/// Доля речи в окне `[t0, t1)` (секунды) — для постфильтра сегментов ASR.
///
/// Кадры, не попавшие в окно целиком, не учитываются; окно короче кадра даёт
/// `ratio = 0` и `total_sec = 0` (вызывающий решает, что с этим делать).
pub fn speech_stats(
    samples: &[i16],
    sample_rate: u32,
    t0: f64,
    t1: f64,
    threshold_db: f64,
    frame_ms: f64,
) -> SpeechStats {
    if samples.is_empty() || sample_rate == 0 || t1 <= t0 {
        return SpeechStats {
            ratio: 0.0,
            speech_sec: 0.0,
            total_sec: 0.0,
        };
    }
    let frame = ((sample_rate as f64 * frame_ms / 1000.0).round() as usize).max(1);
    let start = ((t0.max(0.0) * sample_rate as f64).round() as usize).min(samples.len());
    let end = ((t1.max(0.0) * sample_rate as f64).round() as usize).min(samples.len());
    if end <= start {
        return SpeechStats {
            ratio: 0.0,
            speech_sec: 0.0,
            total_sec: 0.0,
        };
    }
    // Выравниваем границы по кадрам (внутрь), чтобы не считать половину кадра.
    let first = start.div_ceil(frame);
    let last = end / frame;
    if last <= first {
        let sec = (end - start) as f64 / sample_rate as f64;
        let speech = frame_db(samples, start, end - start) >= threshold_db;
        return SpeechStats {
            ratio: if speech { 1.0 } else { 0.0 },
            speech_sec: if speech { sec } else { 0.0 },
            total_sec: sec,
        };
    }
    let count = last - first;
    let mut hits = 0usize;
    for i in first..last {
        if frame_db(samples, i * frame, frame) >= threshold_db {
            hits += 1;
        }
    }
    let frame_sec = frame as f64 / sample_rate as f64;
    SpeechStats {
        ratio: hits as f64 / count as f64,
        speech_sec: hits as f64 * frame_sec,
        total_sec: count as f64 * frame_sec,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    /// Синус с заданной амплитудой (доля полной шкалы) на `sec` секунд.
    fn tone(sec: f64, amp: f64, freq: f64) -> Vec<i16> {
        let n = (sec * SR as f64) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / SR as f64;
                ((2.0 * std::f64::consts::PI * freq * t).sin() * amp * 32_767.0) as i16
            })
            .collect()
    }

    fn silence(sec: f64) -> Vec<i16> {
        vec![0i16; (sec * SR as f64) as usize]
    }

    fn concat(parts: &[Vec<i16>]) -> Vec<i16> {
        parts.iter().flatten().copied().collect()
    }

    #[test]
    fn trims_leading_silence_with_lead_in() {
        let audio = concat(&[silence(1.0), tone(1.0, 0.5, 440.0)]);
        let cut = leading_silence_samples(&audio, SR, &TrimConfig::default());
        // Речь начинается на 1.0 с; окно 200 мс при доле 0.5 ловит её на 0.1 с
        // раньше, минус lead-in 0.3 с ⇒ 0.6 с (допуск на кадр 20 мс).
        let sec = cut as f64 / SR as f64;
        assert!((sec - 0.6).abs() < 0.03, "cut = {sec} с");
        assert!(sec <= 0.7, "речь не должна попасть под обрезку: {sec}");
    }

    #[test]
    fn keeps_quiet_first_utterance() {
        // ≈ −36 dBFS RMS: тихая, но настоящая речь (регресс ловушки «Добрый день»).
        let audio = tone(1.0, 0.0224, 220.0);
        let cut = leading_silence_samples(&audio, SR, &TrimConfig::default());
        assert_eq!(cut, 0, "тихую речь не режем");
    }

    #[test]
    fn trims_room_noise_but_not_noise_plus_tone() {
        let noise: Vec<i16> = (0..SR as usize)
            .map(|i| ((i as f32 * 0.7).sin() * 8.0) as i16) // ≈ −75 dBFS
            .collect();
        let audio = concat(&[noise, tone(1.0, 0.4, 300.0)]);
        let cut = leading_silence_samples(&audio, SR, &TrimConfig::default());
        let sec = cut as f64 / SR as f64;
        assert!(
            (sec - 0.6).abs() < 0.03,
            "шум срезан, речь оставлена: {sec}"
        );
    }

    #[test]
    fn single_short_blip_is_not_speech() {
        // Один выброс 40 мс в тишине не должен останавливать обрезку.
        let mut blip = silence(0.04);
        for s in blip.iter_mut() {
            *s = 8000;
        }
        let audio = concat(&[silence(1.0), blip, silence(1.0), tone(1.0, 0.5, 440.0)]);
        let cut = leading_silence_samples(&audio, SR, &TrimConfig::default());
        let sec = cut as f64 / SR as f64;
        // Речь с 2.0 с; выброс на 1.0–1.04 с не должен её «притянуть».
        assert!((sec - 1.6).abs() < 0.1, "обрезка мимо выброса: {sec}");
    }

    #[test]
    fn no_silence_means_zero_and_all_silence_means_len() {
        let speech = tone(1.0, 0.5, 440.0);
        assert_eq!(
            leading_silence_samples(&speech, SR, &TrimConfig::default()),
            0
        );
        let quiet = silence(1.0);
        assert_eq!(
            leading_silence_samples(&quiet, SR, &TrimConfig::default()),
            quiet.len()
        );
        assert_eq!(leading_silence_samples(&[], SR, &TrimConfig::default()), 0);
    }

    #[test]
    fn speech_stats_counts_ratio_and_seconds() {
        let audio = concat(&[tone(0.5, 0.5, 440.0), silence(0.5)]);
        let st = speech_stats(&audio, SR, 0.0, 1.0, -50.0, 20.0);
        assert!((st.ratio - 0.5).abs() < 0.06, "ratio = {}", st.ratio);
        assert!(
            (st.speech_sec - 0.5).abs() < 0.06,
            "sec = {}",
            st.speech_sec
        );
        let quiet = speech_stats(&silence(1.0), SR, 0.0, 1.0, -50.0, 20.0);
        assert_eq!(quiet.ratio, 0.0);
        assert_eq!(quiet.speech_sec, 0.0);
        let none = speech_stats(&audio, SR, 5.0, 6.0, -50.0, 20.0);
        assert_eq!(none.total_sec, 0.0);
    }

    #[test]
    fn frame_db_of_digital_silence_is_very_low() {
        assert!(frame_db(&silence(0.02), 0, 320) < -100.0);
    }
}
