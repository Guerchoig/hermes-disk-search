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
    /// DXGI (`IDXGIAdapter3::QueryVideoMemoryInfo`) — вендор-нейтральный фолбэк
    /// Windows (AMD/Intel/NVIDIA, включая iGPU). Бюджет per-process: отвечает
    /// ровно на вопрос диспетчера «сколько ЕЩЁ может аллоцировать этот процесс».
    Dxgi,
    /// `list_devices()` движка — ненадёжно (R29), только как контрольная точка.
    EngineDevices,
}

impl VramSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            VramSource::Nvml => "nvml",
            VramSource::Dxgi => "dxgi",
            VramSource::EngineDevices => "engine-list-devices",
        }
    }

    /// Значение ключа конфига `gpu.vram_source` (`nvml` | `engine`).
    pub fn as_config_key(&self) -> &'static str {
        match self {
            VramSource::Nvml => "nvml",
            VramSource::Dxgi => "dxgi",
            VramSource::EngineDevices => "engine",
        }
    }

    /// Разбор `gpu.vram_source`. `auto` = NVML (источник истины по умолчанию, R29);
    /// неизвестное значение ⇒ `None` (вызывающий добавляет warning, но не падает).
    pub fn parse(s: &str) -> Option<VramSource> {
        match s.trim().to_lowercase().as_str() {
            "nvml" | "auto" | "" => Some(VramSource::Nvml),
            "dxgi" => Some(VramSource::Dxgi),
            "engine" | "list_devices" | "list-devices" => Some(VramSource::EngineDevices),
            _ => None,
        }
    }

    /// Можно ли доверять цифрам источника: NVML — да (источник истины),
    /// DXGI — да для «сколько ещё доступно» (бюджет отдаёт ОС, это per-process
    /// бюджет, а не системное «занято»), `memory_free` движка — нет (R29).
    pub fn is_trusted(&self) -> bool {
        matches!(self, VramSource::Nvml | VramSource::Dxgi)
    }
}

impl Default for VramSource {
    /// Дефолт W2 — NVML: движок вводит в заблуждение (R29).
    fn default() -> Self {
        VramSource::Nvml
    }
}

/// Источник данных о свободной/занятой VRAM.
pub trait VramProbe: Send + Sync {
    fn source(&self) -> VramSource;
    fn snapshot(&self) -> Option<VramSnapshot>;
    /// Имя устройства для логов (`NVIDIA GeForce RTX 3060`, `AMD Radeon ...`).
    fn name(&self) -> String {
        "gpu".to_string()
    }
}

/// Цепочка источников VRAM: **NVML** (источник истины, NVIDIA) → **DXGI**
/// (Windows, вендор-нейтральный: AMD/Intel/NVIDIA, включая iGPU) → `None`
/// («не знаем» — диспетчер честно покажет «свободно ?»).
///
/// `EngineDeviceProbe` (list_devices движка) в цепочку не входит: R29 показал
/// до +7,7 ГБ расхождения, он остаётся контрольной точкой.
pub fn open_vram_probe(nvml_index: u32) -> Option<Arc<dyn VramProbe>> {
    if let Ok(p) = NvmlProbe::open(nvml_index) {
        return Some(Arc::new(p));
    }
    #[cfg(windows)]
    if let Ok(p) = dxgi::DxgiProbe::open() {
        return Some(Arc::new(p));
    }
    None
}

