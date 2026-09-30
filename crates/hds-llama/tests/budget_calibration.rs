//! Калибровка оценки VRAM против фактических замеров (A4, `PLAN_W2_LLM_HOST.md` §A4).
//!
//! Проверяет оценку на моделях общего рантайма машины (если их нет — тест
//! пропускается, как 50-файловый паритет в `hds-index`):
//! * **bge-m3** — энкодер (`*.attention.causal = false`), KV-кэша нет, поэтому
//!   оценка = файл + 5 %; замер A1 того же файла дал рост VRAM **+636 МиБ** при
//!   файле 605 МиБ (`tools/parity/out/w2_a1_device.json`) — совпадение ±1 %;
//! * **Qwen3.5-9B-Q6_K** — причинная модель: фиксируем входные данные оценки
//!   (слои/головы KV/размер головы) и печатаем KV для f16/q8_0. Пункт честности:
//!   величину KV у чат-модели обязан подтвердить замер загрузки (A4 шаг 2),
//!   так как у гибридных моделей часть слоёв может не держать KV.

use std::path::PathBuf;

use hds_llama::budget::{check_fit, estimate_need_mib, kv_cache_mib};
use hds_llama::gguf::{read_meta, KvBits};
use hds_llama::runtime::{models_dir, runtime_dir};

/// Путь к модели роли в общем рантайме (None — нет файла).
fn model(role: &str, file: &str) -> Option<PathBuf> {
    let root = runtime_dir().ok()?;
    let p = models_dir(&root, Some(role)).join(file);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

fn file_mib(p: &PathBuf) -> u64 {
    std::fs::metadata(p).map(|m| m.len() / (1024 * 1024)).unwrap_or(0)
}

#[test]
fn embedding_model_estimate_matches_a1_measurement() {
    let Some(p) = model("embedding", "bge-m3-Q8_0.gguf") else {
        eprintln!("пропуск: нет bge-m3 в общем рантайме");
        return;
    };
    let meta = read_meta(&p).expect("метаданные bge-m3");
    assert_eq!(meta.architecture, "bert");
    assert!(
        !meta.has_kv_cache(),
        "bge-m3 — энкодер (attention.causal = false), KV-кэша нет"
    );
    assert_eq!(kv_cache_mib(&meta, 8192, 1, KvBits::F16), 0.0);

    let file = file_mib(&p);
    let need = estimate_need_mib(&meta, file, 8192, 1, KvBits::F16);
    // замер A1: файл 605 МиБ → рост VRAM +636 МиБ
    let measured = 636i64;
    let diff = (need as i64 - measured).abs();
    println!("bge-m3: файл {file} МиБ → оценка {need} МиБ против замера {measured} МиБ (±{diff})");
    assert!(
        diff <= 40,
        "оценка {need} МиБ расходится с замером {measured} МиБ сильнее допуска (40 МиБ)"
    );
}

#[test]
fn chat_model_metadata_and_kv_estimate() {
    let Some(p) = model("chat", "Qwen3.5-9B-Q6_K.gguf") else {
        eprintln!("пропуск: нет чат-модели в общем рантайме");
        return;
    };
    let meta = read_meta(&p).expect("метаданные чат-модели");
    assert_eq!(meta.architecture, "qwen35");
    assert!(meta.has_kv_cache(), "чат-модель причинная — KV-кэш есть");
    assert_eq!(meta.block_count, 32);
    assert_eq!(meta.head_count_kv, 4, "GQA: 4 головы KV");
    assert_eq!(meta.head_dim(), 256, "attention.key_length = 256");

    let f16 = kv_cache_mib(&meta, 32768, 1, KvBits::F16);
    let q8 = kv_cache_mib(&meta, 32768, 1, KvBits::Q8_0);
    println!(
        "Qwen3.5-9B: KV при n_ctx = 32768 — f16 {f16:.0} МиБ, q8_0 {q8:.0} МиБ \
         (величину подтвердить замером загрузки — A4 шаг 2)"
    );
    // 32768 × 32 слоя × 4 головы × (256 + 256) × 2 байта = 4 ГиБ
    assert!((f16 - 4096.0).abs() < 8.0, "KV f16 = {f16} МиБ");
    assert!((q8 - 2176.0).abs() < 8.0, "KV q8_0 = {q8} МиБ");

    // Бюджет: 7,4 ГБ модель + 4 ГБ KV (f16) + 5 % оверхеда не влезает в 12 ГБ
    let need = estimate_need_mib(&meta, file_mib(&p), 32768, 1, KvBits::F16);
    let fit = check_fit(Some(12288), need, 1024);
    assert!(!fit.is_ok(), "на 12 ГБ с резервом 1 ГБ чат не влезает: {need} МиБ");
    let msg = fit.message();
    assert!(
        msg.contains(&need.to_string()) && msg.contains("12288"),
        "в отчёте должны быть точные цифры «нужно/доступно»: {msg}"
    );
    println!("{msg}");
}
