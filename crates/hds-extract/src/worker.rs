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
    /// Дополнительные аргументы воркера (транскрибация: `--profile cuda`).
    pub extra_args: Vec<String>,
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
            extra_args: Vec::new(),
        }
    }
}

/// Поиск интерпретатора воркера извлечения: `HDS_EXTRACT_PYTHON` → `sidecar/python` → `.venv`.
pub fn discover_python(root: &Path) -> Option<PathBuf> {
    discover_python_with(root, "HDS_EXTRACT_PYTHON", None)
}

/// Обобщённый поиск интерпретатора (транскрибация использует свои env/venv).
///
/// Порядок: `<env_var>` → `sidecar/<venv_rel>` (venv профиля) → `sidecar/python`
/// (портативный CPython) → `.venv` проекта (только разработка).
pub fn discover_python_with(root: &Path, env_var: &str, venv_rel: Option<&str>) -> Option<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        if !p.is_empty() {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Some(pb);
            }
        }
    }
    if let Some(rel) = venv_rel {
        let venv = root.join("sidecar").join(rel);
        if venv.join("pyvenv.cfg").exists() {
            for name in ["Scripts/python.exe", "bin/python3", "bin/python"] {
                let py = venv.join(name);
                if py.exists() {
                    return Some(py);
                }
            }
        }
    }
    if let Some(p) = find_python_exe(&root.join("sidecar").join("python")) {
        return Some(p);
    }
    // Фолбэк для разработки: `.venv` проекта. Берём только ЦЕЛЫЙ venv: лаунчер
    // `Scripts\python.exe` требует рядом `pyvenv.cfg` и без него падает с
    // «failed to locate pyvenv.cfg» — недоделанный/обрушенный venv на машине
    // разработчика выглядел как «рабочий» интерпретатор, и воркер не поднимался
    // с непонятной ошибкой («поток вывода закрыт (процесс упал?)»).
    // РЕГРЕССИЯ (13.10.2026): релизная приёмка падала на тесте mock-воркера —
    // в репозитории лежал `.venv` без `pyvenv.cfg`.
    let venv = root.join(".venv");
    if venv.join("pyvenv.cfg").exists() {
        // Windows — `Scripts/python.exe`, macOS/Linux — `bin/python3`.
        for rel in [
            ["Scripts", "python.exe"],
            ["bin", "python3"],
            ["bin", "python"],
        ] {
            let py = venv.join(rel[0]).join(rel[1]);
            if py.exists() {
                return Some(py);
            }
        }
    }
    None
}

/// Поиск интерпретатора в дереве (берём с кратчайшим путём — корень сборки, а не
/// шаблон venv).
///
/// Имена: `python.exe` (Windows-поставка) и `python3`/`python`
/// (macOS/Linux: `uv python install --install-dir` кладёт `python/bin/python3`, а
/// не `python.exe` — без этого поставленный sidecar не находился на macOS).
fn find_python_exe(dir: &Path) -> Option<PathBuf> {
    fn is_python_name(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "python.exe" | "python3" | "python"
        )
    }

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
                .and_then(|n| n.to_str())
                .map(is_python_name)
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
    /// Последняя ошибка JSON-RPC (машинный `kind` для `hds-transcribe`).
    last_error: Option<protocol::RpcError>,
}

/// Чем кончилась попытка запроса (`call_once`) — от этого зависит, поднимать ли
/// процесс заново и повторять ли запрос.
enum CallFail {
    /// Канал мёртв: запись в закрытый stdin, EOF на stdout или stdin уже снят
    /// (процесс убит/упал). Запрос с большой вероятностью **не** обработан —
    /// после перезапуска его можно повторить.
    Dead(CoreError),
    /// Таймаут запроса: процесс уже убит, файл получает `error` (повтор удвоил бы
    /// ожидание); следующий запрос поднимет воркер заново.
    Timeout(CoreError),
    /// Ошибка протокола/запуска: процесс жив или поднять его не удалось — повтор
    /// бессмыслен.
    Other(CoreError),
}

