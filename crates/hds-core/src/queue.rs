//! Очередь тяжёлых RAG-запросов с fair-share по пользователю (M4 T3,
//! `PLAN_M4_CONCURRENCY_SLA.md` §3.2).
//!
//! До M4 при насыщении `rate_limit.max_inflight` тяжёлый `ask_my_files` сразу
//! получал `429` — при 5 пользователях это отказ вместо ожидания (нужен SLA
//! ≤5 мин на ответ). Очередь **ждёт** освобождения слота:
//! **round-robin между пользователями, FIFO внутри пользователя** (политика
//! `queue.policy: rr|fifo`), глубина ожидания — `queue.max_depth`, потолок
//! ожидания — `queue.wait_timeout_sec`.
//!
//! Разделение ответственности: `rate_limit.max_inflight` — лимит **исполнения**
//! (сколько RAG считается прямо сейчас), `queue.max_depth` — лимит **ожидания**.
//! Коды: очередь переполнена → `429`, не дождался слота → `503`, обрыв клиента
//! освобождает место в очереди (проверка живости соединения — вызывающий код).
//!
//! Очередь живёт в процессе MCP-http (документированное ограничение: админский
//! `ask` веб-UI лимитируется своими лимитами M3). Метрики наружу (`hds check`,
//! `/api/diagnostics` — это **другие** процессы) отдаются файлом-снапшотом
//! `<state_dir>/data/mcp-queue.json`: атомарная запись (tmp + rename) и поле
//! `ts`, как у `index.heartbeat.json` — читатель различает «живую» очередь и
//! «статус никто не пишет».

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::config::{dig, Config};

/// Дефолт `queue.max_depth` — сколько запросов ЖДУТ исполнения.
pub const DEFAULT_MAX_DEPTH: u32 = 20;

/// Дефолт `queue.wait_timeout_sec` — потолок ожидания слота.
///
/// Меньше таймаута MCP-клиента ([`CLIENT_TIMEOUT_SEC`]): иначе `503` приходит
/// ровно в момент, когда клиент уже оборвал запрос, и клиент видит обрыв вместо
/// честного кода (`PLAN_M4_CONCURRENCY_SLA.md` §7).
pub const DEFAULT_WAIT_TIMEOUT_SEC: u64 = 240;

/// Таймаут MCP-клиента (Cline/Hermes), секунды (см. `chat.rag_max_tokens`).
/// `queue.wait_timeout_sec` должен быть строго меньше.
pub const CLIENT_TIMEOUT_SEC: u64 = 300;

/// Дефолтная политика очереди — round-robin между пользователями.
pub const DEFAULT_POLICY: QueuePolicy = QueuePolicy::RoundRobin;

/// Возраст снапшота (с), после которого статус очереди считается устаревшим:
/// писатель обновляет файл раз в 2 с.
pub const SNAPSHOT_STALE_SEC: f64 = 10.0;

/// Срез ожидания: между срезами проверяем живость соединения клиента.
const WAIT_SLICE: Duration = Duration::from_millis(150);

/// Окно последних ожиданий для p50/p95 (метрики в снапшоте).
const WAITS_WINDOW: usize = 256;

/// Политика распределения слотов между пользователями.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePolicy {
    /// Round-robin между пользователями, FIFO внутри пользователя (дефолт):
    /// «жадный» пользователь не занимает подряд все слоты, пока ждут другие.
    RoundRobin,
    /// Строгий FIFO по всей очереди (раньше пришёл — раньше поехал).
    Fifo,
}

impl QueuePolicy {
    /// Разбор `queue.policy` (`rr`/`round-robin`/`fifo`; `None` — неизвестное).
    pub fn parse(s: &str) -> Option<QueuePolicy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rr" | "round_robin" | "round-robin" => Some(QueuePolicy::RoundRobin),
            "fifo" => Some(QueuePolicy::Fifo),
            _ => None,
        }
    }

    /// Значение для конфига/диагностики.
    pub fn as_str(&self) -> &'static str {
        match self {
            QueuePolicy::RoundRobin => "rr",
            QueuePolicy::Fifo => "fifo",
        }
    }
}

