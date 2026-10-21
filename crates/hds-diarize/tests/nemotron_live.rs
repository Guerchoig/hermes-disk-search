//! W3: живой тест Nemotron-3 на `frag60` — эталон спайка (2 спикера, 5 сегментов).
//!
//! F13 (ревью L4): пути задаются **только** переменными окружения — никаких
//! зашитых `D:\...` (на другой машине такой тест «зелёный», ничего не проверив).
//! * переменные не заданы ⇒ `[skip]` (CI без модели);
//! * переменные заданы, а файлов нет ⇒ падение с понятным текстом (ошибка
//!   конфигурации теста, а не «тихий успех»).
//!
//! Запуск:
//! ```text
//! set HDS_DIAR_MODEL=<путь к nemotron3_diar_v3.onnx>
//! set HDS_TEST_DIAR_WAV=<путь к frag60.wav>
//! cargo test -p hds-diarize --test nemotron_live -- --ignored --nocapture
//! ```

use std::collections::BTreeSet;
use std::path::Path;

use hds_diarize::{Diarizer, NemotronConfig};

/// Значение переменной, если она задана и не пуста.
fn env_path(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[test]
#[ignore = "требует модель Nemotron-3 (HDS_DIAR_MODEL) и WAV (HDS_TEST_DIAR_WAV)"]
fn frag60_two_speakers_five_segments() {
    let (Some(model), Some(wav)) = (env_path("HDS_DIAR_MODEL"), env_path("HDS_TEST_DIAR_WAV"))
    else {
        eprintln!(
            "[skip] задайте HDS_DIAR_MODEL (путь к nemotron3_diar_v3.onnx) и HDS_TEST_DIAR_WAV \
             (путь к frag60.wav), чтобы прогнать живой тест"
        );
        return;
    };
    assert!(
        Path::new(&model).is_file(),
        "HDS_DIAR_MODEL={model}: файла модели нет"
    );
    assert!(
        Path::new(&wav).is_file(),
        "HDS_TEST_DIAR_WAV={wav}: файла WAV нет"
    );

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
