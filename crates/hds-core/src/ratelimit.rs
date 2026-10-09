//! Rate-limit (M3 T3, `PLAN_M3_NETWORK_PERIMETER.md` §3.3): in-process
//! token-bucket без новых крейтов.
//!
//! Ключ — **пользователь** (M4 T4 §3.3): `user:<login>` по валидному Bearer,
//! иначе IP клиента (за доверенным прокси — из `X-Forwarded-For`, см.
//! `hds_core::net`). Так «два токена одного пользователя» делят одну квоту
//! (регресс M3 F10.3); `rate_limit.per_token: true` возвращает пер-токенный
//! ключ (поведение M3). Измерения:
//! * частота запросов — `requests_per_min` с всплеском `burst`
//!   (`requests_per_min_per_user` — агрегат на пользователя при `per_token`);
//! * `/auth/login` — отдельный, более жёсткий лимит `login_per_min`
//!   (антибрутфорс, ключ всегда IP).
//!
//! Параллелизм тяжёлых запросов (`ask_my_files`): `max_inflight` — глобальный
//! (лимит исполнения очереди, M4 T3), `max_inflight_per_user` — на пользователя
//! (M4 T4); превышение → `429` без запуска работы.
//!
//! Состояние — в памяти процесса; несколько процессов (`mcp-http` + `ui`)
//! лимитируются независимо (документированное ограничение; общий лимит —
//! задача прокси). Дефолт: desktop — выключен, `server` — включён
//! (120 req/min, burst 30, login 10/min, inflight: server 5 / desktop 4,
//! per-user inflight 2); всё перекрывается ключами конфига `rate_limit.*`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use crate::config::{dig, Config};

/// Дефолт `rate_limit.max_inflight_per_user` (M4 T4 §3.3): на 5 пользователей
/// один «жадный» не должен забирать все слоты исполнения.
pub const DEFAULT_MAX_INFLIGHT_PER_USER: u32 = 2;

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
    /// Предел одновременных тяжёлых запросов (дефолт: server 5 / desktop 4) —
    /// **глобальный** (лимит ИСПОЛНЕНИЯ очереди, M4 T3).
    pub max_inflight: u32,
    /// Предел одновременных тяжёлых запросов **на пользователя** (M4 T4,
    /// дефолт [`DEFAULT_MAX_INFLIGHT_PER_USER`]).
    pub max_inflight_per_user: u32,
    /// Частота запросов в минуту **на пользователя** (M4 T4; дефолт — как
    /// `requests_per_min`) — применяется при `rate_limit.per_token: true`,
    /// чтобы два токена одного пользователя не удваивали квоту (F10.3).
    pub requests_per_min_per_user: u32,
    /// `rate_limit.per_token: true` — вернуть пер-токенный ключ лимитов
    /// (поведение M3); по умолчанию ключ — пользователь (M4 T4 §3.3).
    pub per_token: bool,
}

impl RateLimit {
    /// Параметры лимитов из конфига (M4 T3: тем же путём берётся
    /// `max_inflight` для очереди — единый источник дефолта).
    pub fn from_config(cfg: &Config) -> RateLimit {
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
            // F3 (M5 T0.3): лимит ИСПОЛНЕНИЯ очереди не «съедает» слоты чата —
            // дефолт server = 5 (= дефолт `llm_server.parallel`), desktop —
            // прежние 4 (лимитер M3). Рассинхрон явных значений помечает
            // `hds check` (пункт `queue`, `crates/hds-index/src/diag.rs`).
            max_inflight: u(
                "rate_limit.max_inflight",
                if crate::config::profile(cfg).is_server() {
                    5
                } else {
                    4
                },
            ),
            max_inflight_per_user: u(
                "rate_limit.max_inflight_per_user",
                DEFAULT_MAX_INFLIGHT_PER_USER,
            ),
            requests_per_min_per_user: u(
                "rate_limit.requests_per_min_per_user",
                u("rate_limit.requests_per_min", 120),
            ),
            per_token: dig(cfg, "rate_limit.per_token")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
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
    /// Пер-пользовательская частота (`requests_per_min_per_user`; M4 T4 —
    /// применяется при `per_token: true`).
    user_buckets: Mutex<HashMap<String, Bucket>>,
    /// Пер-пользовательский параллелизм (`max_inflight_per_user`; M4 T4).
    /// F6 (M5 T0.6): мёртвый путь `inflight_*`/`InflightGuard` удалён —
    /// рантайм тяжёлых идёт через очередь либо `user_inflight_*`.
    user_inflight: Mutex<HashMap<String, u32>>,
}

impl Limiter {
    /// Лимитер из конфига; `None` — выключен (desktop по умолчанию).
    pub fn from_config(cfg: &Config) -> Option<Limiter> {
        let r = RateLimit::from_config(cfg);
        if !r.enabled {
            return None;
        }
        Some(Limiter::with(r))
    }

