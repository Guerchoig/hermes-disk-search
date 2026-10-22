//! Golden `.md`: формат `### SPEAKER_NN [HH:MM:SS - HH:MM:SS]` и `UNASSIGNED`
//! рендерит Rust (контракт §8), Python отдаёт структуру.

use hds_index::transcribe::{
    detect_speaker_slots, format_timestamp, render_markdown, segments_from_blocks,
};
use hds_transcribe::Block;

fn block(speaker: Option<&str>, start: f64, end: f64, text: &str) -> Block {
    Block {
        speaker: speaker.map(|s| s.to_string()),
        start: Some(start),
        end: Some(end),
        text: text.to_string(),
        ..Default::default()
    }
}

#[test]
fn timestamps_are_hh_mm_ss() {
    assert_eq!(format_timestamp(0.0), "00:00:00");
    assert_eq!(format_timestamp(1.4), "00:00:01");
    assert_eq!(format_timestamp(61.6), "00:01:02");
    assert_eq!(format_timestamp(3661.0), "01:01:01");
    assert_eq!(format_timestamp(f64::NAN), "00:00:00");
}

#[test]
fn golden_markdown_with_speakers() {
    let blocks = vec![
        block(Some("SPEAKER_00"), 1.0, 11.0, "Добрый день."),
        block(Some("SPEAKER_01"), 12.0, 15.4, "Здравствуйте!"),
    ];
    let md = render_markdown(&blocks, true);
    let expected = "### SPEAKER_00 [00:00:01 - 00:00:11]\nДобрый день.\n\n\
                    ### SPEAKER_01 [00:00:12 - 00:00:15]\nЗдравствуйте!\n";
    assert_eq!(md, expected);
    assert_eq!(
        detect_speaker_slots(&md),
        vec!["SPEAKER_00".to_string(), "SPEAKER_01".to_string()]
    );
}

#[test]
fn unassigned_literal_when_diarization_on() {
    let md = render_markdown(&[block(None, 0.0, 2.0, "неизвестно кто")], true);
    assert_eq!(md, "### UNASSIGNED [00:00:00 - 00:00:02]\nнеизвестно кто\n");
    assert_eq!(detect_speaker_slots(&md), vec!["UNASSIGNED".to_string()]);
}

#[test]
fn no_headers_when_diarization_off() {
    let md = render_markdown(&[block(None, 0.0, 2.0, "просто текст")], false);
    assert_eq!(md, "просто текст\n");
    assert!(detect_speaker_slots(&md).is_empty());
}

#[test]
fn empty_blocks_skipped() {
    let md = render_markdown(
        &[
            block(Some("SPEAKER_00"), 0.0, 1.0, "  "),
            block(Some("SPEAKER_00"), 1.0, 2.0, "ок"),
        ],
        true,
    );
    assert_eq!(md, "### SPEAKER_00 [00:00:01 - 00:00:02]\nок\n");
}

#[test]
fn blocks_become_segments() {
    let blocks = vec![block(Some("SPEAKER_00"), 0.5, 2.5, " раз ")];
    let segments = segments_from_blocks(&blocks);
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].text, "раз");
    assert_eq!(segments[0].t_start, Some(0.5));
    assert_eq!(segments[0].t_end, Some(2.5));
}
