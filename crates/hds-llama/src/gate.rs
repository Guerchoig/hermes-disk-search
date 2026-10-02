//! Шлюз доступа к движку: «кто держит движок и с какого времени» + ожидание с бюджетом.
//!
//! Зачем отдельный тип, а не просто `Mutex<Cluster>` (как было в `host::ClusterShared`):
//! * **наблюдаемость** — при зависшем вызове движка (`tools/parity/W4_REPORT.md` §14)
//!   снаружи не было видно, кто держит движок: `status`, `/internal/status`, арбитр и
//!   роли сами ждали тот же мьютекс и превращались в «резидент не отвечает»;
//! * **ограниченное ожидание** — [`Gate::try_with`] отдаёт [`Busy`] вместо бесконечного
//!   ожидания, поэтому статус, heartbeat и роли могут честно ответить
//!   «движок занят ролью X, T с», а не висеть до перезапуска;
//! * **тестируемость** — гейт — обычный `Gate<()>`/`Gate<u32>`, проверяется без движка
//!   и без GPU (`cargo test -p hds-llama --lib gate`).
//!
//! Инварианты:
//! * метка занятости ставится **после** захвата (то есть описывает именно держателя),
//!   снимается через RAII — даже если вызов движка паникует;
//! * `try_with` никогда не ждёт дольше бюджета и в `Err` возвращает **снимок держателя**;
//! * отравленный (poisoned) мьютекс не блокирует насмерть: значение берётся как есть —
//!   паника одного вызова движка не должна выключать наблюдаемость навсегда.

use std::fmt;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

/// Кто и с какого времени держит движок (снимок для `status`/heartbeat/логов).
#[derive(Debug, Clone)]
pub struct Busy {
    /// Что именно держит движок: роль (`embedding`) или служебная операция (`arbiter`).
    pub what: String,
    /// Когда вызов начался (включая время ожидания мьютекса).
    pub since: Instant,
}

impl Busy {
    /// Сколько секунд вызов уже держится.
    pub fn secs(&self) -> u64 {
        self.since.elapsed().as_secs()
    }
}

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} — {} с", self.what, self.secs())
    }
}

/// Сколько ждать между попытками захвата в [`Gate::try_with`].
const POLL: Duration = Duration::from_millis(10);

/// Разделяемый ресурс (движок) с меткой занятости и ожиданием по бюджету.
#[derive(Debug)]
pub struct Gate<T> {
    inner: Mutex<T>,
    /// Держатель ресурса (`None` — свободен). Заполняет только владелец.
    busy: Mutex<Option<Busy>>,
    /// Порог «долгого» вызова в миллисекундах (`0` — не измерять).
    slow_ms: AtomicU64,
    /// Последний долгий вызов: `(что, миллисекунды)` — печатает `status`/heartbeat.
    last_slow: Mutex<Option<(String, u64)>>,
}

impl<T> Gate<T> {
    /// Новый шлюз над значением (движком).
    pub fn new(value: T) -> Gate<T> {
        Gate {
            inner: Mutex::new(value),
            busy: Mutex::new(None),
            slow_ms: AtomicU64::new(0),
            last_slow: Mutex::new(None),
        }
    }

    /// Порог логирования долгих вызовов (мс; `0` — выключить).
    pub fn set_slow_ms(&self, ms: u64) {
        self.slow_ms.store(ms, Ordering::Relaxed);
    }

    /// Последний долгий вызов `(что, мс)` — для логов/статуса.
    pub fn last_slow(&self) -> Option<(String, u64)> {
        self.last_slow.lock().ok().and_then(|v| v.clone())
    }

    /// Кто держит движок сейчас (`None` — свободен).
    pub fn busy(&self) -> Option<Busy> {
        self.busy.lock().ok().and_then(|b| b.clone())
    }

    /// Занят ли движок прямо сейчас.
    pub fn is_busy(&self) -> bool {
        self.busy.lock().map(|b| b.is_some()).unwrap_or(false)
    }

    /// Вызвать движок, помечая занятость (`what` — роль/операция).
    pub fn with_tagged<R>(&self, what: &str, f: impl FnOnce(&T) -> R) -> R {
        let held = self.hold(what);
        f(&held)
    }

