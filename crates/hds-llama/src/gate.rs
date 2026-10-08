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
//! M4 T2 (PLAN_M4_CONCURRENCY_SLA.md §3.1): два режима доступа.
//!
//! * **эксклюзивный** (`with_tagged`/`try_with`) — управление кластером: загрузка/
//!   выгрузка ролей, арбитр, диспетчер, наблюдение. Как прежде, один держатель.
//! * **разделяемый** (`with_shared_tagged`/`try_with_shared`) — **генерация** (chat/
//!   embeddings/rerank): вызовы идут параллельно, внутри движка их распределяют
//!   слоты инстанса (патч P1: `wait_for_instance_slot` с дедлайном). Один мьютекс
//!   на всю генерацию сводил `llm_server.parallel > 1` к нулю: 5 запросов шли
//!   строго по очереди (замер 08.10.2026, `tools/parity/out/m4_slots_*.json`).
//!
//! Инварианты:
//! * метка занятости ставится **после** захвата (то есть описывает именно держателя),
//!   снимается через RAII — даже если вызов движка паникует;
//! * `try_with`/`try_with_shared` никогда не ждут дольше бюджета и в `Err` возвращают
//!   **снимок держателя** (у разделяемых — с числом держателей в `Busy::holders`);
//! * «отравленный» лок не блокирует насмерть: значение берётся как есть — паника
//!   одного вызова движка не должна выключать наблюдаемость навсегда.

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::{Duration, Instant};

/// Кто и с какого времени держит движок (снимок для `status`/heartbeat/логов).
#[derive(Debug, Clone)]
pub struct Busy {
    /// Что именно держит движок: роль (`embedding`) или служебная операция (`arbiter`).
    pub what: String,
    /// Когда вызов начался (включая время ожидания лока).
    pub since: Instant,
    /// Число одновременных держателей: `1` — эксклюзивно, `>1` — параллельная
    /// генерация (M4 T2). Отдельное поле, чтобы страж брошенных запросов и
    /// heartbeat продолжали сравнивать `what` с меткой `engine` как прежде.
    pub holders: u64,
}

impl Busy {
    /// Сколько секунд вызов уже держится.
    pub fn secs(&self) -> u64 {
        self.since.elapsed().as_secs()
    }
}

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.holders > 1 {
            write!(
                f,
                "{} ×{} держателей — {} с",
                self.what,
                self.holders,
                self.secs()
            )
        } else {
            write!(f, "{} — {} с", self.what, self.secs())
        }
    }
}

/// Сколько ждать между попытками захвата в [`Gate::try_with`].
const POLL: Duration = Duration::from_millis(10);

/// Разделяемый ресурс (движок) с меткой занятости и ожиданием по бюджету.
///
/// Внутри — `RwLock`: **генерация** берёт read-лок (параллельно, M4 T2),
/// **управление кластером** — write-лок (как прежде, один держатель).
#[derive(Debug)]
pub struct Gate<T> {
    inner: RwLock<T>,
    /// Эксклюзивный держатель (write-лок): управление кластером (`None` — свободен).
    exclusive: Mutex<Option<Busy>>,
    /// Параллельные держатели генерации (read-лок): id → (что, с какого времени).
    shared: Mutex<HashMap<u64, (String, Instant)>>,
    /// Счётчик id для [`Gate::shared`].
    shared_seq: AtomicU64,
    /// Порог «долгого» вызова в миллисекундах (`0` — не измерять).
    slow_ms: AtomicU64,
    /// Последний долгий вызов: `(что, миллисекунды)` — печатает `status`/heartbeat.
    last_slow: Mutex<Option<(String, u64)>>,
}

