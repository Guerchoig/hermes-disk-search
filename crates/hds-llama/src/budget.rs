//! Бюджет VRAM (A4, `PLAN_W2_LLM_HOST.md` §A4): оценка потребности модели
//! «модель + KV» и сверка с реальной свободной памятью.
//!
//! Источник истины по свободной VRAM — NVML ([`crate::vram`]); `memory_free`
//! движка для этого негоден (R29: расхождение до +7,7 ГБ, см. `W2_REPORT` §1.3).
//!
//! KV-кэш считаем по метаданным GGUF (число слоёв, голов KV, размер головы):
//! `n_ctx × n_parallel × layers × (head_count_kv × head_dim) × 2 (K и V) × bytes/element`.
//! Калибровка против замеров: Qwen3.5-9B Q6_K при `n_ctx = 32768` и `q8_0` давал
//! ≈0,55 ГБ KV (W0), bge-m3 (24 слоя, 8 голов KV, `n_ctx = 8192`) — ≈0,03 ГБ,
//! что совпадает с наблюдённым ростом VRAM (+636 МиБ в A1, `W2_REPORT` §1.2).

use crate::gguf::{GgufMeta, KvBits};

/// Оценка KV-кэша в МиБ для `n_ctx × n_parallel`.
///
/// Энкодерные модели (`*.attention.causal = false`, как у bge-m3) KV-кэш не
/// держат — подтверждено замером A1 (`tools/parity/out/w2_a1_device.json`:
/// файл 605 МиБ, рост VRAM +636 МиБ, то есть весь прирост — веса). Для
/// причинных моделей считаем классическую формулу llama.cpp:
/// `n_ctx × n_parallel × kv_layers × kv_heads × (key_dim + value_dim) × bytes/element`,
/// где `kv_layers` — **не** все слои: у гибридных моделей полное внимание только
/// в каждом `full_attention_interval`-м слое (Qwen3.5-9B: 8 из 32 — находка A4
/// шага 2, подтверждена дампом метаданных `bin/gguf_dump`).
pub fn kv_cache_mib(meta: &GgufMeta, n_ctx: i64, n_parallel: i64, bits: KvBits) -> f64 {
    let kv_layers = meta.kv_layer_count() as f64;
    if kv_layers == 0.0 {
        return 0.0;
    }
    let head = meta.head_dim() as f64;
    let k_dim = meta.key_length.map(|k| k as f64).unwrap_or(head);
    let v_dim = meta.value_length.map(|v| v as f64).unwrap_or(head);
    let per_token = meta.head_count_kv as f64 * (k_dim + v_dim) * bits.bytes_per_element();
    let elements = (n_ctx.max(0) * n_parallel.max(1)) as f64 * kv_layers * per_token;
    elements / (1024.0 * 1024.0)
}

/// Оценка полной потребности инстанса: веса модели + KV + буферы (≈5 % веса).
///
/// Не заменяет замер после загрузки (A1 показал расхождение в единицы процентов),
/// но достаточна, чтобы **до** загрузки ответить «влезает / не влезает» и честно
/// сообщить цифры (авто-деградации нет — решение заказчика 29.09.2026).
pub fn estimate_need_mib(
    meta: &GgufMeta,
    model_file_mib: u64,
    n_ctx: i64,
    n_parallel: i64,
    bits: KvBits,
) -> u64 {
    let overhead = (model_file_mib as f64 * 0.05).ceil();
    (model_file_mib as f64 + kv_cache_mib(meta, n_ctx, n_parallel, bits) + overhead).ceil() as u64
}

/// Решение «влезает ли» — вход для отчёта о нехватке VRAM (без деградации).
#[derive(Debug, Clone, PartialEq)]
pub enum Fit {
    /// Свободной VRAM достаточно (с учётом резерва).
    Fits {
        free_mib: u64,
        need_mib: u64,
        reserve_mib: u64,
    },
    /// Не хватает: сколько не хватает и что предлагается вытеснить.
    NotEnough {
        free_mib: u64,
        need_mib: u64,
        reserve_mib: u64,
        short_mib: u64,
    },
}

impl Fit {
    pub fn is_ok(&self) -> bool {
        matches!(self, Fit::Fits { .. })
    }

