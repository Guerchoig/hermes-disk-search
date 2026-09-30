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
/// `n_ctx × n_parallel × layers × kv_heads × (key_dim + value_dim) × bytes/element`.
pub fn kv_cache_mib(meta: &GgufMeta, n_ctx: i64, n_parallel: i64, bits: KvBits) -> f64 {
    if !meta.has_kv_cache() {
        return 0.0;
    }
    let head = meta.head_dim() as f64;
    let k_dim = meta.key_length.map(|k| k as f64).unwrap_or(head);
    let v_dim = meta.value_length.map(|v| v as f64).unwrap_or(head);
    let per_token = meta.head_count_kv as f64 * (k_dim + v_dim) * bits.bytes_per_element();
    let elements = (n_ctx.max(0) * n_parallel.max(1)) as f64 * meta.block_count as f64 * per_token;
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
    Fits { free_mib: u64, need_mib: u64, reserve_mib: u64 },
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