/// Параметры очереди из конфига (`queue: {...}` / `rate_limit.max_inflight`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueConfig {
    /// Включена ли очередь (`queue.enabled`; без ключа — по профилю:
    /// server → true, desktop → false = поведение M3 «сразу 429»).
    pub enabled: bool,
    /// Сколько запросов могут ЖДАТЬ (исполнение ограничивает `max_inflight`).
    pub max_depth: u32,
    /// Потолок ожидания слота, секунды (0 — ждать нельзя: только быстрый путь).
    pub wait_timeout_sec: u64,
    /// Политика распределения между пользователями.
    pub policy: QueuePolicy,
    /// Лимит ИСПОЛНЕНИЯ (одновременных тяжёлых запросов) —
    /// `rate_limit.max_inflight` (единый дефолт с лимитером M3).
    pub max_inflight: u32,
    /// Предел одновременных RAG **на пользователя** (M4 T4,
    /// `rate_limit.max_inflight_per_user`): «жадный» пользователь не забирает
    /// все слоты; превышение → `429` сразу (`QueueReject::PerUser`).
    /// Учитывается, только если он строже глобального (`max_inflight`).
    pub max_inflight_per_user: u32,
}

impl QueueConfig {
    /// Параметры очереди из конфига (дефолты — по профилю, как в M2/M3).
    pub fn from_config(cfg: &Config) -> QueueConfig {
        QueueConfig {
            enabled: dig(cfg, "queue.enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| crate::config::profile(cfg).is_server()),
            max_depth: dig(cfg, "queue.max_depth")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(DEFAULT_MAX_DEPTH),
            wait_timeout_sec: dig(cfg, "queue.wait_timeout_sec")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_WAIT_TIMEOUT_SEC),
            policy: dig(cfg, "queue.policy")
                .and_then(|v| v.as_str())
                .and_then(QueuePolicy::parse)
                .unwrap_or(DEFAULT_POLICY),
            max_inflight: crate::ratelimit::RateLimit::from_config(cfg).max_inflight,
            max_inflight_per_user: {
                let r = crate::ratelimit::RateLimit::from_config(cfg);
                // Эффективный предел: не выше глобального `max_inflight`.
                r.max_inflight_per_user.min(r.max_inflight)
            },
        }
    }

    /// Потолок ожидания как `Duration`.
    pub fn wait_timeout(&self) -> Duration {
        Duration::from_secs(self.wait_timeout_sec)
    }
}

/// Заявки одного пользователя: ключи ожидающих (глобальные seq) + счётчик
/// его исполняющихся запросов (для пер-пользовательской картины в снапшоте).
#[derive(Debug, Default)]
struct UserQueue {
    waiting: VecDeque<u64>,
    executing: u32,
}

/// Состояние очереди под одним мьютексом.
#[derive(Debug, Default)]
struct State {
    users: HashMap<String, UserQueue>,
    /// RR-курсор: пользователи, у которых есть ожидающие (в порядке обхода).
    order: VecDeque<String>,
    /// Исполняется прямо сейчас (все пользователи).
    executing: u32,
    /// Счётчик arrival-порядка (seq выдаётся при постановке в очередь).
    seq: u64,
    granted: u64,
    rejected_full: u64,
    timeouts: u64,
    aborted: u64,
    /// Отказы по пер-пользовательскому пределу (M4 T4).
    rejected_user: u64,
    /// Окно последних завершённых ожиданий (мс) — p50/p95.
    waits_ms: VecDeque<u64>,
    total_wait_ms: u64,
}

impl State {
    /// Сколько запросов ждут (не считая исполняющихся).
    fn waiting_total(&self) -> usize {
        self.users.values().map(|u| u.waiting.len()).sum()
    }

    /// Кто следующий на слот по политике (rr — курсор, fifo — минимальный seq).
    fn next_user(&self, policy: QueuePolicy) -> Option<String> {
        match policy {
            QueuePolicy::RoundRobin => self.order.front().cloned(),
            QueuePolicy::Fifo => self
                .users
                .iter()
                .filter_map(|(u, q)| q.waiting.front().map(|s| (*s, u)))
                .min()
                .map(|(_, u)| u.clone()),
        }
    }
}

/// Причина отказа очереди (вызывающий код превращает её в HTTP-код).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueReject {
    /// Очередь полна (`queue.max_depth`) → `429`; `depth` — сколько ждало.
    Full { max_depth: u32, depth: u32 },
    /// Не дождался слота за `queue.wait_timeout_sec` → `503`; `depth` — сколько
    /// ждало в момент отказа (включая этот запрос).
    Timeout { waited_sec: u64, depth: u32 },
    /// Пер-пользовательский предел одновременных RAG (M4 T4,
    /// `rate_limit.max_inflight_per_user`) → `429` сразу (ждать не поможет:
    /// лимит на самого пользователя, а не на свободные слоты).
    PerUser { limit: u32 },
    /// Клиент оборвал соединение, пока ждал — ответ отправлять некуда.
    Aborted,
}

/// Очередь тяжёлых запросов (общая на процесс MCP-http).
///
/// Логика чистая (без сети): живость клиента передаётся замыканием `alive`,
/// поэтому ядро тестируется без сокетов.
#[derive(Debug)]
pub struct HeavyQueue {
    cfg: QueueConfig,
    state: Mutex<State>,
    cv: Condvar,
}