impl CallFail {
    fn into_error(self) -> CoreError {
        match self {
            CallFail::Dead(e) | CallFail::Timeout(e) | CallFail::Other(e) => e,
        }
    }
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
            last_error: None,
        };
        w.handshake()?;
        Ok(w)
    }

    /// Возможности воркера (после `hello`).
    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// Последняя ошибка JSON-RPC (в т.ч. машинный `kind`) — для `hds-transcribe`.
    pub fn last_error(&self) -> Option<&protocol::RpcError> {
        self.last_error.as_ref()
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
        self.last_error = None;
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

    /// Один запрос: NDJSON → строка → разбор. Упавший воркер поднимается заново
    /// и запрос повторяется **один** раз ([`CallFail::Dead`]).
    ///
    /// Зачем: раньше запись в закрытый канал (воркер упал) возвращала ошибку
    /// навсегда — `stdin` оставался мёртвым, и **каждый** следующий файл падал с
    /// «воркер: запись: Идёт закрытие канала» (релиз 0.20.5: один `FileNotFoundError`
    /// конфига в Python-воркере → 43 537 «error» в БД за один прогон). Таймаут
    /// по-прежнему отдаёт ошибку файла без повтора (повтор удвоил бы ожидание).
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request(method, params, true)
    }

    /// Публичный запрос с управлением повтором (§5.4).
    ///
    /// `retry_dead = true` — падение канала (процесс упал/закрыт) поднимает
    /// воркер и повторяет запрос **один** раз (извлечение: запрос идемпотентен).
    /// `false` — для транскрибации: падение посреди 30-минутного задания не
    /// повторяем внутри вызова (повтор — на уровне демона, `max_attempts`).
    pub fn request(&mut self, method: &str, params: Value, retry_dead: bool) -> Result<Value> {
        match self.call_once(method, &params) {
            Ok(v) => Ok(v),
            Err(CallFail::Dead(_)) if retry_dead => {
                self.restart()?;
                self.call_once(method, &params)
                    .map_err(CallFail::into_error)
            }
            Err(f) => Err(f.into_error()),
        }
    }

    /// Попытка запроса без перезапуска: чем именно всё кончилось.
    fn call_once(&mut self, method: &str, params: &Value) -> std::result::Result<Value, CallFail> {
        if let Err(e) = self.maybe_restart() {
            return Err(CallFail::Other(e));
        }
        let id = self.next_id;
        self.next_id += 1;
        let line = protocol::request(id, method, params.clone());
        let stdin = match self.stdin.as_mut() {
            // stdin снят (kill после таймаута/остановки) — процесс мёртв.
            None => {
                return Err(CallFail::Dead(CoreError::Other(
                    "воркер: stdin закрыт".into(),
                )))
            }
            Some(s) => s,
        };
        if let Err(e) = stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
        {
            return Err(CallFail::Dead(CoreError::Other(format!(
                "воркер: запись: {e}"
            ))));
        }
        self.requests += 1;
        self.last_error = None;
        match self.rx.recv_timeout(self.cfg.request_timeout) {
            Ok(resp) => match protocol::parse_response(&resp, id) {
                Ok(v) => Ok(v),
                Err(e) => {
                    // Машинный `kind` нужен вызывающему (model_missing/cuda_oom/…).
                    self.last_error = protocol::error_from_line(&resp);
                    Err(CallFail::Other(e))
                }
            },
            Err(RecvTimeoutError::Timeout) => {
                self.kill();
                Err(CallFail::Timeout(CoreError::Other(format!(
                    "воркер: таймаут {method} ({:?}) — процесс перезапущен",
                    self.cfg.request_timeout
                ))))
            }
            Err(RecvTimeoutError::Disconnected) => Err(CallFail::Dead(CoreError::Other(
                "воркер: поток вывода закрыт (процесс упал?)".into(),
            ))),
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
        .args(&cfg.extra_args)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Свой временный каталог (внешних зависимостей у крейта нет — tempfile
    /// не тянем).
    fn tmp_root(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("hds-extract-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Явный `HDS_EXTRACT_PYTHON` сильнее всего — тесты поиска тогда не имеют смысла.
    fn skip_if_env_python() -> bool {
        std::env::var("HDS_EXTRACT_PYTHON").is_ok_and(|v| !v.is_empty())
    }

    /// РЕГРЕССИЯ (13.10.2026): `.venv` без `pyvenv.cfg` — не интерпретатор.
    ///
    /// Лаунчер `Scripts\python.exe` требует `pyvenv.cfg` рядом и без него падает с
    /// «failed to locate pyvenv.cfg»; воркер не поднимался, а релизная приёмка
    /// падала на тесте mock-воркера. Такой venv пропускаем (интерпретатора нет),
    /// целый — берём.
    #[test]
    fn broken_venv_is_not_an_interpreter() {
        if skip_if_env_python() {
            println!("пропуск: задан HDS_EXTRACT_PYTHON");
            return;
        }
        let root = tmp_root("broken-venv");
        let scripts = root.join(".venv").join("Scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("python.exe"), b"").unwrap();
        assert_eq!(
            discover_python(&root),
            None,
            "обрушенный venv (нет pyvenv.cfg) не годится как интерпретатор"
        );

        std::fs::write(root.join(".venv").join("pyvenv.cfg"), b"home = x").unwrap();
        assert_eq!(
            discover_python(&root),
            Some(scripts.join("python.exe")),
            "целый venv — валидный фолбэк для разработки"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Поставка (`sidecar/python`) приоритетнее dev-venv.
    #[test]
    fn shipped_python_wins_over_venv() {
        if skip_if_env_python() {
            println!("пропуск: задан HDS_EXTRACT_PYTHON");
            return;
        }
        let root = tmp_root("sidecar-priority");
        let shipped = root.join("sidecar").join("python").join("portable");
        std::fs::create_dir_all(&shipped).unwrap();
        std::fs::write(shipped.join("python.exe"), b"").unwrap();
        let venv_scripts = root.join(".venv").join("Scripts");
        std::fs::create_dir_all(&venv_scripts).unwrap();
        std::fs::write(venv_scripts.join("python.exe"), b"").unwrap();
        std::fs::write(root.join(".venv").join("pyvenv.cfg"), b"home = x").unwrap();

        assert_eq!(discover_python(&root), Some(shipped.join("python.exe")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// macOS-раскладка поставки (`sidecar/python/bin/python3` — без `.exe`):
    /// интерпретатор тоже находится (раньше искали только `python.exe`, и на
    /// macOS поставленный sidecar не подхватывался).
    #[test]
    fn shipped_python3_is_found_without_exe_suffix() {
        if skip_if_env_python() {
            println!("пропуск: задан HDS_EXTRACT_PYTHON");
            return;
        }
        let root = tmp_root("sidecar-python3");
        let shipped = root.join("sidecar").join("python").join("bin");
        std::fs::create_dir_all(&shipped).unwrap();
        std::fs::write(shipped.join("python3"), b"").unwrap();

        assert_eq!(discover_python(&root), Some(shipped.join("python3")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