/// Windows: DXGI — измерение памяти GPU **без** драйвера NVIDIA.
///
/// Почему: NVML есть только у NVIDIA, а на машинах с AMD/Intel (особенно
/// интегрированной графикой) его нет — живой инцидент 05.10.2026: диспетчер
/// llm-host отклонял транскрибацию «нужно 2254 МиБ, свободно ?» и задание
/// крутилось в ретраях. `IDXGIAdapter3::QueryVideoMemoryInfo` (DXGI 1.4,
/// Windows 10+, dxgi.dll) — документированный, вендор-нейтральный способ,
/// ровно под нашу задачу: MS пишет, что `Budget` — «the OS-provided video memory
/// budget, in bytes, that the application should target», а приложения «must
/// keep usage within the budget assigned to the application process».
///
/// **Границы применимости (проверено на железе 05.10.2026).** Это цифры
/// **нашего процесса**, а не системы: `CurrentUsage` — только наши аллокации,
/// а `Budget − CurrentUsage` отвечает на вопрос «сколько ЕЩЁ разрешено нам», но
/// НЕ «сколько физически свободно». Замер в один момент на одной машине:
/// NVML для RTX 3060 — занято 8549 / свободно 3565 МиБ (системная истина, чат
/// уже резидент), DXGI для той же карты — бюджет 11347, usage 0 (наш процесс),
/// т.е. «свободно» 11347: ОС допускает аллокации, вытесняя других (MS
/// предупреждает о stuttering при выходе за пределы физического). Поэтому на
/// NVIDIA источником истины остаётся NVML (R29), а DXGI — фолбэк для машин без
/// NVML, где важнее «влезет ли модель», чем системная занятость.
///
/// Сегменты: LOCAL — обычная карта; у AMD iGPU (унифицированная память) LOCAL
/// несёт бюджет всего пула (замер: ~48 ГиБ на 64 ГБ RAM), NON_LOCAL пуст —
/// поэтому пробуем LOCAL и лишь при нулевом бюджете переключаемся на NON_LOCAL.
///
/// **Известное ограничение (UMA).** На интегрированной графике бюджет DXGI
/// описывает разрешение ОС, а не heap бэкенда движка: ноутбук AMD (32 ГБ RAM)
/// сообщает движку `Vulkan0 total=16244 МиБ, free=7153 МиБ`, тогда как бюджет
/// DXGI там заметно больше. Если появится инцидент «диспетчер разрешил, а движок
/// не смог аллоцировать», решение — брать `min(бюджет DXGI, память целевого
/// устройства движка)`: сейчас оба сигнала на этом ноутбуке дают «влезает»
/// (нужно ≈3,3 ГиБ против 7,1 ГиБ свободных), и усложнять решение преждевременно.
#[cfg(windows)]
pub mod dxgi {
    // Raw FFI, а не `windows-sys`: в закреплённой версии windows-sys 0.59 модуля
    // Win32::Graphics::Dxgi нет, а тяжёлая зависимость `windows` ради трёх вызовов
    // излишня. COM-ABI стабилен; GUID-ы и порядок методов сверены с метаданными
    // windows-rs (metadata/win32/dxgi.rdl, dxgi1_4.rdl).
    use std::ffi::c_void;
    use std::sync::Mutex;

    use windows_sys::core::{GUID, HRESULT};

    #[cfg(test)]
    use super::NvmlProbe;
    use super::{VramProbe, VramSnapshot, VramSource};

    // --- GUID-ы интерфейсов (сверены с метаданными windows-rs, 05.10.2026) ---
    const IID_IDXGIFACTORY1: GUID = GUID {
        data1: 0x770aae78,
        data2: 0xf26f,
        data3: 0x4dba,
        data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
    };
    const IID_IDXGIADAPTER3: GUID = GUID {
        data1: 0x645967a4,
        data2: 0x1392,
        data3: 0x4310,
        data4: [0xa7, 0x98, 0x80, 0x53, 0xce, 0x3e, 0x93, 0xfd],
    };

    /// `DXGI_MEMORY_SEGMENT_GROUP`: LOCAL = 0 (dedicated), NON_LOCAL = 1 (shared).
    const SEGMENT_LOCAL: u32 = 0;
    const SEGMENT_NON_LOCAL: u32 = 1;
    /// `DXGI_ERROR_NOT_FOUND` — конец перечисления адаптеров.
    const DXGI_ERROR_NOT_FOUND: HRESULT = 0x887A0002u32 as HRESULT;

    /// `DXGI_QUERY_VIDEO_MEMORY_INFO` (dxgi1_4.h): память **нашего процесса** —
    /// `Budget` (сколько ОС разрешила), `CurrentUsage` (сколько занято).
    #[repr(C)]
    #[derive(Default)]
    struct VideoMemoryInfo {
        budget: u64,
        current_usage: u64,
        available_for_reservation: u64,
        current_reservation: u64,
    }

