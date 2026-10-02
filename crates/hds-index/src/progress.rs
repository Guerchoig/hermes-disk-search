//! Живой прогресс индексации — порт `hds/progress.py`.
//!
//! Что сохранено 1:1:
//! * те же счётчики и метод [`ProgressReporter::heartbeat_data`] (поля
//!   `seen/processed/errors/chunks/paused/total/elapsed/rate_min/rate_window/
//!   eta_sec/events/path/phase/progress/remaining`) — их читают UI и MCP;
//! * `rate_window(300)` и `eta_sec()` с теми же правилами (пустое окно ⇒
//!   средняя скорость за всё время);
//! * строка статуса того же вида, `\r`-перерисовка только в tty, в не-tty —
//!   полные строки.
//!
//! Важное отличие от Python (по грабле W0, `PLAN_W2_LLM_HOST.md` §5/B4):
//! **вывод не должен зависеть от читателя stdout** — рендер живёт в отдельном
//! потоке, поэтому переполненный pipe (харнесс W0 «встал») блокирует только
//! рендер-поток, а не конвейер индексации. Главный кросс-процессный канал —
//! `index.heartbeat.json` ([`crate::heartbeat`]).

use std::collections::{BTreeMap, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Формат `HH:MM:SS` (порт `_fmt_dur`).
pub fn fmt_dur(sec: f64) -> String {
    let s = sec as i64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// `%d %03d` для тысяч (порт `_human`).
pub fn human(n: u64) -> String {
    if n >= 1000 {
        format!("{} {:03}", n / 1000, n % 1000)
    } else {
        n.to_string()
    }
}

/// «Nд Nч Nм» / «Nч Nм» / «Nм» (порт `_fmt_eta`).
pub fn fmt_eta(sec: f64) -> String {
    let s = sec.max(0.0) as i64;
    let (d, h, m) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{}д {}ч {}м", d, h, m)
    } else if h > 0 {
        format!("{}ч {}м", h, m)
    } else {
        format!("{}м", m.max(1))
    }
}

fn epoch_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Одно событие «последние обработанные» (для UI/MCP), как `events` в Python.
#[derive(Debug, Clone)]
pub struct Event {
    pub path: Option<String>,
    pub status: String,
    pub kind: Option<String>,
    pub dur: f64,
    pub chunks: u64,
    pub ts: f64,
}

impl Event {
    fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "status": self.status,
            "kind": self.kind,
            "dur": (self.dur * 100.0).round() / 100.0,
            "chunks": self.chunks,
            "ts": self.ts,
        })
    }
}

#[derive(Debug, Default)]
pub struct Inner {
    seen_count: u64,
    processed_count: u64,
    errors: u64,
    chunks: u64,
    by_kind: BTreeMap<String, u64>,
    total: u64,
    seen_ts: VecDeque<Instant>,
    current: Option<(String, String)>,
    current_since: Option<Instant>,
    progress: Option<f64>,
    paused: bool,
    last_done: Option<(String, String, f64)>,
    last_path: Option<String>,
    events: VecDeque<Event>,
}

/// Порт `progress.ProgressReporter` (см. модуль).
///
/// `Clone` — намеренно: фоновому потоку heartbeat нужна своя копия того же
/// состояния (все разделяемые поля — `Arc`, счётчики общие).
#[derive(Clone)]
pub struct ProgressReporter {
    sec: u64,
    inner: Arc<Mutex<Inner>>,
    stop: Arc<AtomicBool>,
    handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    t0: Instant,
    is_tty: bool,
    final_elapsed: Arc<Mutex<f64>>,
}

impl ProgressReporter {
    /// `ProgressReporter(sec)`; `sec = 0` отключает поток-рендер (счётчики живут).
    pub fn new(sec: u64) -> Self {
        let is_tty = {
            use std::io::IsTerminal;
            std::io::stdout().is_terminal()
        };
        ProgressReporter {
            sec,
            inner: Arc::new(Mutex::new(Inner::default())),
            stop: Arc::new(AtomicBool::new(false)),
            handle: Arc::new(Mutex::new(None)),
            t0: Instant::now(),
            is_tty,
            final_elapsed: Arc::new(Mutex::new(0.0)),
        }
    }

