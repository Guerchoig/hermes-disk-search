//! W3: живой тест Nemotron-3 на `frag60` — эталон спайка (2 спикера, 5 сегментов).
//!
//! Требует модель и WAV; по умолчанию берутся артефакты спайка на этой машине,
//! иначе — переменные `HDS_DIAR_MODEL` / `HDS_TEST_DIAR_WAV`. Если файлов нет,
//! тест пропускается (CI без модели).
//!
//! Запуск: `cargo test -p hds-diarize --test nemotron_live -- --ignored --nocapture`

use std::collections::BTreeSet;
use std::path::Path;

use hds_diarize::{Diarizer, NemotronConfig};

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[test]
#[ignore = "требует модель Nemotron-3 и frag60.wav"]
fn frag60_two_speakers_five_segments() {
    let model = env_or(
        "HDS_DIAR_MODEL",
        r"D:\diar-spike-models\nemotron3_diar_v3.onnx",
    );
    let wav = env_or("HDS_TEST_DIAR_WAV", r"D:\diar-spike-data\frag60.wav");
    if !Path::new(&model).is_file() || !Path::new(&wav).is_file() {
        eprintln!("[skip] нет модели ({model}) или WAV ({wav})");
        return;
    }

    let mut diar = Diarizer::load(&NemotronConfig::new(&model)).expect("загрузка модели");
    let turns = diar.run_wav(Path::new(&wav)).expect("диаризация");

    let speakers: BTreeSet<u32> = turns.iter().map(|t| t.speaker).collect();
    println!(
        "frag60: device={} turns={} speakers={speakers:?}",
        diar.device().as_str(),
        turns.len()
    );
    assert_eq!(speakers.len(), 2, "эталон спайка: 2 спикера");
    assert_eq!(turns.len(), 5, "эталон спайка: 5 сегментов");
    // Интро 0–35 с — тишина: первый turn не раньше 35 с (спайк: 35.95 с).
    assert!(
        turns[0].start > 30.0,
        "первый turn на {:.2} с",
        turns[0].start
    );
}
