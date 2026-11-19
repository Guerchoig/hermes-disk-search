//! Процесс Node-воркера `sidecar/hds_agent/dist/worker.mjs` и двусторонний
//! NDJSON-транспорт §4.3.
//!
//! Отличие от `hds_extract::Worker`: тот умеет только «запрос → ответ», а
//! агенту нужен встречный поток — `host_call` от воркера к Rust (`egress_check`,
//! `office_*`, `ask_user`). Поэтому здесь свой цикл чтения:
//!
//! * поток-читатель разбирает stdout построчно и раскладывает сообщения:
//!   `event` — в канал событий, `ready`/`done` — ожидающему вызову,
//!   `host_call` — обработчику [`HostHandler`] (ответ пишется в stdin);
//! * `call` регистрирует ожидание, пишет команду и ждёт ответ с таймаутом;
//! * `next_event` отдаёт события вызывающей стороне (UI/CLI).
//!
//! Окружение процесса готовит [`crate::env`] (TR-3.2): профиль пользователя
//! перенаправлен, ключи провайдеров не наследуются. Async нет — только потоки
//! и блокирующий I/O, как во всём ядре.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

use crate::protocol::{encode_down, parse_line, Down, Up};

/// Параметры запуска воркера.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    /// Интерпретатор Node (`agent.node.path`, портативный или системный).
    pub program: PathBuf,
    /// Собранный воркер (`dist/worker.mjs`).
    pub script: PathBuf,
    /// Рабочий каталог — `<P>\workspace` (П-3: относительные пути от cwd).
    pub cwd: PathBuf,
    /// Очищенное окружение из [`crate::env::build`].
    pub env: Vec<(String, String)>,
    /// Куда писать stderr воркера (по умолчанию — в stderr процесса).
    pub stderr_log: Option<PathBuf>,
    /// Таймаут одной команды (`agent.request_timeout_secs`).
    pub timeout: Duration,
}

impl WorkerSpec {
    /// Таймаут по умолчанию: 120 с (как `agent.request_timeout_secs`).
    pub fn default_timeout() -> Duration {
        Duration::from_secs(120)
    }
}

/// Событие, которое воркер отдаёт вызывающей стороне (алиас `protocol::Up`).
pub use crate::protocol::Up as WorkerEvent;

/// Обработчик встречных запросов воркера. Вызывается в потоке-читателе, поэтому
/// реализация должна быть потокобезопасной и не блокировать надолго.
pub trait HostHandler: Send + Sync {
    /// Обработать `host_call`. `Err` уходит воркеру как `host_result.ok = false`.
    fn handle(&self, call: &str, payload: &Value) -> Result<Value, String>;
}

/// Заглушка: любой вызов — ошибка (для тестов транспорта).
pub struct NoHost;

impl HostHandler for NoHost {
    fn handle(&self, call: &str, _payload: &Value) -> Result<Value, String> {
        Err(format!("host_call {call} не поддерживается"))
    }
}

/// Причина отказа транспорта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    /// Не удалось запустить процесс.
    Spawn(String),
    /// Процесс завершился или канал закрыт.
    Closed,
    /// Команда не ответила за отведённое время.
    Timeout(u64),
    /// Ответ пришёл с `ok = false`.
    Failed(String),
    /// Ввод-вывод сломался.
    Io(String),
}