    /// `final_elapsed` из Python (секунды прогона).
    pub fn final_elapsed(&self) -> f64 {
        *self.final_elapsed.lock().unwrap()
    }

    /// True, если stdout — терминал (только там рисуется «живая строка»).
    pub fn is_tty(&self) -> bool {
        self.is_tty
    }

    /// Запуск потока-рендера (при `sec > 0`).
    pub fn start(&self) {
        if self.sec == 0 {
            return;
        }
        {
            let g = self.handle.lock().unwrap();
            if g.is_some() {
                return;
            }
        }
        let inner = Arc::clone(&self.inner);
        let stop = Arc::clone(&self.stop);
        let t0 = self.t0;
        let sec = self.sec;
        let is_tty = self.is_tty;
        let handle = std::thread::spawn(move || {
            let mut last_len = 0usize;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(sec));
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let line = status_line(&inner, t0, false);
                render(&line, is_tty, false, &mut last_len);
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
    }

    /// Остановка потока-рендера + финальная перерисовка (`note()`).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let handle = self.handle.lock().unwrap().take();
        if let Some(h) = handle {
            let _ = h.join();
        }
        self.note();
    }

    /// `rep.seen()` — файл попал в обработку.
    pub fn seen(&self) {
        let mut g = self.inner.lock().unwrap();
        g.seen_count += 1;
        g.seen_ts.push_back(Instant::now());
    }

    /// `rep.set_total(total)` — пре-подсчёт для ETA.
    pub fn set_total(&self, total: u64) {
        self.inner.lock().unwrap().total = total;
    }

    /// Скользящая скорость (файлов/мин за `window` секунд; пусто ⇒ средняя за всё время).
    pub fn rate_window(&self, window: u64) -> u64 {
        let mut g = self.inner.lock().unwrap();
        let now = Instant::now();
        while let Some(front) = g.seen_ts.front() {
            if front.elapsed() > Duration::from_secs(window) {
                g.seen_ts.pop_front();
            } else {
                break;
            }
        }
        let elapsed = now.duration_since(self.t0).as_secs_f64().max(0.001);
        if g.seen_ts.len() >= 2 {
            let span = (window as f64).min(elapsed.max(0.001));
            (g.seen_ts.len() as f64 / span * 60.0).round() as u64
        } else {
            (g.seen_count as f64 / elapsed * 60.0).round() as u64
        }
    }

    /// Оценка остатка в секундах (`None` — total неизвестен / всё пройдено).
    pub fn eta_sec(&self) -> Option<i64> {
        let g = self.inner.lock().unwrap();
        if g.total == 0 || g.total <= g.seen_count {
            return None;
        }
        let now = Instant::now();
        let recent = g
            .seen_ts
            .iter()
            .filter(|t| t.elapsed() <= Duration::from_secs(300))
            .count();
        let elapsed = now.duration_since(self.t0).as_secs_f64().max(0.001);
        let rate = if recent >= 2 {
            recent as f64 / (300.0f64).min(elapsed.max(0.001))
        } else {
            g.seen_count as f64 / elapsed
        };
        if rate <= 0.0 {
            return None;
        }
        Some(((g.total - g.seen_count) as f64 / rate) as i64)
    }

    /// `rep.processed(status, kind, dur, chunks)` — файл завершён.
    pub fn processed(&self, status: &str, kind: Option<&str>, dur: f64, chunks: u64) {
        let mut g = self.inner.lock().unwrap();
        let key = status.split('(').next().unwrap_or(status).to_string();
        if key.starts_with("error") {
            g.errors += 1;
        }
        if [
            "indexed",
            "moved",
            "unchanged",
            "skipped_big",
            "skipped_type",
        ]
        .contains(&key.as_str())
        {
            g.processed_count += 1;
        }
        g.chunks += chunks;
        if let Some(k) = kind {
            *g.by_kind.entry(k.to_string()).or_insert(0) += 1;
        }
        g.current = None;
        g.current_since = None;
        g.progress = None;
        let ev = Event {
            path: g.last_path.clone(),
            status: key,
            kind: kind.map(|k| k.to_string()),
            dur,
            chunks,
            ts: epoch_now(),
        };
        if g.events.len() >= 50 {
            g.events.pop_back();
        }
        g.events.push_front(ev);
    }

