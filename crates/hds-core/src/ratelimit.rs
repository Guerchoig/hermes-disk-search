//! Rate-limit (M3 T3, `PLAN_M3_NETWORK_PERIMETER.md` §3.3): in-process
//! token-bucket без новых крейтов.
//!
//! Ключ — **токен** (Bearer, если есть), иначе IP клиента (за доверенным
//! прокси — из `X-Forwarded-For`, см. `hds_core::net`). Два измерения:
//! * частота запросов — `requests_per_min` с всплеском `burst`;
//! * `/auth/login` — отдельный, более жёсткий лимит `login_per_min`
//!   (антибрутфорс, ключ всегда IP).
//!
//! Параллелизм тяжёлых запросов (`ask_my_files`) — `max_inflight`:
//! превышение → `429` без запуска работы.
//!
//! Состояние — в памяти процесса; несколько процессов (`mcp-http` + `ui`)
//! лимитируются независимо (документированное ограничение; общий лимит —
//! задача прокси). Дефолт: desktop — выключен, `server` — включён
//! (120 req/min, burst 30, login 10/min, inflight 4); всё перекрывается
//! ключами конфига `rate_limit.*`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::{dig, Config};

/// Параметры лимитов из конфига (`rate_limit: {...}`).
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimit {
    /// Включён ли лимитер (`rate_limit.enabled`; без ключа — по профилю).
    pub enabled: bool,
    /// Частота запросов в минуту (дефолт 120).
    pub requests_per_min: u32,
    /// Всплеск — ёмкость bucket (дефолт 30).
    pub burst: u32,
    /// Лимит `/auth/login` в минуту по IP (дефолт 10).
    pub login_per_min: u32,
    /// Предел одновременных тяжёлых запросов на ключ (дефолт 4).
    pub max_inflight: u32,
}

impl RateLimit {
    fn from_config(cfg: &Config) -> RateLimit {
        let u = |k: &str, d: u32| -> u32 {
            dig(cfg, k)
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v > 0)
                .unwrap_or(d)
        };
        RateLimit {
            enabled: match dig(cfg, "rate_limit.enabled").and_then(|v| v.as_bool()) {
                Some(v) => v,
                None => crate::config::profile(cfg).is_server(),
            },
            requests_per_min: u("rate_limit.requests_per_min", 120),
            burst: u("rate_limit.burst", 30),
            login_per_min: u("rate_limit.login_per_min", 10),
            max_inflight: u("rate_limit.max_inflight", 4),
        }
    }
}

/// Один bucket: остаток «жетонов» и момент последнего пополнения.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Потокобезопасный лимитер. `None` из `from_config` — лимиты выключены
/// (вызовы `check*` даже не доходят до блокировок).
pub struct Limiter {
    cfg: RateLimit,
    buckets: Mutex<HashMap<String, Bucket>>,
    login: Mutex<HashMap<String, Bucket>>,
    inflight: Mutex<HashMap<String, u32>>,
}

impl Limiter {
    /// Лимитер из конфига; `None` — выключен (desktop по умолчанию).
    pub fn from_config(cfg: &Config) -> Option<Limiter> {
        let r = RateLimit::from_config(cfg);
        if !r.enabled {
            return None;
        }
        Some(Limiter {
            cfg: r,
            buckets: Mutex::new(HashMap::new()),
            login: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
        })
    }

    /// Тестовый лимитер с явными параметрами (юнит-тесты).
    pub fn with(cfg: RateLimit) -> Limiter {
        Limiter {
            cfg,
            buckets: Mutex::new(HashMap::new()),
            login: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// Параметры (`X-RateLimit-*`, сообщения).
    pub fn cfg(&self) -> &RateLimit {
        &self.cfg
    }

    /// `Some(sec)` — превышен лимит частоты (подождать `sec`), `None` — пропустить.
    ///
    /// Token-bucket: ёмкость `burst`, пополнение `requests_per_min/60` в сек.
    pub fn check(&self, key: &str) -> Option<u64> {
        let rate = self.cfg.requests_per_min as f64 / 60.0;
        self.tick(&self.buckets, key, self.cfg.burst as f64, rate)
    }

    /// Жёсткий лимит `/auth/login` (антибрутфорс, ключ — IP логина).
    pub fn check_login(&self, key: &str) -> Option<u64> {
        let rate = self.cfg.login_per_min as f64 / 60.0;
        // Ёмкость = 5 жетонов (всплеск мал — брутфорс не помогает).
        self.tick(&self.login, key, 5.0, rate)
    }

    fn tick(
        &self,
        map: &Mutex<HashMap<String, Bucket>>,
        key: &str,
        capacity: f64,
        rate: f64,
    ) -> Option<u64> {
        let mut m = map.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let b = m.entry(key.to_string()).or_insert(Bucket {
            tokens: capacity,
            last: now,
        });
        let elapsed = now.duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * rate).min(capacity);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            None
        } else {
            // Через сколько секунд накопится 1 жетон (минимум 1).
            Some((((1.0 - b.tokens) / rate).ceil() as u64).max(1))
        }
    }

    /// Занять слот параллелизма (`max_inflight` на ключ); `false` — предел.
    pub fn inflight_acquire(&self, key: &str) -> bool {
        let mut m = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        let n = m.entry(key.to_string()).or_insert(0);
        if *n >= self.cfg.max_inflight {
            false
        } else {
            *n += 1;
            true
        }
    }