    /// Строка для лога/`hdsw check`/UI (точные цифры «нужно/доступно»).
    pub fn message(&self) -> String {
        match self {
            Fit::Fits {
                free_mib,
                need_mib,
                reserve_mib,
            } => format!(
                "влезает: нужно {need_mib} МиБ, свободно {free_mib} МиБ, резерв {reserve_mib} МиБ"
            ),
            Fit::NotEnough {
                free_mib,
                need_mib,
                reserve_mib,
                short_mib,
            } => format!(
                "не хватает VRAM: нужно {need_mib} МиБ, свободно {free_mib} МиБ \
                 (резерв {reserve_mib} МиБ) — недостаёт {short_mib} МиБ; смена модели/контекста \
                 за пользователем (авто-деградации нет)"
            ),
        }
    }
}

/// Проверить, влезает ли потребность в текущую свободную память.
pub fn check_fit(free_mib: Option<u64>, need_mib: u64, reserve_mib: u64) -> Fit {
    match free_mib {
        Some(free) if free >= need_mib + reserve_mib => Fit::Fits {
            free_mib: free,
            need_mib,
            reserve_mib,
        },
        Some(free) => Fit::NotEnough {
            free_mib: free,
            need_mib,
            reserve_mib,
            short_mib: (need_mib + reserve_mib).saturating_sub(free),
        },
        // без NVML считаем «не знаем» → не блокируем загрузку, но сообщаем
        None => Fit::NotEnough {
            free_mib: 0,
            need_mib,
            reserve_mib,
            short_mib: need_mib,
        },
    }
}

/// Байт на элемент compute-буфера движка — калибровка по живым замерам W2
/// (`W2_REPORT.md` §10.2a): Qwen3.5-9B (4096 × 32 слоя) при `n_batch` 2048 давал
/// ≈1972 МиБ, при 512 — ≈469 МиБ, то есть ≈7,5 байта на элемент (K/V-буферы f32 плюс
/// служебные). Значение грубое, но именно его не хватало бюджету: он считал только
/// «модель + KV», а `n_ubatch = 8192` у embedding — это гигабайты.
pub const COMPUTE_BYTES_PER_ELEMENT: f64 = 7.5;

/// Оценка compute-буфера движка, МиБ: `n_ubatch × embedding_length × block_count × C`.
///
/// Буфер выделяется при загрузке и не зависит от KV-типа. Для embedding с
/// `n_ubatch = 8192` (bge-m3: 1024 × 24) это ≈1,4 ГиБ, тогда как прежний бюджет
/// обещал «нужно 636 МиБ» — отсюда «загадочная» занятая VRAM при индексации.
pub fn compute_buffer_mib(meta: &GgufMeta, n_ubatch: i64) -> f64 {
    let embd = meta.embedding_length as f64;
    let layers = meta.block_count as f64;
    let tokens = n_ubatch.max(1) as f64;
    tokens * embd * layers * COMPUTE_BYTES_PER_ELEMENT / (1024.0 * 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(embd: u32, layers: u32) -> GgufMeta {
        GgufMeta {
            architecture: "test".to_string(),
            block_count: layers,
            head_count: 32,
            head_count_kv: 8,
            embedding_length: embd,
            key_length: None,
            value_length: None,
            causal: Some(true),
            full_attention_interval: None,
        }
    }

    /// Калибровка по замеру W2: 4096 × 32 слоя, ubatch 2048 → ≈1972 МиБ, 512 → ≈469 МиБ.
    #[test]
    fn compute_buffer_matches_w2_measurements() {
        let m = meta(4096, 32);
        let at2048 = compute_buffer_mib(&m, 2048);
        assert!((at2048 - 1972.0).abs() < 60.0, "2048 → {at2048}");
        let at512 = compute_buffer_mib(&m, 512);
        assert!((at512 - 469.0).abs() < 40.0, "512 → {at512}");
        assert!(at512 < at2048 / 3.0, "буфер обязан падать с ubatch");
    }

    /// Embedding с legacy `n_ubatch = 8192` — гигабайты (то, что пропускал бюджет).
    #[test]
    fn embedding_large_ubatch_is_gigabytes() {
        let m = meta(1024, 24);
        let big = compute_buffer_mib(&m, 8192);
        let small = compute_buffer_mib(&m, 512);
        assert!(big > 1000.0, "8192 → {big} МиБ (ожидаем ~1,4 ГиБ)");
        assert!(small < 120.0, "512 → {small} МиБ (ожидаем ~90 МиБ)");
    }

    /// Вырожденные значения не дают NaN/панику.
    #[test]
    fn compute_buffer_survives_zero_meta() {
        let m = meta(0, 0);
        assert_eq!(compute_buffer_mib(&m, 2048), 0.0);
        assert!(
            compute_buffer_mib(&m, 0) >= 0.0,
            "ubatch 0 не ломает формулу"
        );
    }
}
