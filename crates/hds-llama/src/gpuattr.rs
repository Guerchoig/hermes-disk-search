//! Атрибуция VRAM по процессам (L1, `tools/parity/W4_REPORT.md` §15): **кто держит память GPU**.
//!
//! Зачем: NVML даёт только суммарное «занято/свободно», а `nvidia-smi` в режиме WDDM
//! per-process память **не отдаёт** (`N/A`). В инциденте §14 резидента подозревали в
//! «чужих» 10,4 ГБ, а оказалось, что память держит **наш же процесс** (`llm_host.exe`) —
//! увидеть это удалось лишь вручную счётчиком Windows `\GPU Process Memory(*)\Dedicated Usage`.
//! Здесь тот же счётчик, но программно.
//!
//! Почему PDH, а не NVML: `nvmlDeviceGetComputeRunningProcesses` на WDDM возвращает
//! `NOT_SUPPORTED`, поэтому источник — счётчики производительности Windows (PDH),
//! а суммарное занятое по-прежнему берём из NVML ([`crate::vram`] — источник истины, R29).
//!
//! Инвариант: `foreign = max(0, всего_занято − наш_процесс)`, поэтому строка никогда не
//! врёт в минус даже если драйвер посчитал наш процесс больше суммарного.

use serde::Serialize;

/// Разбивка занятой VRAM: наш процесс против всех остальных.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct VramAttribution {
    /// Всего занято на устройстве (NVML).
    pub total_used_mib: u64,
    /// Держит наш процесс (PDH, сумма по всем инстансам нашего pid).
    pub ours_mib: u64,
    /// Всё остальное (десктоп, браузеры, чужие рантаймы).
    pub foreign_mib: u64,
}

impl VramAttribution {
    /// Собрать разбивку из суммарного «занято» и памяти нашего процесса.
    pub fn from_used(total_used_mib: u64, ours_mib: u64) -> VramAttribution {
        VramAttribution {
            total_used_mib,
            ours_mib,
            foreign_mib: total_used_mib.saturating_sub(ours_mib),
        }
    }

    /// Доля нашего процесса в занятой памяти, %.
    pub fn ours_share(&self) -> u64 {
        if self.total_used_mib == 0 {
            return 0;
        }
        self.ours_mib.saturating_mul(100) / self.total_used_mib
    }

    /// Строка для `status`/UI/`hds check`.
    pub fn line(&self) -> String {
        format!(
            "VRAM по процессам: наш процесс {} МиБ ({} %), чужие {} МиБ, всего занято {} МиБ",
            self.ours_mib,
            self.ours_share(),
            self.foreign_mib,
            self.total_used_mib
        )
    }
}

/// Разбор имени инстанса PDH (`pid_1234_luid_0x00000000_0x0000bb36_phys_0`) → pid.
///
/// Формат задаёт драйвер: `pid_<pid>_luid_<hi>_<lo>_phys_<n>`; нам нужен только `pid`,
/// поэтому разбор терпим к варианту `luid_0x...` без суффикса `phys_`.
pub fn parse_pdh_pid(instance: &str) -> Option<u32> {
    let rest = instance.strip_prefix("pid_")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse::<u32>().ok().filter(|p| *p != 0)
}

/// Атрибуция по суммарному «занято» (NVML) и памяти этого процесса (PDH).
///
/// `total_used_mib = 0` (нет NVML) отдаёт нули: честнее показать «не знаем», чем
/// нарисовать выдуманную разбивку.
pub fn attribution_for_current_process(total_used_mib: u64) -> VramAttribution {
    if total_used_mib == 0 {
        return VramAttribution::default();
    }
    let ours = process_vram_mib(std::process::id()).unwrap_or(0);
    VramAttribution::from_used(total_used_mib, ours)
}

/// Сколько МиБ держит процесс `pid` (Windows: PDH; иначе `None` — «не умеем»).
pub fn process_vram_mib(pid: u32) -> Option<u64> {
    platform::process_dedicated_bytes(pid).map(|b| b >> 20)
}

#[cfg(windows)]
mod platform {
    //! PDH-часть: счётчики `\GPU Process Memory(*)\Dedicated Usage` через `pdh.dll`.
    //!
    //! `pdh.dll` грузим через `libloading` (как движок в других местах): новых
    //! зависимостей проект не тянет, а фичу `windows-sys::Win32_System_Performance`
    //! включать не требуется.
    //!
    //! Почему английское имя счётчика: `PdhAddEnglishCounterW` резолвит счётчики по
    //! английским именам независимо от локали Windows (проверено живым
    //! `Get-Counter '\GPU Process Memory(*)\Dedicated Usage'` на этой машине).

    use std::sync::OnceLock;

    /// `PDH_FMT_COUNTERVALUE`: `DWORD CStatus` + выравненная union (`PDH_FMT_LARGE` → `LONGLONG`).
    #[repr(C)]
    struct FmtCounterValue {
        status: u32,
        _pad: u32,
        value: i64,
    }

    const PDH_FMT_LARGE: u32 = 0x0000_0400;
    const PDH_CSTATUS_VALID_DATA: u32 = 0x0000_0000;
    const PDH_CSTATUS_NEW_DATA: u32 = 0x0000_0001;
    const PDH_MORE_DATA: u32 = 0x8000_07D2;