impl HeavyQueue {
    /// Очередь с явными параметрами (тесты/переиспользование).
    pub fn new(cfg: QueueConfig) -> HeavyQueue {
        HeavyQueue {
            cfg,
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
        }
    }

    /// Параметры очереди.
    pub fn cfg(&self) -> &QueueConfig {
        &self.cfg
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Может ли запрос `user` стартовать прямо сейчас: есть свободный слот и
    /// (очередь пуста либо `user` — следующий по политике).
    fn can_admit(&self, st: &State, user: &str) -> bool {
        if st.executing >= self.cfg.max_inflight {
            return false;
        }
        if st.waiting_total() == 0 {
            return true;
        }
        st.next_user(self.cfg.policy).as_deref() == Some(user)
    }

    /// Занять слот исполнения (ожидая при необходимости).
    ///
    /// * `user` — ключ fair-share (`user:<login>` / `ip:<addr>` / `local`);
    /// * `alive` — проверка живости клиента (вызывается между срезами ожидания;
    ///   `false` → [`QueueReject::Aborted`], место в очереди освобождается).
    ///   `FnMut`, потому что проверка трогает сокет (`peek`).
    ///
    /// Успех → [`QueueTicket`] (RAII: на `Drop` слот освобождается, следующего
    /// будит `notify_all`).
    pub fn acquire<F>(
        self: &Arc<Self>,
        user: &str,
        mut alive: F,
    ) -> Result<QueueTicket, QueueReject>
    where
        F: FnMut() -> bool,
    {
        let started = Instant::now();
        let deadline = started + self.cfg.wait_timeout();
        let mut enqueued: Option<u64> = None;
        let mut st = self.lock();
        loop {
            // 0) Пер-пользовательский предел (M4 T4 §3.3) — только для запроса,
            //    который ЕЩЁ НЕ в очереди: ждать бесполезно, это лимит на
            //    пользователя, а не на свободные слоты → сразу `429`.
            //    Уже стоящего в очереди не перепроверяем (до него дойдёт ход по
            //    fair-share, и он не поднимет число исполняющихся выше предела).
            //    Предел участвует, лишь когда он СТРОЖЕ глобального: иначе он
            //    ничего не добавляет (очередь работает как раньше — ждёт слот).
            if self.cfg.max_inflight_per_user < self.cfg.max_inflight
                && enqueued.is_none()
                && st.users.get(user).map(|q| q.executing).unwrap_or(0)
                    >= self.cfg.max_inflight_per_user
            {
                st.rejected_user += 1;
                drop(st);
                return Err(QueueReject::PerUser {
                    limit: self.cfg.max_inflight_per_user,
                });
            }
            // 1) Быстрый путь: слот свободен и мы — следующие (или очередь пуста).
            //    F4 (M5 T0.4): строгий FIFO ВНУТРИ пользователя — слот берёт
            //    заявка с минимальным `seq` этого пользователя: более ранние
            //    заявки СВОЕГО пользователя не обгоняем (иначе порядок внутри
            //    пользователя определяла гонка пробуждений).
            let ahead_of_me = match enqueued {
                Some(seq) => st
                    .users
                    .get(user)
                    .and_then(|q| q.waiting.front())
                    .is_some_and(|front| *front < seq),
                None => st.users.get(user).is_some_and(|q| !q.waiting.is_empty()),
            };
            if self.can_admit(&st, user) && !ahead_of_me {
                if let Some(seq) = enqueued.take() {
                    remove_waiter(&mut st, user, seq);
                    rotate_rr(&mut st, user);
                }
                st.executing += 1;
                st.granted += 1;
                st.users.entry(user.to_string()).or_default().executing += 1;
                let waited = started.elapsed();
                record_wait(&mut st, waited.as_millis() as u64);
                drop(st);
                return Ok(QueueTicket {
                    queue: Arc::clone(self),
                    user: user.to_string(),
                    waited,
                });
            }
            // 2) Постановка в очередь — один раз, с проверкой глубины.
            if enqueued.is_none() {
                let depth = st.waiting_total() as u32;
                if depth >= self.cfg.max_depth {
                    st.rejected_full += 1;
                    drop(st);
                    return Err(QueueReject::Full {
                        max_depth: self.cfg.max_depth,
                        depth,
                    });
                }
                st.seq += 1;
                let seq = st.seq;
                st.users
                    .entry(user.to_string())
                    .or_default()
                    .waiting
                    .push_back(seq);
                if !st.order.iter().any(|u| u == user) {
                    st.order.push_back(user.to_string());
                }
                enqueued = Some(seq);
                // F5 (M5 T0.5): слот мог освободиться между `release()` и нашей
                // постановкой — будим всех, чтобы свободный слот не простаивал
                // до среза WAIT_SLICE (150 мс).
                self.cv.notify_all();
            }
            // 3) Потолок ожидания.
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let depth = st.waiting_total() as u32;
                record_wait(&mut st, started.elapsed().as_millis() as u64);
                if let Some(seq) = enqueued.take() {
                    remove_waiter(&mut st, user, seq);
                }
                st.timeouts += 1;
                drop(st);
                return Err(QueueReject::Timeout {
                    waited_sec: started.elapsed().as_secs(),
                    depth,
                });
            }
            // 4) Ждём срез и (вне блокировки) проверяем живость клиента.
            let slice = remaining.min(WAIT_SLICE);
            let (guard, _) = self
                .cv
                .wait_timeout(st, slice)
                .unwrap_or_else(|e| e.into_inner());
            drop(guard);
            if !alive() {
                let mut st = self.lock();
                if let Some(seq) = enqueued.take() {
                    remove_waiter(&mut st, user, seq);
                }
                st.aborted += 1;
                drop(st);
                return Err(QueueReject::Aborted);
            }
            st = self.lock();
        }
    }