    /// `rep.set_last_path(path)` — путь, который попадёт в `events`.
    pub fn set_last_path(&self, path: &str) {
        self.inner.lock().unwrap().last_path = Some(path.to_string());
    }

    /// `rep.set_paused(bool)` — пауза индексации (`index.pause`).
    pub fn set_paused(&self, paused: bool) {
        self.inner.lock().unwrap().paused = paused;
    }

    /// `rep.set_current(path, phase)` — что обрабатывается сейчас.
    pub fn set_current(&self, path: &str, phase: &str) {
        let mut g = self.inner.lock().unwrap();
        g.current = Some((path.to_string(), phase.to_string()));
        g.current_since = Some(Instant::now());
        g.progress = None;
    }

    /// `rep.set_progress(pct)` — прогресс внутри файла (0–100, для медиа).
    pub fn set_progress(&self, pct: f64) {
        let mut g = self.inner.lock().unwrap();
        if g.current.is_some() {
            g.progress = Some(pct.clamp(0.0, 100.0));
        }
    }

    /// `rep.last_done = (path, status, dur)` — последняя завершённая строка статуса.
    pub fn set_last_done(&self, path: &str, status: &str, dur: f64) {
        self.inner.lock().unwrap().last_done = Some((path.to_string(), status.to_string(), dur));
    }

    /// Порт `heartbeat_data()`: снимок счётчиков для `index.heartbeat.json`.
    pub fn heartbeat_data(&self) -> Value {
        let g = self.inner.lock().unwrap();
        let elapsed = self.t0.elapsed().as_secs_f64().max(0.001);
        let rate_min = (g.seen_count as f64 / elapsed * 60.0).round() as i64;
        let recent = g
            .seen_ts
            .iter()
            .filter(|t| t.elapsed() <= Duration::from_secs(300))
            .count();
        let rate = if recent >= 2 {
            recent as f64 / (300.0f64).min(elapsed)
        } else {
            g.seen_count as f64 / elapsed
        };
        let eta_opt = if g.total == 0 || g.total <= g.seen_count || rate <= 0.0 {
            None
        } else {
            Some(((g.total - g.seen_count) as f64 / rate) as i64)
        };
        let rate_window = if g.seen_ts.len() >= 2 {
            let window = 300.0f64.min(elapsed);
            (g.seen_ts.len() as f64 / window * 60.0).round() as i64
        } else {
            rate_min
        };
        let events: Vec<Value> = g.events.iter().map(|e| e.to_json()).collect();
        let mut d = json!({
            "seen": g.seen_count,
            "processed": g.processed_count,
            "errors": g.errors,
            "chunks": g.chunks,
            "paused": g.paused,
            "total": g.total,
            "elapsed": (elapsed * 10.0).round() / 10.0,
            "rate_min": rate_min,
            "rate_window": rate_window,
            "eta_sec": eta_opt,
            "events": events,
        });
        if let Some((path, phase)) = &g.current {
            d["path"] = json!(path);
            d["phase"] = json!(phase);
            d["progress"] = json!(g.progress);
        }
        if eta_opt.is_none() {
            if let Some(obj) = d.as_object_mut() {
                obj.remove("eta_sec");
            }
        } else {
            d["remaining"] = json!(g.total - g.seen_count);
        }
        d
    }

    /// `rep.note()` — очистить «живую строку» перед обычной строкой лога.
    pub fn note(&self) {
        if self.is_tty {
            write_out("\r");
        }
    }

    /// Порт `finish(counters)`: финальная строка + `[done] {counters}`.
    pub fn finish(&self, counters: Option<&Value>) {
        *self.final_elapsed.lock().unwrap() = self.t0.elapsed().as_secs_f64();
        self.stop();
        let line = status_line(&self.inner, self.t0, true);
        let mut last_len = 0usize;
        render(&line, self.is_tty, true, &mut last_len);
        if let Some(c) = counters {
            write_out(&format!("[done] {}\n", py_dict(c)));
        }
    }
}

