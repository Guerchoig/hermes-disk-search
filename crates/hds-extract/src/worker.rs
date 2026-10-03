//! Клиент sidecar-воркера: запуск, `hello`, запросы, перезапуск, остановка.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use hds_core::error::{CoreError, Result};
use serde_json::{json, Value};

use crate::protocol::{self, Capabilities, ExtractResult, PROTOCOL_VERSION};

/// Параметры воркера (по умолчанию — §5.1: idle 60 с, таймаут запроса 600 с).
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub python: PathBuf,
    pub root: PathBuf,
    pub script: PathBuf,
    pub idle_timeout: Duration,
    pub request_timeout: Duration,
    /// Перезапуск после N запросов (0 — без перезапуска; §5.1 «после M файлов»).
    pub max_requests: u64,
    /// Паритетный режим извлечения (правила golden.py) — для тестов паритета B4.
    pub parity: bool,
    /// Файл для stderr воркера (None — наследуется родителем).
    pub stderr_log: Option<PathBuf>,
}

impl WorkerConfig {
    /// Конфиг для проекта: `root/sidecar/hds_extract/worker.py`.
    pub fn new(python: &Path, root: &Path) -> Self {
        WorkerConfig {
            python: python.to_path_buf(),
            root: root.to_path_buf(),
            script: root.join("sidecar").join("hds_extract").join("worker.py"),
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(600),
            max_requests: 0,
            parity: false,
            stderr_log: None,
        }
    }
}

/// Поиск интерпретатора воркера: `HDS_EXTRACT_PYTHON` → `sidecar/python` (A) → `.venv` (C).
pub fn discover_python(root: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HDS_EXTRACT_PYTHON") {
        if !p.is_empty() {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Some(pb);
            }
        }
    }
    if let Some(p) = find_python_exe(&root.join("sidecar").join("python")) {
        return Some(p);
    }
    let venv = root.join(".venv").join("Scripts").join("python.exe");
    if venv.exists() {
        return Some(venv);
    }
    None
}

/// Поиск `python.exe` в дереве (берём с кратчайшим путём — корень сборки, а не шаблон venv).
fn find_python_exe(dir: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, best: &mut Option<PathBuf>) {
        let rd = match std::fs::read_dir(dir) {
            Ok(r) => r,
            Err(_) => return,
        };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                walk(&p, best);
            } else if p
                .file_name()
                .map(|n| n.eq_ignore_ascii_case("python.exe"))
                .unwrap_or(false)
            {
                let better = match best {
                    Some(b) => p.to_string_lossy().len() < b.to_string_lossy().len(),
                    None => true,
                };
                if better {
                    *best = Some(p);
                }
            }
        }
    }
    let mut best = None;
    if dir.is_dir() {
        walk(dir, &mut best);
    }
    best
}

/// Запущенный воркер (родитель владеет процессом).
pub struct Worker {
    cfg: WorkerConfig,
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    next_id: i64,
    requests: u64,
    caps: Capabilities,
    started: Instant,
}

impl Worker {
    /// Запуск воркера + рукопожатие `hello`.
    pub fn spawn(cfg: WorkerConfig) -> Result<Worker> {
        if !cfg.script.exists() {
            return Err(CoreError::Other(format!(
                "нет скрипта воркера: {}",
                cfg.script.display()
            )));
        }
        let (child, stdin, rx) = start_process(&cfg)?;
        let mut w = Worker {
            cfg,
            child,
            stdin: Some(stdin),
            rx,
            next_id: 1,
            requests: 0,
            caps: Capabilities::default(),
            started: Instant::now(),
        };
        w.handshake()?;
        Ok(w)
    }

    /// Возможности воркера (после `hello`).
    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// PID процесса воркера (для замера RSS по дереву процессов, §5.1).
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Время с запуска (для логов).
    pub fn started(&self) -> Instant {
        self.started
    }

    /// Интерпретатор воркера (для сообщений/тестов).
    pub fn python(&self) -> &Path {
        &self.cfg.python
    }