    /// Освободить слот (вызывает `QueueTicket::drop`) и разбудить ожидающих.
    fn release(&self, user: &str) {
        let mut st = self.lock();
        st.executing = st.executing.saturating_sub(1);
        if let Some(q) = st.users.get_mut(user) {
            q.executing = q.executing.saturating_sub(1);
        }
        if st
            .users
            .get(user)
            .is_some_and(|q| q.waiting.is_empty() && q.executing == 0)
        {
            st.users.remove(user);
        }
        self.cv.notify_all();
    }

    /// Снимок метрик (для диагностики/снапшота).
    pub fn stats(&self) -> QueueStats {
        let st = self.lock();
        let mut waits: Vec<u64> = st.waits_ms.iter().copied().collect();
        waits.sort_unstable();
        let mut per_user: Vec<UserStat> = st
            .users
            .iter()
            .map(|(user, q)| UserStat {
                user: user.clone(),
                waiting: q.waiting.len() as u32,
                executing: q.executing,
            })
            .collect();
        per_user.sort_by(|a, b| {
            b.waiting
                .cmp(&a.waiting)
                .then_with(|| b.executing.cmp(&a.executing))
                .then_with(|| a.user.cmp(&b.user))
        });
        QueueStats {
            enabled: self.cfg.enabled,
            policy: self.cfg.policy,
            max_inflight: self.cfg.max_inflight,
            max_inflight_per_user: self.cfg.max_inflight_per_user,
            max_depth: self.cfg.max_depth,
            wait_timeout_sec: self.cfg.wait_timeout_sec,
            depth: st.waiting_total() as u32,
            executing: st.executing,
            granted: st.granted,
            rejected_full: st.rejected_full,
            rejected_user: st.rejected_user,
            timeouts: st.timeouts,
            aborted: st.aborted,
            total_wait_ms: st.total_wait_ms,
            wait_p50_ms: percentile(&waits, 0.50),
            wait_p95_ms: percentile(&waits, 0.95),
            per_user,
        }
    }

    /// Снапшот в JSON (кросс-процессный статус — `data/mcp-queue.json`).
    pub fn snapshot_json(&self) -> Value {
        let s = self.stats();
        json!({
            "ts": epoch_now(),
            "pid": std::process::id(),
            "enabled": s.enabled,
            "policy": s.policy.as_str(),
            "max_inflight": s.max_inflight,
            "max_inflight_per_user": s.max_inflight_per_user,
            "max_depth": s.max_depth,
            "wait_timeout_sec": s.wait_timeout_sec,
            "depth": s.depth,
            "executing": s.executing,
            "granted": s.granted,
            "rejected_full": s.rejected_full,
            "rejected_user": s.rejected_user,
            "timeouts": s.timeouts,
            "aborted": s.aborted,
            "total_wait_ms": s.total_wait_ms,
            "wait_p50_ms": s.wait_p50_ms,
            "wait_p95_ms": s.wait_p95_ms,
            "per_user": s
                .per_user
                .iter()
                .map(|u| json!({ "user": u.user, "waiting": u.waiting, "executing": u.executing }))
                .collect::<Vec<_>>(),
        })
    }

    /// Атомарно записать снапшот в `path`.
    pub fn write_snapshot(&self, path: &Path) {
        write_snapshot(path, &self.snapshot_json());
    }
}

/// RAII-слот очереди: на `Drop` освобождает место исполнения.
#[derive(Debug)]
pub struct QueueTicket {
    queue: Arc<HeavyQueue>,
    user: String,
    waited: Duration,
}

