//! Сшивка сегментов ASR с turns диаризации и форматирование `.md`/`.srt`.
//!
//! Формат — ровно как отдаёт движок (спайк `T0_1_DIARIZATION_FORMAT.md`):
//! `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]`, литерал `UNASSIGNED`, пустая строка
//! между репликами, порядок по времени.
//!
//! Правила сшивки (`PLAN_DIARIZATION_NEMOTRON.md` §3.1/§3.4):
//! * спикер реплики — по **середине** сегмента (устойчиво к неточным границам ASR);
//! * середина вне turns ⇒ `None` (в `.md` — `UNASSIGNED`);
//! * если длинный сегмент (≥ [`SPLIT_MIN_SEC`]) пересекает границу спикера, он
//!   делится по границе: иначе один длинный сегмент «съел» бы смену голоса;
//!   текст делится по словам пропорционально длительности частей;
//! * короткие сегменты (< [`SPLIT_MIN_SEC`]) не делятся — смена спикера внутри
//!   пары секунд чаще артефакт разметки, чем реальная реплика.

use crate::Turn;

/// Минимальная длительность сегмента, который имеет смысл делить по границе turn'а.
pub const SPLIT_MIN_SEC: f64 = 3.0;
/// Минимальная длительность части при делении (иначе часть присоединяется к соседней).
pub const MIN_PART_SEC: f64 = 1.0;

/// Сегмент ASR в абсолютной шкале (нейтральный тип: `hds-diarize` не знает
/// про `hds-llama::WhisperSegment`, чтобы не тянуть зависимость назад).
#[derive(Debug, Clone, PartialEq)]
pub struct AsrSpan {
    /// Начало, секунды от начала исходного файла.
    pub start: f64,
    /// Конец, секунды.
    pub end: f64,
    /// Текст реплики.
    pub text: String,
}

impl AsrSpan {
    /// Сегмент с текстом.
    pub fn new(start: f64, end: f64, text: impl Into<String>) -> Self {
        AsrSpan {
            start,
            end,
            text: text.into(),
        }
    }
}

/// Готовая реплика для `.md`/`.srt`: интервал + спикер (`None` — `UNASSIGNED`).
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Начало, секунды.
    pub start: f64,
    /// Конец, секунды.
    pub end: f64,
    /// `Some(id)` — `SPEAKER_NN`; `None` — `UNASSIGNED`.
    pub speaker: Option<u32>,
    /// Текст.
    pub text: String,
}

/// Спикер в точке `t` (окно ±1 мс; побеждает turn с большим пересечением,
/// при равенстве — меньший `speaker`, чтобы результат был детерминированным).
fn speaker_at(turns: &[Turn], t: f64) -> Option<u32> {
    const EPS: f64 = 1e-3;
    let (a, b) = (t - EPS, t + EPS);
    let mut best: Option<(f64, u32)> = None;
    for turn in turns {
        let ov = turn.end.min(b) - turn.start.max(a);
        if ov <= 0.0 {
            continue;
        }
        match best {
            Some((best_ov, best_sp))
                if ov < best_ov || (ov == best_ov && turn.speaker >= best_sp) => {}
            _ => best = Some((ov, turn.speaker)),
        }
    }
    best.map(|(_, sp)| sp)
}

/// Разделить текст по словам пропорционально длительностям частей.
///
/// Возвращает по строке на часть; сумма слов исходного текста сохраняется.
/// Часть может получить 0 слов, если слов меньше, чем частей (например, одно
/// «Да-а-а.» на четыре реплики) — такие блоки вызывающий отбрасывает.
fn split_text(text: &str, durations: &[f64]) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let n = words.len();
    if durations.len() <= 1 || n == 0 {
        return vec![words.join(" ")];
    }
    let total: f64 = durations.iter().sum();
    if total <= 0.0 {
        let mut out = vec![String::new(); durations.len()];
        out[0] = words.join(" ");
        return out;
    }
    let mut counts = Vec::with_capacity(durations.len());
    let mut used = 0usize;
    for (k, d) in durations.iter().enumerate() {
        if k + 1 == durations.len() {
            counts.push(n - used);
        } else {
            let want = ((d / total) * n as f64).round() as usize;
            // B1: слова могли закончиться до последней части — тогда `room == 0`
            // и часть получает 0 слов (её блок отбрасывается ниже). `clamp(1, 0)`
            // здесь паниковал (`min > max`), `saturating_sub` скрывал бы инвариант.
            let room = n - used; // инвариант: used <= n
            let want = if room == 0 { 0 } else { want.max(1).min(room) };
            counts.push(want);
            used += want;
        }
    }
    let mut out = Vec::with_capacity(counts.len());
    let mut at = 0usize;
    for c in counts {
        let end = (at + c).min(n);
        out.push(words[at..end].join(" "));
        at = end;
    }
    out
}