impl WorkerError {
    /// Текст для пользователя и журнала.
    pub fn reason(&self) -> String {
        match self {
            WorkerError::Spawn(e) => format!("воркер не запущен: {e}"),
            WorkerError::Closed => "воркер закрыл канал".to_string(),
            WorkerError::Timeout(secs) => format!("воркер не ответил за {secs} с"),
            WorkerError::Failed(e) => e.clone(),
            WorkerError::Io(e) => format!("сбой канала воркера: {e}"),
        }
    }
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

impl std::error::Error for WorkerError {}

/// Запущенный воркер.
pub struct Worker {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    events: Receiver<Up>,
    pending: Arc<Mutex<HashMap<String, Sender<Up>>>>,
    next_id: AtomicU64,
    timeout: Duration,
    reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    closed: Arc<Mutex<bool>>,
}

impl Worker {
    /// Запустить воркер и поднять потоки чтения.
    pub fn spawn(spec: &WorkerSpec, handler: Arc<dyn HostHandler>) -> Result<Worker, WorkerError> {
        let mut command = Command::new(&spec.program);
        command
            .arg(&spec.script)
            .current_dir(&spec.cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        let mut child = command
            .spawn()
            .map_err(|e| WorkerError::Spawn(format!("{}: {e}", spec.program.display())))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| WorkerError::Spawn("нет stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| WorkerError::Spawn("нет stdout".to_string()))?;
        let stderr = child.stderr.take();

        let stdin = Arc::new(Mutex::new(stdin));
        let pending: Arc<Mutex<HashMap<String, Sender<Up>>>> = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(Mutex::new(false));
        let (events_tx, events_rx) = mpsc::channel::<Up>();

        let reader = {
            let stdin = Arc::clone(&stdin);
            let pending = Arc::clone(&pending);
            let closed = Arc::clone(&closed);
            thread::Builder::new()
                .name("hds-agent-reader".to_string())
                .spawn(move || {
                    read_loop(stdout, stdin, pending, events_tx, closed, handler);
                })
                .map_err(|e| WorkerError::Spawn(e.to_string()))?
        };

        let stderr_reader = stderr.and_then(|stderr| {
            let log = spec.stderr_log.clone();
            thread::Builder::new()
                .name("hds-agent-stderr".to_string())
                .spawn(move || drain_stderr(stderr, log))
                .ok()
        });

        Ok(Worker {
            child,
            stdin,
            events: events_rx,
            pending,
            next_id: AtomicU64::new(1),
            timeout: spec.timeout,
            reader: Some(reader),
            stderr_reader,
            closed,
        })
    }

    /// PID процесса (для диагностики).
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Таймаут команд.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Признак закрытого канала.
    pub fn is_closed(&self) -> bool {
        self.closed.lock().map(|f| *f).unwrap_or(true)
    }

    /// Выполнить команду и дождаться `ready`/`done` с тем же id.
    pub fn call<F>(&self, make: F) -> Result<Up, WorkerError>
    where
        F: FnOnce(&str) -> Down,
    {
        let id = format!("{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| WorkerError::Closed)?;
            pending.insert(id.clone(), tx);
        }
        let down = make(&id);
        if let Err(e) = self.write(&down) {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
            return Err(e);
        }
        match rx.recv_timeout(self.timeout) {
            Ok(Up::Done {
                ok: false, error, ..
            }) => {
                Err(WorkerError::Failed(error.unwrap_or_else(|| {
                    "команда завершилась ошибкой".to_string()
                })))
            }
            Ok(other) => Ok(other),
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&id);
                }
                Err(WorkerError::Timeout(self.timeout.as_secs()))
            }
            Err(RecvTimeoutError::Disconnected) => Err(WorkerError::Closed),
        }
    }

    /// Дождаться следующего события (включая ошибки воркера).
    pub fn next_event(&self, timeout: Duration) -> Result<Up, WorkerError> {
        match self.events.recv_timeout(timeout) {
            Ok(up) => Ok(up),
            Err(RecvTimeoutError::Timeout) => Err(WorkerError::Timeout(timeout.as_secs())),
            Err(RecvTimeoutError::Disconnected) => Err(WorkerError::Closed),
        }
    }

    /// Записать сообщение в stdin воркера.
    pub fn write(&self, down: &Down) -> Result<(), WorkerError> {
        write_locked(&self.stdin, down).map_err(|e| WorkerError::Io(e.to_string()))
    }

