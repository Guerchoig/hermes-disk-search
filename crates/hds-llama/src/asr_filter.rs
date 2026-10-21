//! Постфильтр сегментов ASR по энергии звука (W1, `PLAN_DIARIZATION_NEMOTRON.md` §3.4 п. 5).
//!
//! Зачем: фиксация языка убирает **английские** галлюцинации («Thank you.»,
//! «Obrigado.»), но whisper всё равно «слышит» речь в тишине и паузах — уже
//! по-русски («Продолжение следует…», «Субтитры создавал DimaTorzok»). Обрезка
//! ведущей тишины помогает только в начале файла, поэтому реплики, в окне которых
//! **нет энергии речи**, отбрасываются по факту, а не по языку (спайк `REPORT.md`
//! §11.4).
//!
//! Фильтр работает по **полному** WAV (вариант A): сегменты ASR приходят в шкале
//! среза, поэтому к ним прибавляется `offset_sec` — ровно один раз, здесь.
//! Порог консервативный: тихие, но настоящие реплики (в эталоне есть реплики с
//! речью −17…−19 дБ) остаются — проверяется `min_ratio` **и** `min_speech_sec`.

use hds_diarize::vad::speech_stats;
use hds_diarize::Wav;

use crate::whisper::WhisperSegment;

/// Порог «в сегменте нет речи».
#[derive(Debug, Clone, PartialEq)]
pub struct FilterConfig {
    /// Порог кадра, dBFS.
    pub threshold_db: f64,
    /// Длина кадра, мс.
    pub frame_ms: f64,
    /// Минимальная доля речи в окне, ниже которой сегмент подозрительный.
    pub min_ratio: f64,
    /// Минимальная суммарная речь, секунды (если меньше — сегмент отбрасывается).
    pub min_speech_sec: f64,
}

impl Default for FilterConfig {
    fn default() -> Self {
        FilterConfig {
            threshold_db: -50.0,
            frame_ms: 20.0,
            min_ratio: 0.05,
            min_speech_sec: 0.15,
        }
    }
}

/// Результат фильтрации: что оставили и что выбросили (для лога/статистики).
#[derive(Debug, Clone, PartialEq)]
pub struct FilterOutcome {
    /// Сегменты с речью (порядок сохранён).
    pub kept: Vec<WhisperSegment>,
    /// Отброшенные «реплики из тишины».
    pub dropped: Vec<WhisperSegment>,
}

impl FilterOutcome {
    /// Сколько сегментов отброшено.
    pub fn filtered_count(&self) -> usize {
        self.dropped.len()
    }
}

/// Оставить только сегменты, в окне которых есть речь.
///
/// `offset_sec` — сдвиг шкалы сегментов относительно полного WAV (trim-offset
/// входа ASR, §3.4 п. 4). Окно вне файла (или нулевой длины) считается «без речи».
pub fn drop_silent_segments(
    segments: &[WhisperSegment],
    wav: &Wav,
    offset_sec: f64,
    cfg: &FilterConfig,
) -> FilterOutcome {
    let mut kept = Vec::with_capacity(segments.len());
    let mut dropped = Vec::new();
    for s in segments {
        if !has_speech(s, wav, offset_sec, cfg) {
            dropped.push(s.clone());
        } else {
            kept.push(s.clone());
        }
    }
    FilterOutcome { kept, dropped }
}