    /// Тестовый лимитер с явными параметрами (юнит-тесты).
    pub fn with(cfg: RateLimit) -> Limiter {
        Limiter {
            cfg,
            buckets: Mutex::new(HashMap::new()),
            login: Mutex::new(HashMap::new()),
            user_buckets: Mutex::new(HashMap::new()),
            user_inflight: Mutex::new(HashMap::new()),
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

    /// Частота **на пользователя** (`requests_per_min_per_user`; M4 T4).
    ///
    /// Нужна при `rate_limit.per_token: true`: пер-токенный ключ частоты
    /// сохраняем (совместимость M3), но у пользователя с двумя токенами
    /// суммарная частота не должна удваиваться (регресс F10.3).
    pub fn check_user(&self, key: &str) -> Option<u64> {
        let rate = self.cfg.requests_per_min_per_user as f64 / 60.0;
        self.tick(&self.user_buckets, key, self.cfg.burst as f64, rate)
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

    /// Занять **пер-пользовательский** слот параллелизма (M4 T4 §3.3);
    /// `false` — предел пользователя.
    ///
    /// Эффективный предел — [`Limiter::user_inflight_limit`] (строгий из
    /// `max_inflight_per_user` и глобального `max_inflight`). Используется на
    /// пути без очереди (desktop / `queue.enabled: false`): 3-й одновременный
    /// `ask_my_files` одного пользователя → `429` с `hint` про
    /// пер-пользовательский предел. В server-профиле очередь (`can_admit`)
    /// применяет тот же предел, чтобы отказать сразу.
    pub fn user_inflight_acquire(&self, key: &str) -> bool {
        let limit = self.user_inflight_limit();
        let mut m = self.user_inflight.lock().unwrap_or_else(|e| e.into_inner());
        let n = m.entry(key.to_string()).or_insert(0);
        if *n >= limit {
            false
        } else {
            *n += 1;
            true
        }
    }

    /// Эффективный пер-пользовательский предел параллелизма (M4 T4 §3.3):
    /// строгий из `max_inflight_per_user` и глобального `max_inflight` —
    /// пользователю нельзя дать больше, чем разрешено исполнять вообще.
    pub fn user_inflight_limit(&self) -> u32 {
        self.cfg.max_inflight_per_user.min(self.cfg.max_inflight)
    }

    /// Отпустить пер-пользовательский слот (в паре с `user_inflight_acquire`).
    pub fn user_inflight_release(&self, key: &str) {
        let mut m = self.user_inflight.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(key);
            }
        }
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
            max_inflight_per_user: super::DEFAULT_MAX_INFLIGHT_PER_USER,
            requests_per_min_per_user: rpm,
            per_token: false,
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
        // F3 (M5 T0.3): дефолт server = 5 — как дефолт `llm_server.parallel`,
        // лимит исполнения не меньше слотов чата; desktop — прежние 4.
        assert_eq!(l.cfg().max_inflight, 5);
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

    /// Параллелизм: до пер-пользовательского предела — да, дальше нет;
    /// release возвращает слот. F6 (M5 T0.6): тест переведён на рантайм-путь
    /// `user_inflight_*` (мёртвый `inflight_*`/`InflightGuard` удалён).
    #[test]
    fn user_inflight_capped_and_release_returns_slot() {
        let l = Arc::new(lim(600, 50, 50, 2)); // предел пользователя min(2, 2) = 2
        assert!(l.user_inflight_acquire("u"));
        assert!(l.user_inflight_acquire("u"));
        assert!(!l.user_inflight_acquire("u"), "третий параллельный — 429");
        assert!(
            l.user_inflight_acquire("u2"),
            "другой пользователь — свой предел"
        );
        l.user_inflight_release("u");
        assert!(l.user_inflight_acquire("u"), "после release слот свободен");
        l.user_inflight_release("u");
        l.user_inflight_release("u2");
    }

    /// Частота и параллелизм — независимые измерения (F6: путь `user_inflight_*`).
    #[test]
    fn frequency_and_inflight_independent() {
        let l = Arc::new(lim(600, 50, 50, 1)); // предел пользователя min(2, 1) = 1
        assert!(l.check("u").is_none());
        assert!(l.user_inflight_acquire("u"));
        // Частота не заблокирована inflight-слотом...
        assert!(l.check("u").is_none());
        // ...а слот параллелизма исчерпан.
        assert!(!l.user_inflight_acquire("u"), "второй inflight — 429");
        l.user_inflight_release("u");
        assert!(l.user_inflight_acquire("u"), "после release слот свободен");
    }

    /// M4 T4 (§3.3): дефолты пер-пользовательских ключей и `per_token`
    /// (server: 2 / 120 / false; мусорные значения — дефолты).
    #[test]
    fn m4_per_user_defaults_and_per_token() {
        let l = Limiter::from_config(&cfg("profile: server")).expect("server");
        let c = l.cfg();
        assert_eq!(
            c.max_inflight_per_user, 2,
            "дефолт пер-пользовательского предела"
        );
        assert_eq!(
            c.max_inflight, 5,
            "глобальный предел — из max_inflight (дефолт server 5, F3)"
        );
        assert_eq!(c.requests_per_min_per_user, c.requests_per_min);
        assert!(!c.per_token, "по умолчанию ключ — пользователь, не токен");

        // Явные значения сильнее; нули/мусор — дефолт.
        let custom = Limiter::from_config(&cfg(
            "profile: server\nrate_limit:\n  max_inflight_per_user: 5\n  \
             requests_per_min_per_user: 7\n  per_token: true\n",
        ))
        .unwrap();
        assert_eq!(custom.cfg().max_inflight_per_user, 5);
        assert_eq!(custom.cfg().requests_per_min_per_user, 7);
        assert!(custom.cfg().per_token);

        let junk = Limiter::from_config(&cfg(
            "profile: server\nrate_limit:\n  max_inflight_per_user: 0\n  \
             requests_per_min_per_user: -3\n",
        ))
        .unwrap();
        assert_eq!(junk.cfg().max_inflight_per_user, 2, "0/мусор → дефолт");
        assert_eq!(junk.cfg().requests_per_min_per_user, 120, "мусор → как rpm");
    }

    /// M4 T4 (F10.3): пер-пользовательская частота общая для всех токенов
    /// пользователя — второй «токен» не получает свежий bucket.
    #[test]
    fn user_frequency_bucket_is_shared() {
        let l = lim(60, 2, 5, 4); // requests_per_min_per_user = 60 (см. `lim`)
        let user = "user:anna";
        assert!(l.check_user(user).is_none());
        assert!(l.check_user(user).is_none());
        assert!(
            l.check_user(user).is_some(),
            "третья попытка пользователя — предел"
        );
        // Другой пользователь — свой bucket.
        assert!(l.check_user("user:boris").is_none());
    }

    /// M4 T4 (§3.3): пер-пользовательский параллелизм (`max_inflight_per_user`)
    /// ниже глобального; release возвращает слот.
    #[test]
    fn user_inflight_capped_and_released() {
        let l = Limiter::with(RateLimit {
            enabled: true,
            requests_per_min: 600,
            burst: 50,
            login_per_min: 50,
            max_inflight: 4,
            max_inflight_per_user: 2,
            requests_per_min_per_user: 600,
            per_token: false,
        });
        assert!(l.user_inflight_acquire("user:anna"));
        assert!(l.user_inflight_acquire("user:anna"));
        assert!(
            !l.user_inflight_acquire("user:anna"),
            "3-й параллельный RAG пользователя — отказ"
        );
        // Другой пользователь — свой счётчик.
        assert!(l.user_inflight_acquire("user:boris"));
        l.user_inflight_release("user:anna");
        assert!(l.user_inflight_acquire("user:anna"), "release вернул слот");
        // Освобождение незнакомого ключа — без паники.
        l.user_inflight_release("user:nobody");
    }
}