/// `HH:MM:SS` (без миллисекунд) — формат заголовка `.md`.
pub fn hhmmss(t: f64) -> String {
    let secs = t.max(0.0).floor() as u64;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// `HH:MM:SS,mmm` — формат SRT.
pub fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

/// Метка реплики (`SPEAKER_NN`/`UNASSIGNED`).
fn label(speaker: Option<u32>, unassigned: &str) -> String {
    match speaker {
        Some(id) => format!("SPEAKER_{id:02}"),
        None => unassigned.to_string(),
    }
}

/// `.md` в формате движка: `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]` + текст,
/// пустая строка между репликами.
pub fn render_md(blocks: &[Block], unassigned: &str) -> String {
    let mut out = String::new();
    for b in blocks {
        out.push_str(&format!(
            "### {} [{} - {}]\n{}\n\n",
            label(b.speaker, unassigned),
            hhmmss(b.start),
            hhmmss(b.end),
            b.text.trim()
        ));
    }
    out
}

/// `.srt`: нумерация, `HH:MM:SS,mmm --> …`, метка спикера префиксом текста.
pub fn render_srt(blocks: &[Block], unassigned: &str) -> String {
    let mut out = String::new();
    for (i, b) in blocks.iter().enumerate() {
        let end = if b.end > b.start {
            b.end
        } else {
            b.start + 0.001
        };
        out.push_str(&format!(
            "{}\n{} --> {}\n{}: {}\n\n",
            i + 1,
            srt_time(b.start),
            srt_time(end),
            label(b.speaker, unassigned),
            b.text.trim()
        ));
    }
    out
}

/// Сшить сегменты ASR с turns: середина сегмента, деление длинных сегментов.
///
/// Возвращает блоки в порядке времени; пустые по тексту сегменты отбрасываются,
/// соседние блоки одного спикера склеиваются.
pub fn merge_segments_with_turns(segments: &[AsrSpan], turns: &[Turn]) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    for seg in segments {
        if seg.end <= seg.start || seg.text.trim().is_empty() {
            continue;
        }
        let dur = seg.end - seg.start;
        // Границы: начало/конец сегмента + границы turns внутри него.
        let mut cuts: Vec<f64> = vec![seg.start, seg.end];
        if dur >= SPLIT_MIN_SEC {
            for t in turns {
                for edge in [t.start, t.end] {
                    if edge > seg.start + MIN_PART_SEC && edge < seg.end - MIN_PART_SEC {
                        cuts.push(edge);
                    }
                }
            }
        }
        cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);

        let mut parts: Vec<(f64, f64, Option<u32>)> = Vec::new();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b - a <= 1e-6 {
                continue;
            }
            parts.push((a, b, speaker_at(turns, (a + b) / 2.0)));
        }
        if parts.is_empty() {
            continue;
        }
        // Части короче MIN_PART_SEC присоединяем к соседу (иначе текст режется на обрывки).
        let mut grouped: Vec<(f64, f64, Option<u32>)> = Vec::with_capacity(parts.len());
        for (a, b, sp) in parts {
            match grouped.last_mut() {
                Some(last) if b - a < MIN_PART_SEC => last.1 = b,
                Some(last) if last.2 == sp => last.1 = b,
                _ => grouped.push((a, b, sp)),
            }
        }

        if grouped.len() == 1 {
            blocks.push(Block {
                start: seg.start,
                end: seg.end,
                speaker: grouped[0].2,
                text: seg.text.trim().to_string(),
            });
            continue;
        }
        let durs: Vec<f64> = grouped.iter().map(|(a, b, _)| b - a).collect();
        let texts = split_text(&seg.text, &durs);
        for ((a, b, sp), text) in grouped.into_iter().zip(texts) {
            if text.is_empty() {
                continue;
            }
            blocks.push(Block {
                start: a,
                end: b,
                speaker: sp,
                text,
            });
        }
    }

    blocks.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    // Склеиваем соседние блоки одного спикера (после деления могли получиться куски).
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for b in blocks {
        match out.last_mut() {
            Some(last) if last.speaker == b.speaker && (b.start - last.end).abs() < 1e-6 => {
                last.end = b.end;
                if !last.text.is_empty() && !b.text.is_empty() {
                    last.text.push(' ');
                }
                last.text.push_str(&b.text);
            }
            _ => out.push(b),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(start: f64, end: f64, speaker: u32) -> Turn {
        Turn {
            start,
            end,
            speaker,
        }
    }

    /// Спикер — по середине; середина вне turns ⇒ `UNASSIGNED`.
    #[test]
    fn assigns_by_midpoint_and_unassigned() {
        let turns = [turn(0.0, 10.0, 0), turn(10.0, 20.0, 1)];
        let segs = [
            AsrSpan::new(2.0, 4.0, "первый"),
            AsrSpan::new(11.0, 13.0, "второй"),
            AsrSpan::new(25.0, 27.0, "вне"),
        ];
        let blocks = merge_segments_with_turns(&segs, &turns);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].speaker, Some(0));
        assert_eq!(blocks[1].speaker, Some(1));
        assert_eq!(blocks[2].speaker, None, "вне turns — UNASSIGNED");
    }

    /// Длинный сегмент через границу 10 с делится, текст — по словам.
    #[test]
    fn splits_long_segment_at_speaker_boundary() {
        let turns = [turn(0.0, 10.0, 0), turn(10.0, 20.0, 1)];
        let segs = [AsrSpan::new(
            6.0,
            14.0,
            "раз два три четыре пять шесть семь восемь",
        )];
        let blocks = merge_segments_with_turns(&segs, &turns);
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(blocks[0].speaker, Some(0));
        assert_eq!(blocks[1].speaker, Some(1));
        assert!((blocks[0].end - 10.0).abs() < 1e-9);
        assert!((blocks[1].start - 10.0).abs() < 1e-9);
        let words = format!("{} {}", blocks[0].text, blocks[1].text);
        assert_eq!(
            words.split_whitespace().count(),
            8,
            "слова не потеряны: {words}"
        );
        assert!(!blocks[0].text.is_empty() && !blocks[1].text.is_empty());
    }

    /// Короткий сегмент (2 с) через ту же границу не делится — спикер по середине.
    #[test]
    fn short_segment_is_not_split() {
        let turns = [turn(0.0, 10.0, 0), turn(10.0, 20.0, 1)];
        let segs = [AsrSpan::new(9.5, 11.5, "да нет")];
        let blocks = merge_segments_with_turns(&segs, &turns);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].speaker, Some(1), "середина 10.5 → второй спикер");
        assert_eq!(blocks[0].text, "да нет");
    }

    /// Перекрывающиеся turns: побеждает больший перехлёст, при равенстве — меньший id.
    #[test]
    fn overlapping_turns_pick_largest_overlap() {
        let turns = [turn(0.0, 5.0, 2), turn(4.0, 6.0, 1)];
        // Точка 4.5: пересечение с обоими 0.001 — ничья ⇒ меньший speaker.
        let segs = [AsrSpan::new(4.4, 4.6, "x")];
        assert_eq!(merge_segments_with_turns(&segs, &turns)[0].speaker, Some(1));
        // Уводим середину в 5.5 — попадает только в turn 1.
        let segs = [AsrSpan::new(5.4, 5.6, "y")];
        assert_eq!(merge_segments_with_turns(&segs, &turns)[0].speaker, Some(1));
    }

    /// Пустые turns ⇒ всё `UNASSIGNED`; пустой текст ⇒ сегмент отброшен.
    #[test]
    fn empty_turns_and_empty_text() {
        let segs = [AsrSpan::new(1.0, 2.0, "текст"), AsrSpan::new(3.0, 3.0, "x")];
        let blocks = merge_segments_with_turns(&segs, &[]);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].speaker, None);
        let blocks = merge_segments_with_turns(&[AsrSpan::new(1.0, 2.0, "   ")], &[]);
        assert!(blocks.is_empty());
    }

    /// Формат `.md` — как у движка (T0.1 §2): заголовок, пустая строка, `UNASSIGNED`.
    #[test]
    fn render_md_matches_engine_format() {
        let blocks = [
            Block {
                start: 1.0,
                end: 11.9,
                speaker: Some(0),
                text: "Привет.".into(),
            },
            Block {
                start: 3725.0,
                end: 3726.0,
                speaker: None,
                text: "Хвост.".into(),
            },
        ];
        let md = render_md(&blocks, "UNASSIGNED");
        assert_eq!(
            md,
            "### SPEAKER_00 [00:00:01 - 00:00:11]\nПривет.\n\n\
             ### UNASSIGNED [01:02:05 - 01:02:06]\nХвост.\n\n"
        );
    }

    /// SRT: миллисекунды и метка спикера в тексте.
    #[test]
    fn render_srt_has_milliseconds() {
        let blocks = [Block {
            start: 1.0,
            end: 2.5,
            speaker: Some(3),
            text: "Текст".into(),
        }];
        let srt = render_srt(&blocks, "UNASSIGNED");
        assert_eq!(
            srt,
            "1\n00:00:01,000 --> 00:00:02,500\nSPEAKER_03: Текст\n\n"
        );
    }

    /// Таймкоды `.md` — целые секунды вниз (как движок).
    #[test]
    fn hhmmss_floor_and_srt_round() {
        assert_eq!(hhmmss(59.999), "00:00:59");
        assert_eq!(hhmmss(3600.5), "01:00:00");
        assert_eq!(srt_time(59.9996), "00:01:00,000");
    }

    /// B1 (`DIARIZATION_NEMOTRON_CODE_REVIEW.md`, приложение A): 5-секундный
    /// сегмент ASR с **двумя** словами; turns делят его на четыре части с
    /// разными спикерами. До фикса `split_text` паниковал (`clamp(1, 0)`):
    /// слова заканчивались раньше, чем части.
    #[test]
    fn few_words_many_speakers_does_not_panic() {
        let turns = [
            turn(0.0, 1.5, 0),
            turn(1.5, 2.5, 1),
            turn(2.5, 3.5, 2),
            turn(3.5, 5.0, 3),
        ];
        let segs = [AsrSpan::new(0.0, 5.0, "да нет")];
        let blocks = merge_segments_with_turns(&segs, &turns);
        let words: usize = blocks
            .iter()
            .map(|b| b.text.split_whitespace().count())
            .sum();
        assert_eq!(words, 2, "слова не потеряны");
    }

    /// F1 (B1): одно слово на три части — слово не теряется и не дублируется.
    #[test]
    fn one_word_three_parts_keeps_word_once() {
        let parts = split_text("Секундочку", &[1.0, 1.0, 1.0]);
        assert_eq!(parts.len(), 3);
        let words: usize = parts.iter().map(|p| p.split_whitespace().count()).sum();
        assert_eq!(words, 1, "слово ровно один раз: {parts:?}");
        assert_eq!(parts.iter().filter(|p| !p.is_empty()).count(), 1);
        assert_eq!(parts[0], "Секундочку");
    }

    /// F1 (B1): сумма слов сохраняется для любого соотношения «слов × частей»,
    /// паники нет (до фикса `2..6` слов × `3..6` частей падали).
    #[test]
    fn split_text_preserves_words() {
        for parts in 2..=6usize {
            for words in 1..=6usize {
                let text = (1..=words)
                    .map(|i| format!("w{i}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let durations = vec![1.0; parts];
                let out = split_text(&text, &durations);
                assert_eq!(out.len(), parts, "{words} слов / {parts} частей");
                let got: usize = out.iter().map(|p| p.split_whitespace().count()).sum();
                assert_eq!(got, words, "{words} слов / {parts} частей: {out:?}");
            }
        }
    }

    /// F1: нулевая суммарная длительность — текст не теряется (вся в первой части).
    #[test]
    fn split_text_zero_total_duration() {
        let out = split_text("да нет", &[0.0, 0.0, 0.0]);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], "да нет");
        assert!(out[1].is_empty() && out[2].is_empty());
    }
}