    type PdhOpenQueryW = unsafe extern "system" fn(*const u16, usize, *mut isize) -> u32;
    type PdhAddEnglishCounterW =
        unsafe extern "system" fn(isize, *const u16, usize, *mut isize) -> u32;
    type PdhCollectQueryData = unsafe extern "system" fn(isize) -> u32;
    type PdhGetFormattedCounterValue =
        unsafe extern "system" fn(isize, u32, *mut u32, *mut FmtCounterValue) -> u32;
    type PdhCloseQuery = unsafe extern "system" fn(isize) -> u32;
    type PdhExpandWildCardPathW =
        unsafe extern "system" fn(*const u16, *const u16, *mut u16, *mut u32) -> u32;

    struct Pdh {
        _lib: libloading::Library,
        open_query: PdhOpenQueryW,
        add_counter: PdhAddEnglishCounterW,
        collect: PdhCollectQueryData,
        get_value: PdhGetFormattedCounterValue,
        close_query: PdhCloseQuery,
        expand: PdhExpandWildCardPathW,
    }

    /// PDH загружается один раз на процесс (длл системная, выгружать не нужно).
    fn pdh() -> Option<&'static Pdh> {
        static PDH: OnceLock<Option<Pdh>> = OnceLock::new();
        PDH.get_or_init(|| unsafe {
            let lib = libloading::Library::new("pdh.dll").ok()?;
            let sym = |name: &[u8]| lib.get::<*const ()>(name).ok().map(|s| *s);
            Some(Pdh {
                // явные аннотации: `clippy::missing_transmute_annotations` (в проекте
                // это уже ловилось в `hds-core::db::register_vec0`)
                open_query: std::mem::transmute::<*const (), PdhOpenQueryW>(sym(
                    b"PdhOpenQueryW\0",
                )?),
                add_counter: std::mem::transmute::<*const (), PdhAddEnglishCounterW>(sym(
                    b"PdhAddEnglishCounterW\0",
                )?),
                collect: std::mem::transmute::<*const (), PdhCollectQueryData>(sym(
                    b"PdhCollectQueryData\0",
                )?),
                get_value: std::mem::transmute::<*const (), PdhGetFormattedCounterValue>(sym(
                    b"PdhGetFormattedCounterValue\0",
                )?),
                close_query: std::mem::transmute::<*const (), PdhCloseQuery>(sym(
                    b"PdhCloseQuery\0",
                )?),
                expand: std::mem::transmute::<*const (), PdhExpandWildCardPathW>(sym(
                    b"PdhExpandWildCardPathW\0",
                )?),
                _lib: lib,
            })
        })
        .as_ref()
    }

    /// UTF-16 с завершающим нулём (вход для PDH-функций).
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Разбор MULTI_SZ (двойной нуль в конце) в список путей.
    fn multi_sz(buf: &[u16]) -> Vec<String> {
        buf.split(|c| *c == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }

    /// Сколько байт выделенной памяти GPU держит процесс `pid` (`None` — счётчик недоступен).
    ///
    /// `Some(0)` — счётчик прочитан, но инстансов нашего pid нет (мы действительно
    /// ничего не держим); `None` — PDH/счётчик недоступны (тогда статус честно молчит).
    pub fn process_dedicated_bytes(pid: u32) -> Option<u64> {
        let pdh = pdh()?;
        unsafe {
            let mut query: isize = 0;
            if (pdh.open_query)(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let result = sum_for_pid(pdh, query, pid);
            (pdh.close_query)(query);
            result
        }
    }

    /// Имя инстанса из пути счётчика: `\GPU Process Memory(pid_1_luid_…)\Dedicated Usage`
    /// → `pid_1_luid_…`.
    fn instance_of(path: &str) -> Option<&str> {
        let open = path.find('(')?;
        let close = path.rfind(')')?;
        (close > open).then(|| &path[open + 1..close])
    }

    /// Текущие значения счётчиков после `PdhCollectQueryData`: `(сумма байт, сколько валидных)`.
    fn read_total(pdh: &Pdh, counters: &[isize]) -> (u64, usize) {
        let mut total = 0u64;
        let mut valid = 0usize;
        for counter in counters {
            let mut value = FmtCounterValue {
                status: 0,
                _pad: 0,
                value: 0,
            };
            let mut kind: u32 = 0;
            let rc = unsafe { (pdh.get_value)(*counter, PDH_FMT_LARGE, &mut kind, &mut value) };
            let ok =
                rc == 0 && matches!(value.status, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA);
            if ok {
                valid += 1;
                if value.value > 0 {
                    total += value.value as u64;
                }
            }
        }
        (total, valid)
    }

    /// Сумма по всем инстансам счётчика, чей pid — наш.
    fn sum_for_pid(pdh: &Pdh, query: isize, pid: u32) -> Option<u64> {
        let wildcard = wide(r"\GPU Process Memory(*)\Dedicated Usage");
        // первый вызов — узнать размер списка (ожидаем PDH_MORE_DATA)
        let mut len: u32 = 0;
        let rc = unsafe {
            (pdh.expand)(
                std::ptr::null(),
                wildcard.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if rc != PDH_MORE_DATA || len == 0 {
            return None; // счётчика нет (не NVIDIA/старая ОС) — «не умеем измерять»
        }
        let mut buf: Vec<u16> = vec![0; len as usize];
        let rc = unsafe {
            (pdh.expand)(
                std::ptr::null(),
                wildcard.as_ptr(),
                buf.as_mut_ptr(),
                &mut len,
            )
        };
        if rc != 0 {
            return None;
        }

        let prefix = format!("pid_{pid}_");
        let mut counters: Vec<isize> = Vec::new();
        for path in multi_sz(&buf) {
            if instance_of(&path).map(|inst| inst.starts_with(&prefix)) != Some(true) {
                continue;
            }
            let wide_path = wide(&path);
            let mut counter: isize = 0;
            if unsafe { (pdh.add_counter)(query, wide_path.as_ptr(), 0, &mut counter) } == 0 {
                counters.push(counter);
            }
        }
        if counters.is_empty() {
            return Some(0); // счётчик жив, инстансов нашего pid нет — держим 0
        }
        if unsafe { (pdh.collect)(query) } != 0 {
            return None;
        }
        let (mut total, mut valid) = read_total(pdh, &counters);
        if valid == 0 {
            // счётчик «dedicated usage» иногда отдаёт значение со второго сэмпла
            std::thread::sleep(std::time::Duration::from_millis(50));
            if unsafe { (pdh.collect)(query) } != 0 {
                return None;
            }
            let again = read_total(pdh, &counters);
            total = again.0;
            valid = again.1;
        }
        if valid == 0 {
            return None;
        }
        Some(total)
    }

    #[cfg(test)]
    mod parse_tests {
        use super::*;

        #[test]
        fn extracts_instance_from_counter_path() {
            assert_eq!(
                instance_of(r"\GPU Process Memory(pid_36704_luid_0x0_0x1_phys_0)\Dedicated Usage"),
                Some("pid_36704_luid_0x0_0x1_phys_0")
            );
            assert_eq!(instance_of("без скобок"), None);
            assert_eq!(instance_of(")(наоборот("), None);
        }

        #[test]
        fn splits_multi_sz_by_nul() {
            let raw: Vec<u16> = "a\0bb\0\0".encode_utf16().collect();
            assert_eq!(multi_sz(&raw), vec!["a".to_string(), "bb".to_string()]);
            assert!(multi_sz(&[0, 0]).is_empty());
        }
    }
}

#[cfg(not(windows))]
mod platform {
    /// Вне Windows атрибуции нет (macOS — Metal-фаза, Linux вне плана): `None` = «не умеем».
    pub fn process_dedicated_bytes(_pid: u32) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pdh_instance_pid() {
        assert_eq!(
            parse_pdh_pid("pid_36704_luid_0x00000000_0x0000bb36_phys_0"),
            Some(36704)
        );
        assert_eq!(parse_pdh_pid("pid_12_luid_0x0_0x1"), Some(12));
        assert_eq!(
            parse_pdh_pid("pid_0_luid_0x0_0x1"),
            None,
            "нулевой pid — мусор"
        );
        assert_eq!(parse_pdh_pid("gpu_engine_pid_7"), None);
        assert_eq!(parse_pdh_pid("pid_"), None);
    }

    #[test]
    fn attribution_splits_ours_and_foreign() {
        let a = VramAttribution::from_used(10765, 10416);
        assert_eq!(a.ours_mib, 10416);
        assert_eq!(a.foreign_mib, 349);
        assert_eq!(a.ours_share(), 96);
        assert!(a.line().contains("наш процесс 10416 МиБ"));
        assert!(a.line().contains("чужие 349 МиБ"));
    }

    #[test]
    fn attribution_saturates_when_ours_exceeds_total() {
        let a = VramAttribution::from_used(100, 130);
        assert_eq!(a.foreign_mib, 0, "чужого не может быть отрицательно");
        assert_eq!(a.ours_share(), 130);
    }

    #[test]
    fn attribution_without_nvml_is_empty() {
        let a = attribution_for_current_process(0);
        assert_eq!(a, VramAttribution::default());
        assert_eq!(
            a.line(),
            "VRAM по процессам: наш процесс 0 МиБ (0 %), чужие 0 МиБ, всего занято 0 МиБ"
        );
    }

    #[test]
    #[ignore = "живой счётчик GPU: запускать на машине с NVIDIA (--ignored --nocapture)"]
    fn live_process_vram_is_measurable() {
        // `HDS_ATTR_PID` позволяет проверить чужой процесс (например, резидент:
        // `HDS_ATTR_PID=36704`), не заводя отдельного бинаря. Текст — ASCII, чтобы
        // пережить кодировки консоли в логах.
        let pid = std::env::var("HDS_ATTR_PID")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or_else(std::process::id);
        let bytes = platform::process_dedicated_bytes(pid);
        println!("pid {pid}: {bytes:?} bytes dedicated VRAM (None = counter unavailable)");
    }
}