/// Порт `_status_line`: «⏱ … просмотрено … обработано … ошибок …».
fn status_line(inner: &Arc<Mutex<Inner>>, t0: Instant, final_: bool) -> String {
    let g = inner.lock().unwrap();
    let elapsed = t0.elapsed().as_secs_f64().max(0.001);
    let rate = g.seen_count as f64 / elapsed * 60.0;
    let mut line = format!(
        "⏱ {} | просмотрено {} | обработано {} | ошибок {} | {:.0} файлов/мин",
        fmt_dur(elapsed),
        human(g.seen_count),
        g.processed_count,
        g.errors,
        rate
    );
    let eta = if g.total != 0 && g.total > g.seen_count {
        let recent = g
            .seen_ts
            .iter()
            .filter(|t| t.elapsed() <= Duration::from_secs(300))
            .count();
        let r = if recent >= 2 {
            recent as f64 / (300.0f64).min(elapsed)
        } else {
            g.seen_count as f64 / elapsed
        };
        if r > 0.0 {
            Some(((g.total - g.seen_count) as f64 / r) as i64)
        } else {
            None
        }
    } else {
        None
    };
    if let Some(e) = eta {
        line += &format!(
            " | осталось ≈ {} | ETA ≈ {}",
            human(g.total.saturating_sub(g.seen_count)),
            fmt_eta(e as f64)
        );
    }
    if g.paused {
        line = format!("⏸ ПАУЗА | {line}");
    }
    let tail = if let Some((path, phase)) = &g.current {
        let age = fmt_dur(
            g.current_since
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(0.0),
        );
        let name = basename(path);
        match g.progress {
            Some(p) => format!(" | ▶ {} [{}, {:.0}%, идёт {}]", name, phase, p, age),
            None => format!(" | ▶ {} [{}, идёт {}]", name, phase, age),
        }
    } else if let Some((path, status, dur)) = &g.last_done {
        if final_ {
            String::new()
        } else {
            format!(" | ✓ {} ({}, {:.1} с)", basename(path), status, dur)
        }
    } else {
        String::new()
    };
    line + &tail
}

/// `os.path.basename` для строки пути (разделители `/` и `\`).
fn basename(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// `_render`: `\r`-перерисовка в tty, иначе полная строка.
fn render(line: &str, is_tty: bool, final_: bool, last_len: &mut usize) {
    if is_tty && !final_ {
        let width = 120;
        let cut: String = line.chars().take(width).collect();
        let pad = (*last_len).max(cut.chars().count());
        let out = format!("\r{:<width$}", cut, width = pad);
        *last_len = pad;
        write_out(&out);
    } else {
        write_out(&format!("{line}\n"));
    }
}

/// Запись в stdout без паники на ошибках (запись — «best effort», грабля W0).
fn write_out(s: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(s.as_bytes());
    let _ = out.flush();
}

/// Python-подобное представление словаря (`{'key': value, …}`) для `[done]`.
pub fn py_dict(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let items: Vec<String> = m
                .iter()
                .map(|(k, val)| format!("'{}': {}", k.replace('\'', "\\'"), py_value(val)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
        other => py_value(other),
    }
}

fn py_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("'{}'", s.replace('\'', "\\'")),
        Value::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// Фазы по видам файла — порт `progress.PHASES` (что показывает «▶ … [фаза]»).
pub fn phase_for(kind: Option<&str>) -> &'static str {
    match kind {
        Some("media") => "видео/аудио → ffmpeg + Whisper",
        Some("pdf") => "PDF → текст/OCR",
        Some("image") => "картинка → OCR",
        Some("mpp") => "MS Project → разбор задач",
        Some("docx") => "Word-документ",
        Some("xlsx") => "Excel-таблица",
        Some("pptx") => "PowerPoint-презентация",
        Some("text") => "текст/код",
        _ => "обработка",
    }
}