impl<T> Gate<T> {
    /// Новый шлюз над значением (движком).
    pub fn new(value: T) -> Gate<T> {
        Gate {
            inner: RwLock::new(value),
            exclusive: Mutex::new(None),
            shared: Mutex::new(HashMap::new()),
            shared_seq: AtomicU64::new(0),
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
    ///
    /// Приоритет — эксклюзивный держатель (управление); иначе — сводка по
    /// параллельным держателям генерации (метка старейшего + число держателей).
    pub fn busy(&self) -> Option<Busy> {
        if let Ok(slot) = self.exclusive.lock() {
            if let Some(b) = slot.as_ref() {
                return Some(Busy {
                    what: b.what.clone(),
                    since: b.since,
                    holders: 1,
                });
            }
        }
        self.shared_busy()
    }

    /// Сводка по параллельным держателям генерации (`None` — их нет).
    fn shared_busy(&self) -> Option<Busy> {
        let map = self.shared.lock().ok()?;
        let oldest = map
            .values()
            .min_by_key(|(_, since)| *since)
            .map(|(what, since)| (what.clone(), *since))?;
        Some(Busy {
            what: oldest.0,
            since: oldest.1,
            holders: map.len() as u64,
        })
    }

    /// Занят ли движок прямо сейчас.
    pub fn is_busy(&self) -> bool {
        self.busy().is_some()
    }

    /// Вызвать движок, помечая занятость (`what` — роль/операция).
    ///
    /// Эксклюзивный доступ: управление кластером (загрузка/выгрузка, арбитр,
    /// наблюдение). Для параллельной генерации — [`Gate::with_shared_tagged`].
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
            match self.inner.try_write() {
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
                            holders: 1,
                        }));
                    }
                    std::thread::sleep(POLL);
                }
            }
        }
    }

    /// **Разделяемый** доступ для генерации (M4 T2): вызовы идут параллельно,
    /// слоты инстанса движка распределяют их (патч P1 — ожидание слота с дедлайном).
    pub fn with_shared_tagged<R>(&self, what: &str, f: impl FnOnce(&T) -> R) -> R {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(err) => err.into_inner(),
        };
        let id = self.shared_register(what);
        let held = HeldShared {
            gate: self,
            what: what.to_string(),
            started: Instant::now(),
            id,
            guard: Some(guard),
        };
        f(&held)
    }

    /// Разделяемый доступ с бюджетом ожидания: `Err(Busy)` — не дождались за
    /// `budget` (обычно это значит, что идёт управление кластером — держатель
    /// виден в [`Gate::busy`]).
    pub fn try_with_shared<R>(
        &self,
        what: &str,
        budget: Duration,
        f: impl FnOnce(&T) -> R,
    ) -> Result<R, Busy> {
        let t0 = Instant::now();
        let deadline = t0 + budget;
        loop {
            match self.inner.try_read() {
                Ok(guard) => {
                    let id = self.shared_register(what);
                    let held = HeldShared {
                        gate: self,
                        what: what.to_string(),
                        started: t0,
                        id,
                        guard: Some(guard),
                    };
                    return Ok(f(&held));
                }
                Err(TryLockError::Poisoned(err)) => {
                    let id = self.shared_register(what);
                    let held = HeldShared {
                        gate: self,
                        what: what.to_string(),
                        started: t0,
                        id,
                        guard: Some(err.into_inner()),
                    };
                    return Ok(f(&held));
                }
                Err(TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        return Err(self.busy().unwrap_or(Busy {
                            what: what.to_string(),
                            since: t0,
                            holders: 1,
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
        let guard = match self.inner.write() {
            Ok(g) => g,
            Err(err) => err.into_inner(),
        };
        Held::new(self, what, t0, guard)
    }

    /// Мы стали эксклюзивным держателем — публикуем метку (чужую не перетираем).
    fn set_exclusive(&self, what: &str, t0: Instant) {
        if let Ok(mut slot) = self.exclusive.lock() {
            if slot.is_none() {
                *slot = Some(Busy {
                    what: what.to_string(),
                    since: t0,
                    holders: 1,
                });
            }
        }
    }

    /// Зарегистрировать параллельного держателя генерации (id для снятия метки).
    fn shared_register(&self, what: &str) -> u64 {
        let id = self.shared_seq.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut map) = self.shared.lock() {
            map.insert(id, (what.to_string(), Instant::now()));
        }
        id
    }

    /// Эксклюзивный вызов завершён: снимаем метку и, если вызов был долгим, помним его.
    fn finish_exclusive(&self, what: &str, t0: Instant) {
        if let Ok(mut slot) = self.exclusive.lock() {
            *slot = None;
        }
        self.note_slow(what, t0);
    }

    /// Параллельный держатель завершён: снимаем свою запись.
    fn finish_shared(&self, id: u64, what: &str, t0: Instant) {
        if let Ok(mut map) = self.shared.lock() {
            map.remove(&id);
        }
        self.note_slow(what, t0);
    }

    /// Долгий вызов — в `last_slow` (для `status`/heartbeat).
    fn note_slow(&self, what: &str, t0: Instant) {
        let ms = t0.elapsed().as_millis() as u64;
        let threshold = self.slow_ms.load(Ordering::Relaxed);
        if threshold > 0 && ms >= threshold {
            if let Ok(mut last) = self.last_slow.lock() {
                *last = Some((what.to_string(), ms));
            }
        }
    }
}

/// Владение ресурсом (эксклюзивное): держит write-лок и метку занятости до конца вызова.
#[derive(Debug)]
struct Held<'a, T> {
    gate: &'a Gate<T>,
    what: String,
    started: Instant,
    guard: Option<RwLockWriteGuard<'a, T>>,
}

impl<'a, T> Held<'a, T> {
    fn new(
        gate: &'a Gate<T>,
        what: &str,
        started: Instant,
        guard: RwLockWriteGuard<'a, T>,
    ) -> Held<'a, T> {
        gate.set_exclusive(what, started);
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
        // порядок важен: сначала снимаем метку (пока держим лок),
        // затем отпускаем сам лок — иначе окно «свободен, но помечен занятым»
        self.gate.finish_exclusive(&self.what, self.started);
        self.guard = None;
    }
}

/// Владение ресурсом (разделяемое, M4 T2): держит read-лок и свою запись в
/// `shared` до конца вызова — метка занятости суммируется по держателям.
#[derive(Debug)]
struct HeldShared<'a, T> {
    gate: &'a Gate<T>,
    what: String,
    started: Instant,
    id: u64,
    guard: Option<RwLockReadGuard<'a, T>>,
}

impl<T> Deref for HeldShared<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.guard.as_deref().expect("guarded value жив до Drop")
    }
}