    /// `DXGI_ADAPTER_DESC1` (dxgi.h).
    #[repr(C)]
    struct AdapterDesc1 {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        adapter_luid: Luid,
        flags: u32,
    }

    #[repr(C)]
    struct Luid {
        low_part: u32,
        high_part: i32,
    }

    // --- vtable-слоты (COM ABI; нумерация методов из метаданных windows-rs) ---
    // IUnknown: QI 0, AddRef 1, Release 2. IDXGIObject: 3–6.
    // IDXGIFactory: EnumAdapters 7, MakeWindowAssociation 8, GetWindowAssociation 9,
    //               CreateSwapChain 10, CreateSoftwareAdapter 11.
    // IDXGIFactory1: EnumAdapters1 12, IsCurrent 13.
    // IDXGIAdapter: EnumOutputs 7, GetDesc 8, CheckInterfaceSupport 9.
    // IDXGIAdapter1: GetDesc1 10. IDXGIAdapter2: GetDesc2 11.
    // IDXGIAdapter3: Register…Teardown 12, Unregister…Teardown 13,
    //                QueryVideoMemoryInfo 14, SetVideoMemoryReservation 15,
    //                Register…BudgetChange 16, Unregister…BudgetChange 17.
    type QueryInterfaceFn =
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
    type EnumAdapters1Fn = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT;
    type GetDesc1Fn = unsafe extern "system" fn(*mut c_void, *mut AdapterDesc1) -> HRESULT;
    type QueryVideoMemoryInfoFn =
        unsafe extern "system" fn(*mut c_void, u32, u32, *mut VideoMemoryInfo) -> HRESULT;

    /// Прочитать указатель функции из vtable COM-объекта (слот `index`).
    ///
    /// Грабля: арифметика по `*const c_void` сдвигает на 1 байт (`c_void`
    /// безразмерный) — vtable идём как по массиву указателей (`*const c_void`),
    /// шаг слота = 8 байт.
    unsafe fn slot<T: Copy>(obj: *mut c_void, index: usize) -> T {
        let vtbl = *(obj as *mut *const c_void) as *const *const c_void;
        let entry: *const c_void = *vtbl.add(index);
        std::mem::transmute_copy::<*const c_void, T>(&entry)
    }

    unsafe fn release(obj: *mut c_void) {
        if !obj.is_null() {
            let f: ReleaseFn = slot(obj, 2);
            f(obj);
        }
    }

    #[link(name = "dxgi")]
    extern "system" {
        fn CreateDXGIFactory1(riid: *const GUID, pp_factory: *mut *mut c_void) -> HRESULT;
    }

    /// Программный адаптер Microsoft («Microsoft Basic Render Driver») — не GPU.
    const MS_SOFTWARE_VENDOR_ID: u32 = 0x1414;

    pub struct DxgiProbe {
        /// COM-объект адаптера (IDXGIAdapter3): доступ строго под мьютексом
        /// (COM API не обязан быть потокобезопасным), поэтому Send/Sync — вручную.
        handles: Mutex<Option<Handles>>,
        device_name: String,
        /// Dedicated-сегмент адаптера из `DXGI_ADAPTER_DESC1` — по нему выбираем
        /// дискретную карту и понимаем, iGPU это или нет.
        dedicated_bytes: u64,
        /// Сегмент, который измеряем: LOCAL (обычная карта/унифицированная память
        /// AMD iGPU) с fallback на NON_LOCAL, если LOCAL-бюджет нулевой.
        segment: u32,
    }

    struct Handles {
        adapter3: *mut c_void,
    }

    // Все обращения к COM-указателям идут под Mutex — гонок нет.
    unsafe impl Send for DxgiProbe {}
    unsafe impl Sync for DxgiProbe {}

    impl DxgiProbe {
        /// Открыть DXGI: выбрать **наиболее ёмкий по dedicated** аппаратный адаптер.
        ///
        /// Почему не «первый»: `EnumAdapters1` отдаёт адаптер, на котором показан
        /// рабочий стол (обычно iGPU), а движок может работать на дискретной карте —
        /// на машине с двумя GPU «первый» дал бы цифры не того устройства.
        /// Дискретная карта узнаётся по крупному dedicated-сегменту (у iGPU это
        /// BIOS carve-out, сотни МиБ). Программные адаптеры MS пропускаются.
        pub fn open() -> crate::error::Result<DxgiProbe> {
            let mut all = Self::open_all()?;
            if all.is_empty() {
                return Err(crate::EngineError::Other(
                    "DXGI: ни одного аппаратного адаптера не найдено".to_string(),
                ));
            }
            let best = all
                .iter()
                .enumerate()
                .max_by_key(|(_, p)| p.dedicated_bytes)
                .map(|(i, _)| i)
                .unwrap_or(0);
            Ok(all.swap_remove(best))
        }

