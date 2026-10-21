//! F7 (ревью M4): в одном процессе CLIP и диаризация используют **одно**
//! ONNX-окружение.
//!
//! `ort 2.0.0-rc.15` допускает один `Environment` на процесс. Раньше окружение
//! создавалось дважды (`hds_clip::env` и `hds_diarize::ort_env`), и коллизии не
//! было только потому, что `hds` и `llm_host` — разные бинари; но `hds` уже
//! линкует оба крейта, так что прогон диаризации из UI упал бы с «only one
//! environment is allowed per process».
//!
//! Тест держит оба API в одном процессе: второй вызов обязан вернуть то же
//! окружение (общий `OnceLock` в `hds-ort`).

#[test]
fn one_environment_per_process() {
    let clip = hds_clip::env::environment();
    let diar = hds_diarize::ort_env();
    assert!(
        clip.is_ok(),
        "CLIP не смог инициализировать ORT: {:?}",
        clip.err()
    );
    assert!(
        diar.is_ok(),
        "диаризация не смогла инициализировать ORT: {:?}",
        diar.err()
    );
    let (clip, diar) = (clip.unwrap(), diar.unwrap());
    // Адреса сравниваем через указатели: типы приходят из разных реэкспортов
    // (`ort` и `parakeet_rs::ort`), но это один и тот же `Environment`.
    let (a, b) = (clip as *const _ as usize, diar as *const _ as usize);
    assert_eq!(
        a, b,
        "CLIP и диаризация получили разные окружения — обёртки должны ходить в hds-ort"
    );
}
