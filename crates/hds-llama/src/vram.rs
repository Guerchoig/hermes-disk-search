//! Бюджет VRAM: **NVML — источник истины** (R29), `memory_free` движка — только
//! контрольная точка. Замеры W0: `list_devices()` сообщал +10 325 МиБ при занятой
//! VRAM и +2 906 МиБ при свободной, дрейф `nvidia-smi` при этом 0.
//!
//! Абстракция под macOS (W2-7): интерфейс [`VramProbe`] позволяет добавить
//! Metal (`recommendedMaxWorkingSetSize`/`currentAllocatedSize`) без правок
//! потребителей; в DoD W2 входят только NVML и фолбэк (§10.0 основного плана).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::cluster::Device;
use crate::error::Result;

/// Снимок памяти устройства в МиБ.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct VramSnapshot {
    pub total_mib: u64,
    pub used_mib: u64,
    pub free_mib: u64,
}

impl VramSnapshot {
    /// Дельта занятой памяти между снимками (для проверки «VRAM вырос»).
    pub fn used_delta(&self, base: &VramSnapshot) -> i64 {
        self.used_mib as i64 - base.used_mib as i64
    }
}

/// Откуда взяты цифры о VRAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VramSource {
    /// NVML (`nvml.dll`, драйвер NVIDIA) — источник истины на Windows/Linux.
    Nvml,
    /// `list_devices()` движка — ненадёжно (R29), только как контрольная точка.
    EngineDevices,
}

impl VramSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            VramSource::Nvml => "nvml",
            VramSource::EngineDevices => "engine-list-devices",
        }
    }
}

/// Источник данных о свободной/занятой VRAM.
pub trait VramProbe: Send + Sync {
    fn source(&self) -> VramSource;
    fn snapshot(&self) -> Option<VramSnapshot>;
}

/// NVML: память GPU по индексу (0 — первая NVIDIA-карта в системе).
pub struct NvmlProbe {
    nvml: nvml_wrapper::Nvml,
    index: u32,
    name: String,
}

impl NvmlProbe {
    /// Открыть NVML; `Err`, если драйвер/NVML недоступны (macOS, AMD, RDP).
    pub fn open(index: u32) -> Result<NvmlProbe> {
        let nvml = nvml_wrapper::Nvml::init()?;
        let name = nvml
            .device_by_index(index)
            .and_then(|d| d.name())
            .unwrap_or_else(|_| format!("gpu{index}"));
        Ok(NvmlProbe { nvml, index, name })
    }

    /// Имя GPU (например `NVIDIA GeForce RTX 3060`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Индекс GPU в нумерации NVML (совпадает с `nvidia-smi`).
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl VramProbe for NvmlProbe {
    fn source(&self) -> VramSource {
        VramSource::Nvml
    }

    fn snapshot(&self) -> Option<VramSnapshot> {
        let dev = self.nvml.device_by_index(self.index).ok()?;
        let mem = dev.memory_info().ok()?;
        Some(VramSnapshot {
            total_mib: (mem.total as u64) >> 20,
            used_mib: (mem.used as u64) >> 20,
            free_mib: (mem.free as u64) >> 20,
        })
    }
}

/// Фолбэк на случай отсутствия NVML: значения из `list_devices()` движка
/// (передаются владельцем кластера — сам `Cluster` не `Sync`, поэтому «живой»
/// опрос невозможен; это и есть причина, по которой бюджет ведём по NVML).
pub struct EngineDeviceProbe {
    snapshot: VramSnapshot,
    device_name: String,
}

impl EngineDeviceProbe {
    /// Снимок по устройству-ускорителю из `list_devices()`.
    pub fn from_device(device: &Device) -> Self {
        EngineDeviceProbe {
            snapshot: VramSnapshot {
                total_mib: device.memory_total >> 20,
                used_mib: device.memory_total.saturating_sub(device.memory_free) >> 20,
                free_mib: device.memory_free >> 20,
            },
            device_name: device.name.clone(),
        }
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

impl VramProbe for EngineDeviceProbe {
    fn source(&self) -> VramSource {
        VramSource::EngineDevices
    }

    fn snapshot(&self) -> Option<VramSnapshot> {
        Some(self.snapshot)
    }
}

/// Задел под macOS (W2-7): Metal-API отдаёт `recommendedMaxWorkingSetSize` и
/// `currentAllocatedSize`. В W2 не реализуется (нет машины Apple Silicon) —
/// точка расширения зафиксирована trait'ом [`VramProbe`].
#[cfg(target_os = "macos")]
pub fn open_metal() -> Option<Box<dyn VramProbe>> {
    None
}

/// Фоновый сэмплер VRAM: пик занятой памяти во время загрузки/инференса.
/// `Nvml` в `nvml-wrapper` — `Send + Sync`, поэтому поток безопасен.
pub struct VramSampler {
    stop: Arc<AtomicBool>,
    peak: Arc<Mutex<VramSnapshot>>,
    samples: Arc<Mutex<usize>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl VramSampler {
    /// Запустить сэмплирование с интервалом `interval`.
    pub fn start(probe: Arc<dyn VramProbe>, interval: Duration) -> VramSampler {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(Mutex::new(VramSnapshot::default()));
        let samples = Arc::new(Mutex::new(0usize));
        let (s, p, n) = (Arc::clone(&stop), Arc::clone(&peak), Arc::clone(&samples));
        let handle = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                if let Some(snap) = probe.snapshot() {
                    if let Ok(mut cur) = p.lock() {
                        if snap.used_mib > cur.used_mib {
                            *cur = snap;
                        }
                    }
                    if let Ok(mut c) = n.lock() {
                        *c += 1;
                    }
                }
                std::thread::sleep(interval);
            }
        });
        VramSampler {
            stop,
            peak,
            samples,
            handle: Some(handle),
        }
    }

    /// Пик занятой VRAM за время сэмплирования.
    pub fn peak(&self) -> VramSnapshot {
        self.peak.lock().map(|v| *v).unwrap_or_default()
    }

    /// Число успешных сэмплов (0 = NVML не отвечал).
    pub fn samples(&self) -> usize {
        self.samples.lock().map(|v| *v).unwrap_or(0)
    }

    /// Остановить сэмплирование и получить пик.
    pub fn stop(mut self) -> VramSnapshot {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.peak()
    }
}