        /// Все аппаратные адаптеры машины (диагностика/валидация на железе:
        /// `live_dxgi_vs_nvml`, отчёты о VRAM при нескольких GPU).
        pub fn open_all() -> crate::error::Result<Vec<DxgiProbe>> {
            let mut factory: *mut c_void = std::ptr::null_mut();
            let hr = unsafe { CreateDXGIFactory1(&IID_IDXGIFACTORY1, &mut factory) };
            if hr != 0 || factory.is_null() {
                return Err(crate::EngineError::Other(format!(
                    "CreateDXGIFactory1: HRESULT {hr:#x}"
                )));
            }
            let probes = enumerate_adapters(factory);
            // Адаптеры держат собственную ссылку (QI) — фабрика больше не нужна.
            release_handle(&mut factory);
            if probes.is_empty() {
                return Err(crate::EngineError::Other(
                    "DXGI: ни одного аппаратного адаптера не найдено".to_string(),
                ));
            }
            Ok(probes)
        }
    }

    /// Перечислить аппаратные адаптеры (программные адаптеры MS пропускаются),
    /// для каждого вернуть готовый [`DxgiProbe`] с собственной ссылкой QI.
    fn enumerate_adapters(factory: *mut c_void) -> Vec<DxgiProbe> {
        let mut out = Vec::new();
        let mut i = 0u32;
        loop {
            let mut adapter: *mut c_void = std::ptr::null_mut();
            // IDXGIFactory1::EnumAdapters1 — слот 12.
            let enum_fn: EnumAdapters1Fn = unsafe { slot(factory, 12) };
            let hr = unsafe { enum_fn(factory, i, &mut adapter) };
            if hr == DXGI_ERROR_NOT_FOUND {
                break;
            }
            if hr != 0 || adapter.is_null() {
                i += 1;
                continue;
            }
            let mut desc = unsafe { std::mem::zeroed::<AdapterDesc1>() };
            // IDXGIAdapter1::GetDesc1 — слот 10.
            let desc_fn: GetDesc1Fn = unsafe { slot(adapter, 10) };
            let got = unsafe { desc_fn(adapter, &mut desc) };
            if got != 0 || desc.vendor_id == MS_SOFTWARE_VENDOR_ID {
                unsafe { release(adapter) };
                i += 1;
                continue;
            }
            let mut adapter3: *mut c_void = std::ptr::null_mut();
            // IUnknown::QueryInterface — слот 0.
            let qi_fn: QueryInterfaceFn = unsafe { slot(adapter, 0) };
            let hr = unsafe { qi_fn(adapter, &IID_IDXGIADAPTER3, &mut adapter3) };
            // Ссылка EnumAdapters больше не нужна: QI дал свою.
            unsafe { release(adapter) };
            if hr != 0 || adapter3.is_null() {
                i += 1;
                continue;
            }
            let mut name = String::from_utf16_lossy(&desc.description);
            if let Some(nul) = name.find('\0') {
                name.truncate(nul);
            }
            out.push(DxgiProbe {
                handles: Mutex::new(Some(Handles { adapter3 })),
                device_name: name,
                dedicated_bytes: desc.dedicated_video_memory as u64,
                segment: SEGMENT_LOCAL,
            });
            i += 1;
        }
        out
    }

    fn release_handle(h: &mut *mut c_void) {
        if !h.is_null() {
            unsafe { release(*h) };
            *h = std::ptr::null_mut();
        }
    }

    impl Drop for DxgiProbe {
        fn drop(&mut self) {
            if let Ok(mut inner) = self.handles.lock() {
                if let Some(h) = inner.as_mut() {
                    release_handle(&mut h.adapter3);
                }
            }
        }
    }