impl QueueTicket {
    /// Сколько миллисекунд запрос прождал слот (0 — стартовал сразу).
    pub fn waited_ms(&self) -> u64 {
        self.waited.as_millis() as u64
    }

    /// Ключ fair-share, под которым запрос стоял в очереди.
    pub fn user(&self) -> &str {
        &self.user
    }
}

impl Drop for QueueTicket {
    fn drop(&mut self) {
        self.queue.release(&self.user);
    }
}

/// Метрики очереди (для `hds check`/`/api/diagnostics`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueStats {
    pub enabled: bool,
    pub policy: QueuePolicy,
    pub max_inflight: u32,
    /// Предел одновременных RAG на пользователя (M4 T4).
    pub max_inflight_per_user: u32,
    pub max_depth: u32,
    pub wait_timeout_sec: u64,
    /// Ждут сейчас.
    pub depth: u32,
    /// Исполняется сейчас.
    pub executing: u32,
    pub granted: u64,
    pub rejected_full: u64,
    /// Отказы по пер-пользовательскому пределу (M4 T4).
    pub rejected_user: u64,
    pub timeouts: u64,
    pub aborted: u64,
    pub total_wait_ms: u64,
    /// Медиана ожиданий, мс (включая ожидания, закончившиеся таймаутом).
    pub wait_p50_ms: u64,
    /// 95-й перцентиль ожиданий, мс (включая таймауты).
    pub wait_p95_ms: u64,
    /// Пер-пользовательская картина («кто кого ждёт»).
    pub per_user: Vec<UserStat>,
}

/// Ожидание/исполнение одного пользователя.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserStat {
    pub user: String,
    pub waiting: u32,
    pub executing: u32,
}

/// Учесть завершённое ожидание (мс) в окне p50/p95 и в сумме.
///
/// В окно попадают и ожидания, закончившиеся таймаутом: иначе система, где все
/// запросы не дожидаются слота, показывала бы «ожидание 0».
fn record_wait(st: &mut State, ms: u64) {
    if ms == 0 {
        return;
    }
    st.total_wait_ms += ms;
    if st.waits_ms.len() >= WAITS_WINDOW {
        st.waits_ms.pop_front();
    }
    st.waits_ms.push_back(ms);
}

/// Убрать заявку `seq` из очереди пользователя; пустую запись — удалить.
fn remove_waiter(st: &mut State, user: &str, seq: u64) {
    if let Some(q) = st.users.get_mut(user) {
        q.waiting.retain(|s| *s != seq);
    }
    if st
        .users
        .get(user)
        .is_some_and(|q| q.waiting.is_empty() && q.executing == 0)
    {
        st.users.remove(user);
    }
    let users = &st.users;
    st.order
        .retain(|u| users.get(u).is_some_and(|q| !q.waiting.is_empty()));
}

/// Round-robin: только что обслуженный пользователь уходит в конец курсора,
/// если у него ещё есть ожидающие (жадный не забирает подряд все слоты).
fn rotate_rr(st: &mut State, user: &str) {
    let Some(pos) = st.order.iter().position(|u| u == user) else {
        return;
    };
    if let Some(u) = st.order.remove(pos) {
        let more = st.users.get(&u).is_some_and(|q| !q.waiting.is_empty());
        if more {
            st.order.push_back(u);
        }
    }
}

/// p-перцентиль (0..1) по отсортированному срезу (`0`, если пусто).
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Путь снапшота очереди: `<state_dir>/data/mcp-queue.json`
/// (кросс-процессный статус для `hds check`/`/api/diagnostics`).
pub fn snapshot_path(cfg: &Config) -> PathBuf {
    crate::config::state_dir(cfg)
        .join("data")
        .join("mcp-queue.json")
}

/// Атомарная запись снапшота (временный файл + rename) — читатель не увидит
/// частичный JSON (как `index.heartbeat.json`).
pub fn write_snapshot(path: &Path, v: &Value) {
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, v.to_string().as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Разбор снапшота (`None` — нет файла/битый JSON).
pub fn read_snapshot(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Возраст снапшота по полю `ts`, секунды (`None` — нет файла/поля).
pub fn snapshot_age_secs(v: &Value) -> Option<f64> {
    v.get("ts")
        .and_then(|t| t.as_f64())
        .map(|ts| (epoch_now() - ts).max(0.0))
}

/// Фоновый писатель снапшота (поток-daemon; `stop` останавливает цикл).
///
/// Первый снапшот пишется сразу — `hds check` не ждёт интервала.
pub fn spawn_snapshot_writer(
    queue: Arc<HeavyQueue>,
    path: PathBuf,
    every: Duration,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        queue.write_snapshot(&path);
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(every);
            if stop.load(Ordering::Relaxed) {
                break;
            }
            queue.write_snapshot(&path);
        }
    })
}

fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cfg_of(text: &str) -> Config {
        serde_yaml::from_str(text).unwrap()
    }

    fn qc(policy: QueuePolicy, max_inflight: u32, max_depth: u32, wait_sec: u64) -> QueueConfig {
        QueueConfig {
            enabled: true,
            max_depth,
            wait_timeout_sec: wait_sec,
            policy,
            max_inflight,
            // Тесты RR/FIFO (до T4) шлют от одного пользователя несколько
            // запросов подряд — пер-пользовательский предел тут не участвует.
            max_inflight_per_user: u32::MAX,
        }
    }

    fn queue(
        policy: QueuePolicy,
        max_inflight: u32,
        max_depth: u32,
        wait_sec: u64,
    ) -> Arc<HeavyQueue> {
        Arc::new(HeavyQueue::new(qc(
            policy,
            max_inflight,
            max_depth,
            wait_sec,
        )))
    }

    /// Дефолты по профилю (desktop — выкл, server — вкл), явные ключи сильнее;
    /// `max_inflight` берётся из `rate_limit` (единый дефолт с лимитером M3).
    #[test]
    fn config_defaults_by_profile_and_overrides() {
        let desktop = QueueConfig::from_config(&cfg_of("profile: desktop"));
        assert!(
            !desktop.enabled,
            "desktop — очередь выключена (поведение M3)"
        );
        assert_eq!(desktop.max_depth, DEFAULT_MAX_DEPTH);
        assert_eq!(desktop.wait_timeout_sec, DEFAULT_WAIT_TIMEOUT_SEC);
        assert_eq!(desktop.policy, QueuePolicy::RoundRobin);
        assert_eq!(desktop.max_inflight, 4, "дефолт rate_limit.max_inflight");

        let server = QueueConfig::from_config(&cfg_of("profile: server"));
        assert!(server.enabled, "server — очередь включена");
        assert!(
            server.wait_timeout_sec < CLIENT_TIMEOUT_SEC,
            "ожидание короче клиентского таймаута"
        );
        assert_eq!(
            server.max_inflight_per_user, 2,
            "дефолт rate_limit.max_inflight_per_user (M4 T4)"
        );

        let custom = QueueConfig::from_config(&cfg_of(
            "profile: server\nqueue:\n  enabled: false\n  max_depth: 3\n  wait_timeout_sec: 30\n  policy: fifo\nrate_limit:\n  max_inflight: 2\n",
        ));
        assert!(!custom.enabled, "явное значение сильнее профиля");
        assert_eq!(custom.max_depth, 3);
        assert_eq!(custom.wait_timeout_sec, 30);
        assert_eq!(custom.policy, QueuePolicy::Fifo);
        assert_eq!(custom.max_inflight, 2);

        // Мусорные значения — дефолты, а не паника.
        let junk = QueueConfig::from_config(&cfg_of(
            "profile: server\nqueue:\n  enabled: да\n  max_depth: -1\n  policy: random\n",
        ));
        assert!(junk.enabled);
        assert_eq!(junk.max_depth, DEFAULT_MAX_DEPTH);
        assert_eq!(junk.policy, QueuePolicy::RoundRobin);
        assert_eq!(QueuePolicy::parse(" RR "), Some(QueuePolicy::RoundRobin));
        assert_eq!(QueuePolicy::parse("fifo"), Some(QueuePolicy::Fifo));
        assert_eq!(QueuePolicy::parse(""), None);
    }

    /// M4 T4 (§3.3): пер-пользовательский предел — 3-й одновременный RAG
    /// одного пользователя отвергается СРАЗУ (`PerUser`), хотя глобальный слот
    /// свободен; освобождение слота возвращает возможность стартовать.
    #[test]
    fn per_user_limit_rejects_immediately() {
        let cfg = QueueConfig {
            enabled: true,
            max_depth: 5,
            wait_timeout_sec: 2,
            policy: QueuePolicy::RoundRobin,
            max_inflight: 3,
            max_inflight_per_user: 1,
        };
        let q = Arc::new(HeavyQueue::new(cfg));
        let a = q.acquire("user:anna", || true).expect("слот свободен");
        // Второй запрос ЭТОГО пользователя — сразу отказ (ждать нечего).
        match q.acquire("user:anna", || true) {
            Err(QueueReject::PerUser { limit }) => assert_eq!(limit, 1),
            other => panic!("ожидался PerUser, получено {other:?}"),
        }
        // Другой пользователь — свой счётчик.
        let b = q.acquire("user:boris", || true).expect("свой слот");
        let st = q.stats();
        assert_eq!(st.executing, 2, "anna + boris");
        assert_eq!(st.depth, 0, "в очередь никто не встал");
        assert_eq!(st.rejected_user, 1);
        assert_eq!(st.max_inflight_per_user, 1);
        // Снапшот показывает лимит и счётчик отказов.
        let v = q.snapshot_json();
        assert_eq!(v["max_inflight_per_user"], 1);
        assert_eq!(v["rejected_user"], 1);
        // Освобождение слота — пользователь снова может стартовать.
        drop(a);
        let a2 = q.acquire("user:anna", || true).expect("слот вернулся");
        drop(a2);
        drop(b);
        assert_eq!(q.stats().executing, 0);
    }

    /// Свободные слоты — быстрый путь: без ожидания, `waited_ms == 0`.
    #[test]
    fn admits_immediately_when_free() {
        let q = queue(QueuePolicy::RoundRobin, 2, 4, 5);
        let a = q.acquire("user:a", || true).expect("слот свободен");
        let b = q.acquire("user:b", || true).expect("второй слот свободен");
        assert_eq!(a.waited_ms(), 0);
        assert_eq!(b.waited_ms(), 0);
        let st = q.stats();
        assert_eq!(st.executing, 2);
        assert_eq!(st.depth, 0);
        assert_eq!(st.granted, 2);
        assert_eq!(st.max_inflight, 2);
        drop(a);
        let c = q.acquire("user:c", || true).expect("слот освобождён");
        assert_eq!(q.stats().executing, 2);
        drop(b);
        drop(c);
        assert_eq!(q.stats().executing, 0, "все слоты вернулись");
    }

    /// Очередь полна (`max_depth: 0` — ждать нельзя) → `QueueReject::Full` (429).
    #[test]
    fn full_queue_rejects() {
        let q = queue(QueuePolicy::RoundRobin, 1, 0, 5);
        let _held = q.acquire("user:a", || true).unwrap();
        let err = q.acquire("user:b", || true).expect_err("ждать негде");
        assert_eq!(
            err,
            QueueReject::Full {
                max_depth: 0,
                depth: 0
            }
        );
        let st = q.stats();
        assert_eq!(st.rejected_full, 1);
        assert_eq!(st.depth, 0);
        assert_eq!(st.granted, 1, "отказ не занимает слот");
    }

    /// Потолок ожидания (`wait_timeout_sec: 0`) → `QueueReject::Timeout` (503).
    #[test]
    fn wait_timeout_rejects() {
        let q = queue(QueuePolicy::RoundRobin, 1, 5, 0);
        let _held = q.acquire("user:a", || true).unwrap();
        let err = q
            .acquire("user:b", || true)
            .expect_err("слота нет и ждать нельзя");
        assert_eq!(
            err,
            QueueReject::Timeout {
                waited_sec: 0,
                depth: 1
            }
        );
        let st = q.stats();
        assert_eq!(st.timeouts, 1);
        assert_eq!(st.depth, 0, "таймаут снимает заявку");
    }

    /// Обрыв клиента, пока запрос ждёт, освобождает место (место не «течёт»).
    #[test]
    fn client_abort_frees_place() {
        let q = queue(QueuePolicy::RoundRobin, 1, 5, 5);
        let held = q.acquire("user:a", || true).unwrap();
        let err = q.acquire("user:b", || false).expect_err("клиент ушёл");
        assert_eq!(err, QueueReject::Aborted);
        let st = q.stats();
        assert_eq!(st.aborted, 1);
        assert_eq!(st.depth, 0);
        assert_eq!(st.granted, 1);
        drop(held);
        assert!(q.acquire("user:b", || true).is_ok(), "место свободно");
    }

    /// Порядок выдачи слотов при `max_inflight: 1`: «жадный» A шлёт 3 запроса,
    /// B — один; приходят в порядке A, A, A, B (каждый держит слот 120 мс).
    fn grant_order(policy: QueuePolicy) -> Vec<String> {
        let q = queue(policy, 1, 20, 10);
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for (i, user) in ["user:a", "user:a", "user:a", "user:b"].iter().enumerate() {
            let q = Arc::clone(&q);
            let log = Arc::clone(&log);
            let user = user.to_string();
            handles.push(std::thread::spawn(move || {
                if i > 0 {
                    std::thread::sleep(Duration::from_millis(25 * i as u64));
                }
                let t = q.acquire(&user, || true).expect("слот достанется");
                log.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(user.clone());
                std::thread::sleep(Duration::from_millis(120));
                drop(t);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let out = log.lock().unwrap_or_else(|e| e.into_inner()).clone();
        out
    }

    /// Fair-share: жадный пользователь не занимает слоты подряд, пока ждёт другой.
    #[test]
    fn rr_fair_share_does_not_starve_others() {
        let order = grant_order(QueuePolicy::RoundRobin);
        assert_eq!(
            order,
            vec!["user:a", "user:a", "user:b", "user:a"],
            "round-robin: после второго слота A поехал B, хотя A пришёл раньше"
        );
    }

    /// `policy: fifo` — строго по времени прихода (без fair-share).
    #[test]
    fn fifo_policy_is_strict_order() {
        let order = grant_order(QueuePolicy::Fifo);
        assert_eq!(
            order,
            vec!["user:a", "user:a", "user:a", "user:b"],
            "fifo: оба A раньше B"
        );
    }

    /// Снапшот: поля диагностики, пер-пользовательская картина, p50/p95.
    #[test]
    fn snapshot_fields_and_first_person_view() {
        let q = queue(QueuePolicy::RoundRobin, 1, 5, 5);
        let held = q.acquire("user:anna", || true).unwrap();
        let waiter = {
            let q = Arc::clone(&q);
            std::thread::spawn(move || {
                let t = q.acquire("user:boris", || true).expect("дождётся слота");
                std::thread::sleep(Duration::from_millis(60));
                drop(t);
            })
        };
        std::thread::sleep(Duration::from_millis(60));
        let v = q.snapshot_json();
        assert_eq!(v["depth"], 1, "{v}");
        assert_eq!(v["executing"], 1, "{v}");
        assert_eq!(v["max_inflight"], 1);
        assert_eq!(v["policy"], "rr");
        assert!(v["pid"].as_u64().unwrap_or(0) > 0);
        assert!(v["ts"].as_f64().unwrap_or(0.0) > 0.0);
        let per = v["per_user"].as_array().unwrap();
        assert_eq!(per.len(), 2, "кто исполняет и кто ждёт: {v}");
        assert_eq!(
            per[0]["user"], "user:boris",
            "ждущие — раньше в списке: {v}"
        );
        assert_eq!(per[1]["user"], "user:anna");
        drop(held);
        waiter.join().unwrap();
        let st = q.stats();
        assert_eq!(st.executing, 0);
        assert!(st.wait_p50_ms >= 40, "ожидание записано: {st:?}");
        assert!(st.wait_p95_ms >= st.wait_p50_ms, "{st:?}");
        assert!(st.total_wait_ms > 0, "{st:?}");
    }

    /// Файл-снапшот: атомарная запись/чтение (кросс-процессная диагностика).
    #[test]
    fn snapshot_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("hds-queue-snap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("data").join("mcp-queue.json");
        let q = queue(QueuePolicy::Fifo, 2, 3, 7);
        q.write_snapshot(&path);
        let v = read_snapshot(&path).expect("снапшот записан");
        assert_eq!(v["max_depth"], 3);
        assert_eq!(v["wait_timeout_sec"], 7);
        assert_eq!(v["policy"], "fifo");
        let age = snapshot_age_secs(&v).expect("поле ts");
        assert!(age < 5.0, "снапшот свежий: {age}");
        assert!(!path.with_extension("json.tmp").exists(), "tmp-файл убран");
        assert!(read_snapshot(&dir.join("nope.json")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F4 (M5 T0.4): строгий FIFO внутри пользователя — после освобождения
    /// слота его получает заявка с МЕНЬШИМ `seq` (вставшая раньше), а не
    /// «какая нить проснулась первой»; порядок выдачи слотов = порядок
    /// постановки в очередь.
    #[test]
    fn fifo_within_user_by_seq() {
        let q = queue(QueuePolicy::RoundRobin, 1, 5, 30);
        let holder = q.acquire("user:u", || true).expect("слот свободен");

        // Две заявки того же пользователя ждут (глубина 2)...
        let order = Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));
        let mut joins = Vec::new();
        for i in 0..2 {
            let q = Arc::clone(&q);
            let order = Arc::clone(&order);
            joins.push(std::thread::spawn(move || {
                let t = q.acquire("user:u", || true).expect("дождались слота");
                order.lock().unwrap().push(i);
                drop(t); // слот уходит следующей заявке
            }));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while q.stats().depth < 2 {
            assert!(Instant::now() < deadline, "заявки не встали в очередь");
            std::thread::sleep(Duration::from_millis(10));
        }

        // ...тогда освобождаем слот: обе разбужены, но сначала — заявка с
        // меньшим seq (i=0), вторая дожидается своего хода.
        drop(holder);
        for j in joins {
            j.join().unwrap();
        }
        let order = order.lock().unwrap().clone();
        assert_eq!(order, vec![0, 1], "FIFO внутри пользователя: {order:?}");
    }
}
