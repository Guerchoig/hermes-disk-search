//! Дата и время без внешних крейтов (перенесено из `hds_cli::support` в A4.4).
//!
//! Нужно там, где имя артефакта содержит дату-время: копия прежней версии
//! документа в `.hds-agent\backup\<дата-время>\` (TR-7.6), `.moved-<stamp>` в
//! `db_move`, `status` показывает локальное время последней индексации.
//! Одна реализация на весь workspace: `chrono`/`time` в зависимостях нет
//! намеренно (ядро собирается минимальным набором).
//!
//! Время — **локальное**: пользователь видит в имени каталога свой час, а не
//! UTC. На Windows смещение берётся у `GetLocalTime` (до минуты).

use std::time::{SystemTime, UNIX_EPOCH};

/// `time.time()` — секунды с дробной частью от эпохи.
pub fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Локальная дата-время из epoch-секунд (порт `datetime.fromtimestamp(ts)`):
/// `YYYY-MM-DD HH:MM:SS` и микросекунды, если ненулевые (как Python `str(datetime)`).
pub fn fmt_local_datetime(ts: f64) -> String {
    let local = ts + local_offset_secs() as f64;
    let mut secs = local.floor() as i64;
    let mut micro = (local.fract() * 1_000_000.0).round() as i64;
    if micro >= 1_000_000 {
        secs += 1;
        micro = 0;
    }
    let (y, m, d, hh, mm, ss) = civil_from_epoch(secs);
    if micro == 0 {
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
    } else {
        format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}.{micro:06}")
    }
}

/// Метка для **имени каталога/файла**: `YYYYMMDD-HHMMSS` (локальное время).
///
/// Двоеточия в имени файла запрещены Windows, поэтому формат без них — тот же,
/// что у `time.strftime("%Y%m%d-%H%M%S")` в `hds/cli.py`.
pub fn stamp_local(ts: f64) -> String {
    let local = ts + local_offset_secs() as f64;
    let secs = local.floor() as i64;
    let (y, m, d, hh, mm, ss) = civil_from_epoch(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// Смещение локального времени от UTC (секунды), округлённое до минуты.
#[cfg(windows)]
fn local_offset_secs() -> i64 {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTimeLocal {
        year: u16,
        month: u16,
        dow: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(p: *mut SystemTimeLocal);
    }
    let mut st = SystemTimeLocal::default();
    unsafe { GetLocalTime(&mut st) };
    let local = days_from_civil(st.year as i64, st.month as u32, st.day as u32) * 86400
        + st.hour as i64 * 3600
        + st.minute as i64 * 60
        + st.second as i64;
    let utc = now_epoch() as i64;
    ((local - utc) as f64 / 60.0).round() as i64 * 60
}

/// Вне Windows локальное смещение не вычисляем — используем UTC.
#[cfg(not(windows))]
fn local_offset_secs() -> i64 {
    0
}

/// Дни с 1970-01-01 (Howard Hinnant `days_from_civil`). Нужен только Windows-ветке.
#[cfg(windows)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m as i64 - 3 } else { m as i64 + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `(year, month, day, hour, min, sec)` из epoch-секунд (UTC).
pub fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hh, mm, ss) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_zero_is_1970() {
        assert_eq!(civil_from_epoch(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_epoch(86_400), (1970, 1, 2, 0, 0, 0));
        // 2026-11-20 12:34:56 UTC
        assert_eq!(civil_from_epoch(1_795_178_096), (2026, 11, 20, 12, 34, 56));
    }

    #[test]
    fn stamps_have_no_colons() {
        let stamp = stamp_local(1_795_178_096.0);
        assert_eq!(stamp.len(), 15, "{stamp}");
        assert!(!stamp.contains(':'), "{stamp}");
        assert_eq!(stamp.chars().nth(8), Some('-'), "{stamp}");
        let text = fmt_local_datetime(1_795_178_096.0);
        assert!(text.contains(':'), "{text}");
    }
}