    /// Живой ли процесс.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn handshake(&mut self) -> Result<()> {
        let v = self.call("hello", json!({"protocol": PROTOCOL_VERSION}))?;
        let caps = protocol::capabilities_from(&v);
        if caps.protocol != PROTOCOL_VERSION {
            return Err(CoreError::Other(format!(
                "воркер: протокол {} (ждали {})",
                caps.protocol, PROTOCOL_VERSION
            )));
        }
        self.caps = caps;
        Ok(())
    }

    /// Перезапуск: убить текущий процесс, поднять новый, повторить `hello`.
    pub fn restart(&mut self) -> Result<()> {
        self.kill();
        let (child, stdin, rx) = start_process(&self.cfg)?;
        self.child = child;
        self.stdin = Some(stdin);
        self.rx = rx;
        self.next_id = 1;
        self.requests = 0;
        self.handshake()
    }

    fn maybe_restart(&mut self) -> Result<()> {
        if self.cfg.max_requests > 0 && self.requests >= self.cfg.max_requests {
            self.restart()?;
        }
        Ok(())
    }

    fn kill(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Один запрос: NDJSON → строка → разбор (таймаут → ошибка, процесс убит).
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.maybe_restart()?;
        let id = self.next_id;
        self.next_id += 1;
        let line = protocol::request(id, method, params);
        {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| CoreError::Other("воркер: stdin закрыт".into()))?;
            stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .map_err(|e| CoreError::Other(format!("воркер: запись: {e}")))?;
        }
        self.requests += 1;
        match self.rx.recv_timeout(self.cfg.request_timeout) {
            Ok(resp) => protocol::parse_response(&resp, id),
            Err(RecvTimeoutError::Timeout) => {
                self.kill();
                Err(CoreError::Other(format!(
                    "воркер: таймаут {method} ({:?}) — процесс перезапущен",
                    self.cfg.request_timeout
                )))
            }
            Err(RecvTimeoutError::Disconnected) => Err(CoreError::Other(
                "воркер: поток вывода закрыт (процесс упал?)".into(),
            )),
        }
    }

    /// `extract`: сегменты файла (`kind`, `segments`, `warnings`, `elapsed_ms`).
    pub fn extract(&mut self, path: &Path) -> Result<ExtractResult> {
        let params = json!({
            "path": path.to_string_lossy(),
            "opts": {"parity": self.cfg.parity},
        });
        let v = self.call("extract", params)?;
        Ok(protocol::extract_from(&v))
    }

    /// `normalize`: пакетная лемматизация текстов (для `chunks_fts` и запросов).
    pub fn normalize(&mut self, texts: &[String]) -> Result<Vec<String>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let v = self.call("normalize", json!({"texts": texts}))?;
        Ok(protocol::lemmas_from(&v))
    }

    /// `clip_image`: векторы картинок. Воркер отвечает ошибкой (CLIP — в Rust, §7).
    pub fn clip_image(&mut self, paths: &[PathBuf]) -> Result<Vec<Vec<f32>>> {
        let list: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let v = self.call("clip_image", json!({"paths": list}))?;
        Ok(v.get("vectors")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|row| {
                        row.as_array()
                            .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Штатная остановка: `shutdown` → закрыть stdin → подождать выхода (до 5 с) → kill.
    pub fn shutdown(&mut self) {
        let _ = self.call("shutdown", json!({}));
        self.stdin = None; // EOF на stdin — основной путь остановки (§5.1)
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            if Instant::now() >= deadline {
                self.kill();
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Worker {
    /// Родитель владеет процессом: при своём завершении убиваем воркер (§5.1).
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Запуск процесса + поток-читатель строк stdout.
fn start_process(cfg: &WorkerConfig) -> Result<(Child, ChildStdin, Receiver<String>)> {
    let mut cmd = Command::new(&cfg.python);
    cmd.arg(&cfg.script)
        .arg("--root")
        .arg(&cfg.root)
        .arg("--idle-timeout")
        .arg(cfg.idle_timeout.as_secs().to_string())
        .current_dir(&cfg.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    // Windows: консольный `python.exe` не должен вспыхивать окном. Родитель
    // (детачед MCP-сервер/UI) может не иметь консоли — тогда Windows создаёт
    // ребёнку НОВУЮ консоль, и каждый запуск воркера мигал окном на экране
    // (замечено: Cline → каждый поиск поднимал видимую консоль).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match &cfg.stderr_log {
        Some(p) => {
            let f = std::fs::File::create(p)
                .map_err(|e| CoreError::Other(format!("stderr-лог {}: {e}", p.display())))?;
            cmd.stderr(Stdio::from(f));
        }
        None => {
            cmd.stderr(Stdio::inherit());
        }
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Other(format!("запуск воркера {}: {e}", cfg.python.display())))?;
    let stdin = child.stdin.take().expect("stdin воркера");
    let stdout = child.stdout.take().expect("stdout воркера");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    Ok((child, stdin, rx))
}