/// Есть ли в окне сегмента речь (по энергии полного WAV).
pub fn has_speech(seg: &WhisperSegment, wav: &Wav, offset_sec: f64, cfg: &FilterConfig) -> bool {
    let t0 = seg.t_start + offset_sec;
    let t1 = seg.t_end + offset_sec;
    if t1 <= t0 {
        return false;
    }
    let st = speech_stats(
        &wav.samples,
        wav.sample_rate,
        t0,
        t1,
        cfg.threshold_db,
        cfg.frame_ms,
    );
    if st.total_sec <= 0.0 {
        return false;
    }
    // План §3.4 п. 5: отбрасываем, если доля речи мала **и** суммарной речи мало.
    // Значит, для сохранения достаточно одного условия — иначе обрывочные, но
    // настоящие реплики (8 % речи за 1.6 с, живой прогон 21.10) терялись бы.
    st.ratio >= cfg.min_ratio || st.speech_sec >= cfg.min_speech_sec
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 16_000;

    fn tone(sec: f64, amp: f64) -> Vec<i16> {
        let n = (sec * SR as f64) as usize;
        (0..n)
            .map(|i| {
                ((2.0 * std::f64::consts::PI * 220.0 * i as f64 / SR as f64).sin() * amp * 32_767.0)
                    as i16
            })
            .collect()
    }

    fn wav(parts: &[Vec<i16>]) -> Wav {
        Wav {
            sample_rate: SR,
            channels: 1,
            samples: parts.iter().flatten().copied().collect(),
        }
    }

    fn seg(text: &str, t_start: f64, t_end: f64) -> WhisperSegment {
        WhisperSegment {
            text: text.to_string(),
            t_start,
            t_end,
        }
    }

    /// «Продолжение следует…» в паузе — отбрасывается; речь — остаётся.
    #[test]
    fn drops_hallucinations_in_silence_only() {
        let w = wav(&[vec![0i16; 5 * SR as usize], tone(5.0, 0.5)]);
        let segments = vec![
            seg("Продолжение следует...", 0.2, 2.0),
            seg("Добрый день", 5.2, 7.0),
        ];
        let out = drop_silent_segments(&segments, &w, 0.0, &FilterConfig::default());
        assert_eq!(out.filtered_count(), 1, "одна реплика из тишины");
        assert_eq!(out.kept.len(), 1);
        assert_eq!(out.kept[0].text, "Добрый день");
        assert_eq!(out.dropped[0].text, "Продолжение следует...");
    }

    /// Шкала сегментов — от среза: `offset_sec` применяется один раз.
    #[test]
    fn offset_maps_trimmed_timeline_to_full_wav() {
        let w = wav(&[vec![0i16; 5 * SR as usize], tone(5.0, 0.5)]);
        // Тот же сегмент в шкале среза (0.2…2.0) + offset 5 с ⇒ 5.2…7.0 — речь.
        let segments = vec![seg("Добрый день", 0.2, 2.0)];
        let without = drop_silent_segments(&segments, &w, 0.0, &FilterConfig::default());
        assert_eq!(without.filtered_count(), 1, "без offset это тишина");
        let with = drop_silent_segments(&segments, &w, 5.0, &FilterConfig::default());
        assert_eq!(with.filtered_count(), 0, "с offset — речь");
    }

    /// Тихая, но настоящая речь (−30 dBFS RMS) не должна отбрасываться.
    #[test]
    fn keeps_quiet_speech() {
        let w = wav(&[tone(2.0, 0.06)]); // ≈ −31 dBFS
        let segments = vec![seg("тихо, но речь", 0.2, 1.8)];
        let out = drop_silent_segments(&segments, &w, 0.0, &FilterConfig::default());
        assert_eq!(out.filtered_count(), 0);
    }

    /// Пограничный случай живого прогона: речь только в 140 мс из 1.6 с
    /// (доля ≈ 9 % ≥ 5 %) — сегмент остаётся, хотя `speech_sec` ниже 150 мс.
    #[test]
    fn short_faint_fragment_survives_by_ratio() {
        let mut samples = vec![0i16; (1.6 * SR as f64) as usize];
        let start = (0.9 * SR as f64) as usize;
        let tail = tone(0.14, 0.06);
        samples[start..start + tail.len()].copy_from_slice(&tail);
        let w = Wav {
            sample_rate: SR,
            channels: 1,
            samples,
        };
        let out = drop_silent_segments(
            &[seg("планировать их", 0.0, 1.6)],
            &w,
            0.0,
            &FilterConfig::default(),
        );
        assert_eq!(out.filtered_count(), 0, "обрывок настоящей речи сохраняем");
    }

    /// Пустое/вырожденное окно — «без речи».
    #[test]
    fn degenerate_windows_are_dropped() {
        let w = wav(&[tone(2.0, 0.5)]);
        for (a, b) in [(1.0, 1.0), (3.0, 4.0), (1.5, 0.5)] {
            assert!(!has_speech(
                &seg("x", a, b),
                &w,
                0.0,
                &FilterConfig::default()
            ));
        }
    }
}