    /// Корректно завершить процесс. При отказе — убить.
    pub fn shutdown(mut self) {
        let _ = self.write(&Down::Shutdown {
            id: "shutdown".to_string(),
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_reader.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_loop(
    stdout: ChildStdout,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<String, Sender<Up>>>>,
    events: Sender<Up>,
    closed: Arc<Mutex<bool>>,
    handler: Arc<dyn HostHandler>,
) {
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let Some(up) = parse_line(&line) else {
            continue;
        };
        match up {
            Up::HostCall { id, call, payload } => {
                let reply = match handler.handle(&call, &payload) {
                    Ok(value) => Down::HostResult {
                        id,
                        ok: true,
                        result: value,
                        error: None,
                    },
                    Err(error) => Down::HostResult {
                        id,
                        ok: false,
                        result: Value::Null,
                        error: Some(error),
                    },
                };
                let _ = write_locked(&stdin, &reply);
            }
            Up::Ready { ref id, .. } | Up::Done { ref id, .. } => {
                let id = id.clone();
                let sender = pending.lock().ok().and_then(|mut map| map.remove(&id));
                match sender {
                    Some(tx) => {
                        let _ = tx.send(up);
                    }
                    None => {
                        let _ = events.send(up);
                    }
                }
            }
            other => {
                let _ = events.send(other);
            }
        }
    }
    if let Ok(mut flag) = closed.lock() {
        *flag = true;
    }
    if let Ok(mut map) = pending.lock() {
        map.clear();
    }
}

fn write_locked(stdin: &Arc<Mutex<ChildStdin>>, down: &Down) -> std::io::Result<()> {
    let line = encode_down(down);
    let mut guard = stdin
        .lock()
        .map_err(|_| std::io::Error::other("stdin заблокирован"))?;
    guard.write_all(line.as_bytes())?;
    guard.write_all(b"\n")?;
    guard.flush()
}

fn drain_stderr(stderr: std::process::ChildStderr, log: Option<PathBuf>) {
    use std::io::Read;
    let mut reader = BufReader::new(stderr);
    match log {
        Some(path) => {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let mut buffer = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    let _ = file.write_all(&buffer[..n]);
                }
            }
        }
        None => {
            let mut sink = std::io::sink();
            let _ = std::io::copy(&mut reader, &mut sink);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Мини-воркер на Node для проверки транспорта (реальный протокол §4.3).
    const ECHO_WORKER: &str = r#"
import { createInterface } from 'node:readline';
const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
const out = (value) => process.stdout.write(JSON.stringify(value) + '\n');
rl.on('line', (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.type === 'hello') {
    out({ type: 'ready', id: msg.id, protocol: 1, node: process.version, sdk: 'test', tools: [] });
  } else if (msg.type === 'send') {
    out({ type: 'event', id: msg.id, event: { kind: 'assistant', text: 'echo: ' + msg.text } });
    out({ type: 'done', id: msg.id, ok: true, result: { text: 'echo: ' + msg.text } });
  } else if (msg.type === 'start') {
    out({ type: 'event', id: msg.id, event: { kind: 'started', project: msg.project } });
    out({ type: 'done', id: msg.id, ok: true, result: { session: 's1' } });
  } else if (msg.type === 'call_host') {
    out({ type: 'host_call', id: 'h1', call: 'egress_check', payload: { kind: 'model', body: { text: msg.text } } });
  } else if (msg.type === 'host_result') {
    out({ type: 'done', id: msg.id, ok: true, result: msg });
  } else if (msg.type === 'shutdown') {
    out({ type: 'done', id: msg.id, ok: true, result: {} });
    process.exit(0);
  }
});
"#;

    struct EchoHost;

    impl HostHandler for EchoHost {
        fn handle(&self, call: &str, payload: &Value) -> Result<Value, String> {
            if call == "egress_check" {
                Ok(json!({ "ok": true, "call": call, "body": payload }))
            } else {
                Err(format!("нет обработчика {call}"))
            }
        }
    }

    fn node_available() -> bool {
        Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Минимум окружения, без которого Node не стартует: без `SystemRoot`
    /// падает инициализация CSPRNG (`ncrypto::CSPRNG`). Продакшн-окружение
    /// собирает [`crate::env::build`] (TR-3.2); здесь — тот же минимум.
    fn node_env(dir: &std::path::Path) -> Vec<(String, String)> {
        let mut vars: Vec<(String, String)> = Vec::new();
        for name in ["SystemRoot", "PATH", "ComSpec", "PATHEXT", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                vars.push((name.to_string(), value.to_string_lossy().into_owned()));
            }
        }
        let temp = dir.to_string_lossy().into_owned();
        vars.push(("TEMP".to_string(), temp.clone()));
        vars.push(("TMP".to_string(), temp));
        vars
    }

    fn spec(dir: &std::path::Path) -> WorkerSpec {
        let script = dir.join("echo_worker.mjs");
        std::fs::write(&script, ECHO_WORKER).unwrap();
        WorkerSpec {
            program: PathBuf::from("node"),
            script,
            cwd: dir.to_path_buf(),
            env: node_env(dir),
            stderr_log: None,
            timeout: Duration::from_secs(30),
        }
    }

    #[test]
    fn hello_send_and_shutdown_round_trip() {
        if !node_available() {
            eprintln!("node недоступен — тест транспорта пропущен");
            return;
        }
        let dir = std::env::temp_dir().join(format!("hds-agent-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let worker = Worker::spawn(&spec(&dir), Arc::new(EchoHost)).unwrap();

        let ready = worker
            .call(|id| Down::Hello {
                id: id.to_string(),
                protocol: crate::protocol::PROTOCOL_VERSION,
            })
            .unwrap();
        match ready {
            Up::Ready { protocol, .. } => assert_eq!(protocol, 1),
            other => panic!("ожидался ready, получено {other:?}"),
        }

        let done = worker
            .call(|id| Down::Send {
                id: id.to_string(),
                text: "привет".to_string(),
            })
            .unwrap();
        match done {
            Up::Done { ok, result, .. } => {
                assert!(ok);
                assert_eq!(result["text"], "echo: привет");
            }
            other => panic!("ожидался done, получено {other:?}"),
        }

        let event = worker.next_event(Duration::from_secs(5)).unwrap();
        match event {
            Up::Event { event, .. } => assert_eq!(event["text"], "echo: привет"),
            other => panic!("ожидалось event, получено {other:?}"),
        }

        worker.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_call_is_answered_by_handler() {
        if !node_available() {
            eprintln!("node недоступен — тест host_call пропущен");
            return;
        }
        let dir = std::env::temp_dir().join(format!("hds-agent-hostcall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let worker = Worker::spawn(&spec(&dir), Arc::new(EchoHost)).unwrap();
        worker
            .call(|id| Down::Hello {
                id: id.to_string(),
                protocol: crate::protocol::PROTOCOL_VERSION,
            })
            .unwrap();
        // Воркер не понимает call_host как команду (его нет в Down) — пишем
        // строку напрямую, как это сделал бы сам воркер.
        {
            let mut stdin = worker.stdin.lock().unwrap();
            stdin
                .write_all(b"{\"type\":\"call_host\",\"id\":\"9\",\"text\":\"x\"}\n")
                .unwrap();
            stdin.flush().unwrap();
        }

        let event = worker.next_event(Duration::from_secs(5)).unwrap();
        match event {
            Up::Done { result, .. } => {
                assert_eq!(result["type"], "host_result");
                assert_eq!(result["ok"], true);
                assert_eq!(result["result"]["call"], "egress_check");
            }
            other => panic!("ожидался done от host_result, получено {other:?}"),
        }
        worker.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timeout_is_reported() {
        if !node_available() {
            eprintln!("node недоступен — тест таймаута пропущен");
            return;
        }
        let dir = std::env::temp_dir().join(format!("hds-agent-timeout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = spec(&dir);
        s.timeout = Duration::from_millis(300);
        let worker = Worker::spawn(&s, Arc::new(EchoHost)).unwrap();
        // Команда, на которую мини-воркер не отвечает.
        let err = worker
            .call(|id| Down::Abort { id: id.to_string() })
            .unwrap_err();
        assert!(matches!(err, WorkerError::Timeout(_)), "{err:?}");
        worker.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