    /// То же, но с бюджетом ожидания: `Err(Busy)` — не дождались за `budget`.
    ///
    /// Возвращаемый `Busy` описывает **держателя** (а не нас) — это и есть ответ на
    /// вопрос «кто кого ждёт».
    pub fn try_with<R>(
        &self,
        what: &str,
        budget: Duration,
        f: impl FnOnce(&T) -> R,
    ) -> Result<R, Busy> {
        let t0 = Instant::now();
        let deadline = t0 + budget;
        loop {
            match self.inner.try_lock() {
                Ok(guard) => {
                    let held = Held::new(self, what, t0, guard);
                    return Ok(f(&held));
                }
                Err(TryLockError::Poisoned(err)) => {
                    // Паника внутри вызова движка не должна выключать наблюдаемость:
                    // берём значение как есть (как это делал прежний `with`).
                    let guard = err.into_inner();
                    let held = Held::new(self, what, t0, guard);
                    return Ok(f(&held));
                }
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(self.busy().unwrap_or(Busy {
                            what: what.to_string(),
                            since: t0,
                        }));
                    }
                    std::thread::sleep(POLL);
                }
            }
        }
    }

    /// Захватить ресурс (блокирующе) и пометить занятость. Возвращённый [`Held`]
    /// снимает метку при уничтожении — в том числе при панике в вызванном коде.
    fn hold(&self, what: &str) -> Held<'_, T> {
        let t0 = Instant::now();
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(err) => err.into_inner(),
        };
        Held::new(self, what, t0, guard)
    }

    /// Мы стали держателем — публикуем метку (чужую не перетираем).
    fn set_busy(&self, what: &str, t0: Instant) {
        if let Ok(mut slot) = self.busy.lock() {
            if slot.is_none() {
                *slot = Some(Busy {
                    what: what.to_string(),
                    since: t0,
                });
            }
        }
    }

    /// Вызов завершён: снимаем метку и, если вызов был долгим, запоминаем его.
    fn finish(&self, what: &str, t0: Instant) {
        if let Ok(mut slot) = self.busy.lock() {
            *slot = None;
        }
        let ms = t0.elapsed().as_millis() as u64;
        let threshold = self.slow_ms.load(Ordering::Relaxed);
        if threshold > 0 && ms >= threshold {
            if let Ok(mut last) = self.last_slow.lock() {
                *last = Some((what.to_string(), ms));
            }
        }
    }
}

/// Владение ресурсом: держит мьютекс и метку занятости до конца вызова.
#[derive(Debug)]
struct Held<'a, T> {
    gate: &'a Gate<T>,
    what: String,
    started: Instant,
    guard: Option<MutexGuard<'a, T>>,
}

impl<'a, T> Held<'a, T> {
    fn new(
        gate: &'a Gate<T>,
        what: &str,
        started: Instant,
        guard: MutexGuard<'a, T>,
    ) -> Held<'a, T> {
        gate.set_busy(what, started);
        Held {
            gate,
            what: what.to_string(),
            started,
            guard: Some(guard),
        }
    }
}

impl<T> Deref for Held<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // `guard` снимается только в `Drop`, поэтому здесь он всегда есть
        self.guard.as_deref().expect("guarded value жив до Drop")
    }
}

impl<T> Drop for Held<'_, T> {
    fn drop(&mut self) {
        // порядок важен: сначала снимаем метку (пока держим мьютекс),
        // затем отпускаем сам мьютекс — иначе окно «свободен, но помечен занятым»
        self.gate.finish(&self.what, self.started);
        self.guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn busy_is_visible_while_held() {
        let gate = Arc::new(Gate::new(()));
        let holder = Arc::clone(&gate);
        let h = std::thread::spawn(move || {
            holder.with_tagged("embedding", |_| {
                std::thread::sleep(Duration::from_millis(300))
            })
        });
        std::thread::sleep(Duration::from_millis(60));
        let busy = gate.busy().expect("движок должен быть помечен занятым");
        assert_eq!(busy.what, "embedding");
        assert!(busy.secs() < 5, "счётчик секунд не должен врать");
        assert!(gate.is_busy());
        h.join().unwrap();
        assert!(gate.busy().is_none(), "метка снимается после вызова");
    }

    #[test]
    fn try_with_reports_holder_on_timeout() {
        let gate = Arc::new(Gate::new(()));
        let holder = Arc::clone(&gate);
        let h = std::thread::spawn(move || {
            holder.with_tagged("embeddings", |_| {
                std::thread::sleep(Duration::from_millis(250))
            })
        });
        std::thread::sleep(Duration::from_millis(50));
        let err = gate
            .try_with("status", Duration::from_millis(50), |_| ())
            .expect_err("бюджет должен истечь");
        assert_eq!(err.what, "embeddings", "виден именно держатель");
        h.join().unwrap();
    }

    #[test]
    fn try_with_succeeds_when_free_and_returns_value() {
        let gate = Gate::new(41u32);
        let got = gate
            .try_with("status", Duration::from_millis(200), |v| *v + 1)
            .expect("на свободном движке ожидание не нужно");
        assert_eq!(got, 42);
        assert!(gate.busy().is_none());
    }

    #[test]
    fn last_slow_records_long_call() {
        let gate = Gate::new(());
        gate.set_slow_ms(1);
        gate.with_tagged("chat", |_| std::thread::sleep(Duration::from_millis(15)));
        let (what, ms) = gate.last_slow().expect("долгий вызов должен быть записан");
        assert_eq!(what, "chat");
        assert!(ms >= 1, "миллисекунды не должны быть нулевыми: {ms}");
    }

    #[test]
    fn slow_below_threshold_is_not_recorded() {
        let gate = Gate::new(());
        gate.set_slow_ms(60_000);
        gate.with_tagged("chat", |_| ());
        assert!(gate.last_slow().is_none());
    }

    #[test]
    fn poison_is_recovered_and_busy_cleared() {
        let gate = Gate::new(0u32);
        let g = &gate;
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            g.with_tagged("chat", |_| panic!("движок упал"));
        }));
        assert!(panicked.is_err(), "паника должна пройти наружу");
        assert!(
            gate.busy().is_none(),
            "метка снимается даже при панике (RAII)"
        );
        let got = gate
            .try_with("status", Duration::from_millis(200), |v| *v)
            .expect("отравленный мьютекс не должен блокировать наблюдаемость");
        assert_eq!(got, 0);
    }
}