    impl VramProbe for DxgiProbe {
        fn source(&self) -> VramSource {
            VramSource::Dxgi
        }

        fn name(&self) -> String {
            self.device_name.clone()
        }

        fn snapshot(&self) -> Option<VramSnapshot> {
            let inner = self.handles.lock().ok()?;
            let h = inner.as_ref()?;
            let mut info = VideoMemoryInfo::default();
            // IDXGIAdapter3::QueryVideoMemoryInfo — слот 14; NodeIndex 0.
            let q_fn: QueryVideoMemoryInfoFn = unsafe { slot(h.adapter3, 14) };
            let mut segment = self.segment;
            let mut hr = unsafe { q_fn(h.adapter3, 0, segment, &mut info) };
            if hr != 0 {
                return None;
            }
            if info.budget == 0 && segment == SEGMENT_LOCAL {
                // У части драйверов бюджет LOCAL нулевой, пока процесс не создал
                // D3D-объектов, а у iGPU осмысленный пул иногда сообщается как
                // NON_LOCAL — пробуем второй сегмент, прежде чем сдаться.
                segment = SEGMENT_NON_LOCAL;
                info = VideoMemoryInfo::default();
                hr = unsafe { q_fn(h.adapter3, 0, segment, &mut info) };
                if hr != 0 || info.budget == 0 {
                    return None;
                }
            }
            Some(VramSnapshot {
                total_mib: info.budget >> 20,
                used_mib: info.current_usage >> 20,
                free_mib: info.budget.saturating_sub(info.current_usage) >> 20,
            })
        }
    }

    /// Живая сверка измерений на железе: NVML (эталон NVIDIA) против DXGI по
    /// каждому аппаратному адаптеру. Отвечает на вопрос «можно ли доверять DXGI»
    /// эмпирически: на машине с NVIDIA сравниваем free/total NVML и бюджет DXGI,
    /// а также проверяем, что выбирается дискретная карта, а не iGPU.
    #[test]
    #[ignore = "живое железо: запускать вручную (--ignored --nocapture)"]
    fn live_dxgi_vs_nvml() {
        match NvmlProbe::open(0) {
            Ok(nvml) => println!("NVML {}: {:?}", nvml.name(), nvml.snapshot()),
            Err(e) => println!("NVML недоступен (не NVIDIA): {e}"),
        }
        match DxgiProbe::open_all() {
            Ok(all) => {
                for p in &all {
                    println!(
                        "DXGI «{}» (dedicated {} МиБ): {:?}",
                        p.name(),
                        p.dedicated_bytes >> 20,
                        p.snapshot()
                    );
                }
                if let Ok(best) = DxgiProbe::open() {
                    println!("DXGI выбрал для измерений: «{}»", best.name());
                }
            }
            Err(e) => println!("DXGI недоступен: {e}"),
        }
    }

    /// DXGI есть на любой Windows (включая CI с программным адаптером):
    /// тест терпимый — если аппаратного адаптера нет, снимок может быть `None`.
    #[test]
    fn dxgi_probe_works_on_windows() {
        let probe = match DxgiProbe::open() {
            Ok(p) => p,
            Err(e) => {
                println!("DXGI недоступен на этой машине: {e} — тест пропущен");
                return;
            }
        };
        assert_eq!(probe.source(), VramSource::Dxgi);
        println!("адаптер: {}", probe.name());
        if let Some(v) = probe.snapshot() {
            println!(
                "бюджет {} МиБ, занято {} МиБ, свободно {} МиБ",
                v.total_mib, v.used_mib, v.free_mib
            );
        } else {
            println!("снимок None — адаптер программный");
        }
    }
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

    /// Имя GPU — как `nvidia-smi` (`NVIDIA GeForce RTX 3060`).
    fn name(&self) -> String {
        self.name.clone()
    }

    fn snapshot(&self) -> Option<VramSnapshot> {
        let dev = self.nvml.device_by_index(self.index).ok()?;
        let mem = dev.memory_info().ok()?;
        Some(VramSnapshot {
            total_mib: mem.total >> 20,
            used_mib: mem.used >> 20,
            free_mib: mem.free >> 20,
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