impl<T> Drop for HeldShared<'_, T> {
    fn drop(&mut self) {
        self.gate.finish_shared(self.id, &self.what, self.started);
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
            .expect("отравленный лок не должен блокировать наблюдаемость");
        assert_eq!(got, 0);
    }

    /// M4 T2: разделяемый режим пускает вызовы параллельно (раньше мьютекс
    /// сериализовал генерацию — слоты движка простаивали).
    #[test]
    fn shared_calls_run_concurrently() {
        let gate = Arc::new(Gate::new(()));
        let t0 = Instant::now();
        let hop = |g: Arc<Gate<()>>| {
            std::thread::spawn(move || {
                g.with_shared_tagged("engine", |_| std::thread::sleep(Duration::from_millis(300)))
            })
        };
        let a = hop(Arc::clone(&gate));
        let b = hop(Arc::clone(&gate));
        let c = hop(Arc::clone(&gate));
        std::thread::sleep(Duration::from_millis(80));
        let busy = gate.busy().expect("параллельные держатели видны");
        assert_eq!(busy.what, "engine", "метка держателя — как у генерации");
        assert_eq!(busy.holders, 3, "видны все три параллельных вызова");
        assert!(gate.is_busy());
        a.join().unwrap();
        b.join().unwrap();
        c.join().unwrap();
        assert!(
            t0.elapsed() < Duration::from_millis(800),
            "три вызова по 300 мс параллельно должны уложиться заметно быстрее \
             последовательных 900 мс: {:?}",
            t0.elapsed()
        );
        assert!(gate.busy().is_none(), "метка снимается после всех вызовов");
    }

    /// Эксклюзивному (управление кластером) не пробиться через генерацию:
    /// `Err(Busy)` с числом держателей — и он не ждёт дольше бюджета.
    #[test]
    fn exclusive_sees_shared_holders_as_busy() {
        let gate = Arc::new(Gate::new(()));
        let holder = Arc::clone(&gate);
        let h = std::thread::spawn(move || {
            holder.with_shared_tagged("engine", |_| std::thread::sleep(Duration::from_millis(300)))
        });
        std::thread::sleep(Duration::from_millis(60));
        let t0 = Instant::now();
        let err = gate
            .try_with("dispatcher", Duration::from_millis(50), |_| ())
            .expect_err("генерация держит лок — управление не проходит");
        assert_eq!(err.what, "engine");
        assert_eq!(err.holders, 1);
        assert!(
            t0.elapsed() < Duration::from_millis(250),
            "ждали не дольше бюджета"
        );
        h.join().unwrap();
    }

    /// Обратная сторона: пока идёт управление (write-лок), генерация с бюджетом
    /// получает честный `Busy` с меткой держателя, а не висит.
    #[test]
    fn shared_sees_exclusive_holder_as_busy() {
        let gate = Arc::new(Gate::new(()));
        let holder = Arc::clone(&gate);
        let h = std::thread::spawn(move || {
            holder.with_tagged("load:chat", |_| {
                std::thread::sleep(Duration::from_millis(300))
            })
        });
        std::thread::sleep(Duration::from_millis(60));
        let err = gate
            .try_with_shared("engine", Duration::from_millis(50), |_| ())
            .expect_err("идёт загрузка роли — генерация не стартует");
        assert_eq!(err.what, "load:chat");
        assert_eq!(err.holders, 1);
        h.join().unwrap();
    }
}
