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
    // Находка A4 шага 2: Qwen3.5-9B — гибридная модель (`qwen35.ssm.*`),
    // полное внимание только в каждом 4-м слое.
    assert_eq!(
        meta.full_attention_interval,
        Some(4),
        "у Qwen3.5-9B полный KV держат 8 слоёв из 32"
    );
    assert_eq!(meta.kv_layer_count(), 8);
    assert_eq!(meta.kv_layer_count_within(8), 2, "при офлоаде 8 слоёв KV держат 2");
    assert_eq!(meta.kv_layer_count_within(4), 1);

    let f16 = kv_cache_mib(&meta, 32768, 1, KvBits::F16);
    let q8 = kv_cache_mib(&meta, 32768, 1, KvBits::Q8_0);
    println!(
        "Qwen3.5-9B (гибрид, KV только в 8 из 32 слоёв): при n_ctx = 32768 — \
         KV f16 {f16:.0} МиБ, q8_0 {q8:.0} МиБ"
    );
    // 32768 × 8 слоёв × 4 головы × (256 + 256) × 2 байта = 1024 МиБ
    assert!((f16 - 1024.0).abs() < 4.0, "KV f16 = {f16} МиБ");
    assert!((q8 - 544.0).abs() < 4.0, "KV q8_0 = {q8} МиБ");

    // Бюджет: 7,4 ГБ модель + 1 ГБ KV (f16) + 5 % оверхеда — на 12 ГБ влезает
    // с резервом 1 ГБ (это и было решение по `llm.chat.n_ctx`: 32768 оставляем)
    let need = estimate_need_mib(&meta, file_mib(&p), 32768, 1, KvBits::F16);
    let empty = check_fit(Some(12_288), need, 1024);
    assert!(
        empty.is_ok(),
        "на пустой 12 ГБ чат должен влезать при n_ctx 32768: {need} МиБ — {}",
        empty.message()
    );
    // а на карте, занятой штатными ролями (~8,5 ГБ), — уже нет
    let busy = check_fit(Some(3_562), need, 1024);
    assert!(!busy.is_ok(), "при 3,5 ГБ свободных чат не влезает");
    let msg = busy.message();
    assert!(
        msg.contains(&need.to_string()) && msg.contains("3562"),
        "в отчёте должны быть точные цифры «нужно/доступно»: {msg}"
    );
    println!("{msg}");
}
