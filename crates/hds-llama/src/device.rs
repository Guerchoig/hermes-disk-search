//! Выбор устройства для инстанса: правила движка (§11.3) + наш конфиг
//! `gpu.device_index`.
//!
//! Факт W0 (R32): без явного выбора устройства движок на Windows/Linux считает
//! **на CPU**, хотя CUDA-бэкенд загружен, — поэтому `llm-host` обязан задавать
//! устройство явно, а тест A1 проверяет, каким именно способом.

use crate::cluster::Device;
use crate::error::{EngineError, Result};

/// Наш конфиг `gpu.device_index`: `0` — CPU (без выбора устройства), `1` — первый GPU.
pub const CONFIG_CPU: i32 = 0;

/// Способ адресации устройства инстанса.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelection {
    /// Устройство не задаём — «как решит движок» (Windows/Linux: CPU-only, R32).
    Auto,
    /// Явный `manual_devices_csv` (bridge-индексы устройств через запятую).
    Csv(String),
}

impl DeviceSelection {
    pub fn csv(&self) -> Option<String> {
        match self {
            DeviceSelection::Auto => None,
            DeviceSelection::Csv(c) => Some(c.clone()),
        }
    }

    pub fn label(&self) -> String {
        match self {
            DeviceSelection::Auto => "auto (без устройства)".to_string(),
            DeviceSelection::Csv(c) => format!("manual_devices_csv={c}"),
        }
    }
}

/// Устройства-ускорители (CUDA/Vulkan/Metal) в порядке перечисления движком.
pub fn accelerators(devices: &[Device]) -> Vec<&Device> {
    devices.iter().filter(|d| d.is_accelerator()).collect()
}

/// Устройство-CPU, если движок его перечислил (`backend = "CPU"`).
pub fn cpu_device(devices: &[Device]) -> Option<&Device> {
    devices.iter().find(|d| !d.is_accelerator())
}

/// Текстовое описание списка устройств (для `llm-host devices`/логов).
///
/// `desc` — описание устройства от движка (обычно имя GPU: `AMD Radeon(TM)
/// Graphics`, `NVIDIA GeForce RTX 3060`): по нему проба VRAM (NVML/DXGI)
/// сопоставляется с устройством движка (`vram_probe_for_device`).
pub fn describe_devices(devices: &[Device]) -> String {
    devices
        .iter()
        .map(|d| {
            format!(
                "index={} backend={} name={}{} free={:.0} МиБ total={:.0} МиБ",
                d.bridge_device_index,
                d.backend,
                d.name,
                if d.description.trim().is_empty() {
                    String::new()
                } else {
                    format!(" desc={}", d.description)
                },
                d.memory_free_mib(),
                d.memory_total as f64 / (1024.0 * 1024.0)
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// `gpu.device_index` → способ выбора устройства (§A2 плана W2).
///
/// Уточнение по итогам A1 (см. `tools/parity/W2_REPORT.md`):
/// * устройство задаётся **числовым** `manual_devices_csv` (bridge-индекс);
///   имя (`"CUDA0"`) движок отвергает: `manual device selection is no longer
///   available`;
/// * `Auto` (устройство не задано) на проверенной сборке ушло **на GPU**
///   (группа по умолчанию включает CUDA) — это противоречит документации движка
///   («без выбора — CPU-only»), поэтому в продакшене `Auto` не используем:
///   индекс `0` (CPU) превращаем в явный CSV с индексом CPU-устройства.
pub fn selection_from_config_index(index: i32, devices: &[Device]) -> Result<DeviceSelection> {
    if index <= CONFIG_CPU {
        // явный CPU: иначе результат зависит от версии движка (и на деле оказывается GPU)
        return Ok(match cpu_device(devices) {
            Some(cpu) => DeviceSelection::Csv(cpu.bridge_device_index.to_string()),
            None => DeviceSelection::Auto,
        });
    }
    let acc = accelerators(devices);
    let pos = (index - 1) as usize;
    match acc.get(pos) {
        Some(d) => Ok(DeviceSelection::Csv(d.bridge_device_index.to_string())),
        None => Err(EngineError::Other(format!(
            "gpu.device_index={index}: ускорителей найдено {} ({}) — выберите индекс из 'llm-host devices' \
             или 0 для CPU",
            acc.len(),
            acc.iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Обратное преобразование: ускоритель с таким `bridge_device_index` → `gpu.device_index`.
pub fn config_index_for(devices: &[Device], bridge_index: i32) -> Option<i32> {
    accelerators(devices)
        .iter()
        .position(|d| d.bridge_device_index == bridge_index)
        .map(|i| i as i32 + 1)
}