    /// Отпустить слот параллелизма (в паре с `inflight_acquire`).
    pub fn inflight_release(&self, key: &str) {
        let mut m = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(key);
            }
        }
    }

    /// Занять слот с авто-освобождением на `Drop`; `None` — предел достигнут.
    pub fn inflight_guard(self: &Arc<Self>, key: &str) -> Option<InflightGuard> {
        if self.inflight_acquire(key) {
            Some(InflightGuard {
                limiter: Arc::clone(self),
                key: key.to_string(),
            })
        } else {
            None
        }
    }
}

/// RAII-слот параллелизма: на `Drop` освобождает слот.
pub struct InflightGuard {
    limiter: Arc<Limiter>,
    key: String,
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.limiter.inflight_release(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::{Limiter, RateLimit};
    use crate::config::Config;
    use std::sync::Arc;

    fn cfg(text: &str) -> Config {
        serde_yaml::from_str(text).unwrap()
    }

    fn lim(rpm: u32, burst: u32, login: u32, inflight: u32) -> Limiter {
        Limiter::with(RateLimit {
            enabled: true,
            requests_per_min: rpm,
            burst,
            login_per_min: login,
            max_inflight: inflight,
        })
    }

    /// Дефолты по профилю: desktop — выключен, server — включён (план §3.3).
    #[test]
    fn defaults_follow_profile() {
        assert!(
            Limiter::from_config(&cfg("")).is_none(),
            "desktop — лимитов нет"
        );
        assert!(Limiter::from_config(&cfg("profile: desktop")).is_none());
        let l = Limiter::from_config(&cfg("profile: server")).expect("server — включены");
        assert_eq!(l.cfg().requests_per_min, 120);
        assert_eq!(l.cfg().burst, 30);
        assert_eq!(l.cfg().login_per_min, 10);
        assert_eq!(l.cfg().max_inflight, 4);
        // Явное выключение сильнее профиля; явное включение — сильнее десктопа.
        assert!(
            Limiter::from_config(&cfg("profile: server\nrate_limit:\n  enabled: false")).is_none()
        );
        assert!(
            Limiter::from_config(&cfg("profile: desktop\nrate_limit:\n  enabled: true")).is_some()
        );
    }

    /// Bucket: burst запросов проходят, дальше 429 с Retry-After ≥ 1.
    #[test]
    fn burst_then_429_with_retry_after() {
        let l = lim(60, 3, 5, 2); // 1 жетон/сек, ёмкость 3
        assert!(l.check("tok-a").is_none());
        assert!(l.check("tok-a").is_none());
        assert!(l.check("tok-a").is_none());
        let retry = l.check("tok-a").expect("четвёртый — превышение");
        assert!(retry >= 1, "retry={retry}");
        // Другой ключ — свои жетоны (ключ токен/IP, не глобальный).
        assert!(l.check("tok-b").is_none());
    }

    /// `/auth/login` — отдельный, жёсткий лимит (антибрутфорс).
    #[test]
    fn login_limit_is_separate_and_strict() {
        let l = lim(600, 50, 3, 4);
        // Общая частота почти не исчерпана...
        assert!(l.check("1.2.3.4").is_none());
        assert!(l.check("1.2.3.4").is_none());
        // ...а логин-лимит исчерпан пятью попытками с одного IP
        // (burst 5; устойчивый темп — login_per_min).
        assert!(l.check_login("1.2.3.4").is_none());
        assert!(l.check_login("1.2.3.4").is_none());
        assert!(l.check_login("1.2.3.4").is_none());
        assert!(l.check_login("1.2.3.4").is_none());
        assert!(l.check_login("1.2.3.4").is_none());
        assert!(l.check_login("1.2.3.4").is_some(), "6-й логин — 429");
        // Другой IP — свои попытки.
        assert!(l.check_login("5.6.7.8").is_none());
    }

    /// Параллелизм: до max_inflight — да, дальше нет; Drop освобождает.
    #[test]
    fn inflight_capped_and_guard_releases() {
        let l = Arc::new(lim(600, 50, 50, 2));
        assert!(l.inflight_acquire("tok"));
        assert!(l.inflight_acquire("tok"));
        assert!(!l.inflight_acquire("tok"), "третий параллельный — 429");
        {
            let _g = l.inflight_guard("tok2").expect("другой ключ — ок");
            assert!(l.inflight_acquire("tok2"));
            assert!(!l.inflight_acquire("tok2"));
        } // guard.drop → слот tok2 освобождён
        assert!(l.inflight_acquire("tok2"), "после Drop слот свободен");
        l.inflight_release("tok");
        assert!(l.inflight_acquire("tok"), "release возвращает слот");
    }

    /// Частота и параллелизм — независимые измерения.
    #[test]
    fn frequency_and_inflight_independent() {
        let l = Arc::new(lim(600, 50, 50, 1));
        assert!(l.check("tok").is_none());
        let g = l.inflight_guard("tok").unwrap();
        // Частота не заблокирована inflight-слотом...
        assert!(l.check("tok").is_none());
        // ...а слот параллелизма исчерпан.
        assert!(!l.inflight_acquire("tok"), "второй inflight — 429");
        drop(g);
        assert!(l.inflight_acquire("tok"), "после Drop слот свободен");
    }
}
