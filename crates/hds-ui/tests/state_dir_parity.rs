//! M2-F6 (T0 M4): `system_state_dir` продублирован в `hds-core::config` и
//! `hds-llama::host` (у `hds-llama` нет зависимости на `hds-core`, поэтому
//! функция скопирована). Тест ловит расхождение дубликатов — `hds-ui` зависит
//! от обоих крейтов, удобное место для сверки.

/// Каталог состояния сервера (ProgramData / Application Support) должен
/// совпадать у обоих дубликатов.
#[test]
fn system_state_dir_matches_between_crates() {
    assert_eq!(
        hds_core::config::system_state_dir(),
        hds_llama::host::system_state_dir(),
        "дубликат system_state_dir в hds-llama разошёлся с hds-core"
    );
}
