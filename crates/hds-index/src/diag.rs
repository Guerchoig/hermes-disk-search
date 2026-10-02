//! Диагностика окружения (порт `hds/diag.py::run_checks` «по смыслу») — общий код
//! для `hds check` (CLI) и `/api/diagnostics` (веб-интерфейс).
//!
//! Раньше жил в `hds-cli`, но `hds-cli` зависит от `hds-ui` (подкоманда `hds ui`) —
//! поэтому перенесён в `hds-index`, чтобы UI мог вызывать проверки **в процессе**,
//! без запуска второго бинаря.
//!
//! Покрываем: БД, корни, чат-роль, embedding-роль (+контекст), OCR (tesseract),
//! ffmpeg, лемматизатор (через `capabilities` воркера), реранк. Осознанно **не**
//! покрываем whisper/mpxj/vulkan — их проверяет Python-версия до W5.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hds_core::config::{self, dig, Config};
use hds_core::error::{CoreError, Result};
use hds_core::{db, http, EMB_CONTEXT};
use hds_extract::discover_python;
use serde_json::Value;

use crate::Sidecar;

/// Таймаут сетевого зонда (как `_PROBE_TIMEOUT = 3` в Python).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Idle-timeout воркера диагностики (проверка лемматизатора короткая, но 60 с
/// дефолта иногда мало на старте — держим как CLI: 3600 с).
const DIAG_WORKER_IDLE: Duration = Duration::from_secs(3600);

/// Один пункт проверки (поля как в `diag.run_checks`: `id/status/title/msg/fix`).
pub struct Check {
    pub id: &'static str,
    /// `ok` | `warn` | `fail`.
    pub status: &'static str,
    pub title: String,
    pub msg: String,
    pub fix: String,
}

impl Check {
    fn new(id: &'static str, status: &'static str, title: impl Into<String>) -> Check {
        Check {
            id,
            status,
            title: title.into(),
            msg: String::new(),
            fix: String::new(),
        }
    }

    fn msg(mut self, m: impl Into<String>) -> Check {
        self.msg = m.into();
        self
    }

    fn fix(mut self, f: impl Into<String>) -> Check {
        self.fix = f.into();
        self
    }
}

/// `embedding.dim` (по умолчанию 1024).
fn dim_of(cfg: &Config) -> i64 {
    dig(cfg, "embedding.dim")
        .and_then(|v| v.as_i64())
        .unwrap_or(1024)
}

/// Поиск исполняемого файла в `PATH` (порт `shutil.which` для одного имени).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// `tesseract` доступен: путь из `index.ocr_tesseract_cmd` или `PATH` (как `_tesseract_ready`).
pub fn tesseract_ready(cfg: &Config) -> bool {
    if let Some(cmd) = dig(cfg, "index.ocr_tesseract_cmd").and_then(|v| v.as_str()) {
        if !cmd.trim().is_empty() && Path::new(cmd.trim()).is_file() {
            return true;
        }
    }
    which("tesseract.exe").is_some() || which("tesseract").is_some()
}

/// Нормализация пути для сравнения (`normcase(normpath())`: общий разделитель + нижний регистр).
pub fn norm_path(s: &str) -> String {
    s.replace('\\', "/").to_lowercase()
}

/// Каталог общего llama-рантайма (`llama_runtime.runtime_dir`): env → LOCALAPPDATA/home.
pub fn runtime_dir() -> PathBuf {
    if let Ok(v) = std::env::var("LLAMA_RUNTIME_DIR") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(v) = std::env::var("LOCALAPPDATA") {
            if !v.is_empty() {
                return PathBuf::from(v).join("llama-runtime");
            }
        }
        if let Ok(h) = std::env::var("USERPROFILE") {
            return PathBuf::from(h)
                .join("AppData")
                .join("Local")
                .join("llama-runtime");
        }
        PathBuf::from("llama-runtime")
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("llama-runtime")
    }
}

/// Порт `llama_runtime.resolve_model` + `llama_server._abs_model`: путь GGUF роли.
pub fn resolve_model(cfg: &Config, role: &str) -> PathBuf {
    let spec = dig(cfg, &format!("llm_server.{role}.model"))
        .and_then(|v| v.as_str())
        .unwrap_or(role)
        .trim()
        .to_string();
    if let Some(rest) = spec.strip_prefix("shared:") {
        let r = if rest.trim().is_empty() {
            role
        } else {
            rest.trim()
        };
        let dir = runtime_dir().join("models").join(r);
        if let Some(name) = manifest_file(&dir.join("current.json")) {
            let p = dir.join(&name);
            if p.is_file() {
                return p;
            }
            if let Some(cand) = gguf_files(&dir)
                .into_iter()
                .find(|c| c.file_name().map(|n| n == name.as_str()).unwrap_or(false))
            {
                return cand;
            }
            return dir;
        }
        let ggufs = gguf_files(&dir);
        if ggufs.len() == 1 {
            return ggufs.into_iter().next().unwrap();
        }
        return dir;
    }
    let p = PathBuf::from(&spec);
    if p.is_absolute() {
        p
    } else {
        config::project_root().join(p)
    }
}

/// `*.gguf` в каталоге, отсортированные по имени (порт `sorted(d.glob("*.gguf"))`).
fn gguf_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("gguf"))
                        .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    v
}

/// `{"file": …}` из манифеста модели (UTF-8/BOM); `None` — нет/битый.
fn manifest_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(text).ok()?;
    v.get("file")
        .and_then(|f| f.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Порт пары `(host, port)` роли: `llm_server.host` (127.0.0.1) + `llm_server.<role>.port`.
pub fn role_addr(cfg: &Config, role: &str) -> (String, u16) {
    let host = dig(cfg, "llm_server.host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1")
        .to_string();
    let default = match role {
        "chat" => 8010,
        "embedding" => 8011,
        "rerank" => 8012,
        _ => 8010,
    };
    let port = dig(cfg, &format!("llm_server.{role}.port"))
        .and_then(|v| v.as_u64())
        .unwrap_or(default) as u16;
    (host, port)
}

/// Итог пробы роли (как `STATE_*` в `hds/llama_server.py`).
pub enum Probe {
    /// Живой сервер с ожидаемой моделью (наши `/health` + `/props.total_slots`).
    Llama(Value),
    /// Порт занят, но это не ожидаемый сервер (`/props` без `total_slots`/другая модель).
    Foreign,
    /// Никто не слушает.
    Down,
}

/// Порт `llama_server.probe`: `/health`, затем `/props` (сверка `total_slots` и `model_path`).
pub fn probe_role(cfg: &Config, role: &str, timeout: Duration) -> Probe {
    let (host, port) = role_addr(cfg, role);
    let healthy = http::request(&host, port, "GET", "/health", &[], None, timeout)
        .map(|r| (200..300).contains(&r.status))
        .unwrap_or(false);
    if !healthy {
        return Probe::Down;
    }
    let props = match http::request(&host, port, "GET", "/props", &[], None, timeout) {
        Ok(r) if (200..300).contains(&r.status) => r.json().unwrap_or(Value::Null),
        _ => return Probe::Foreign,
    };
    let slots = props
        .get("total_slots")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if slots < 1 {
        return Probe::Foreign;
    }
    let actual = props
        .get("model_path")
        .or_else(|| props.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !actual.is_empty() {
        let expected = resolve_model(cfg, role);
        if norm_path(actual) != norm_path(&expected.to_string_lossy()) {
            return Probe::Foreign;
        }
    }
    Probe::Llama(props)
}

/// Фактический контекст инстанса из `/props` (`props_context`).
pub fn props_context(props: &Value) -> Option<i64> {
    props
        .get("default_generation_settings")
        .and_then(|d| d.get("n_ctx"))
        .and_then(|v| v.as_i64())
        .or_else(|| props.get("n_ctx").and_then(|v| v.as_i64()))
}

/// Запуск Python-воркера извлечения/лемматизации (`sidecar/hds_extract/worker.py`).
fn build_sidecar(root: &Path) -> Result<Sidecar> {
    let py = discover_python(root).ok_or_else(|| {
        CoreError::Other(format!(
            "не найден интерпретатор воркера (sidecar/python или .venv\\Scripts\\python.exe) в {}",
            root.display()
        ))
    })?;
    Sidecar::spawn_with(&py, root, false, DIAG_WORKER_IDLE)
}

/// Проверка БД (пункт 1): `ok`, либо `fail`/`warn` (занята).
pub fn check_db(cfg: &Config) -> Check {
    let path = config::db_abs_path(cfg);
    match db::connect(&path, dim_of(cfg)) {
        Ok(_) => Check::new("db", "ok", format!("База данных: {}", path.display())),
        Err(e) => {
            let es = e.message();
            let locked = es.to_lowercase().contains("locked") && path.exists();
            if locked {
                Check::new(
                    "db",
                    "warn",
                    "База данных занята другим процессом (вероятно, идёт индексация)",
                )
            } else {
                Check::new("db", "fail", format!("База данных недоступна: {es}"))
                    .msg(format!("db_path: {}", path.display()))
                    .fix("Проверьте db_path в config.yaml или перенесите базу (hds db-move).")
            }
        }
    }
}

/// Проверка корней индексации (пункт 2): пусто/нет каталогов — `warn`.
pub fn check_roots(cfg: &Config) -> Check {
    let roots: Vec<String> = dig(cfg, "index.roots")
        .and_then(|v| v.as_sequence())
        .map(|s| {
            s.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if roots.is_empty() {
        return Check::new("roots", "warn", "Корни индексации не заданы")
            .fix("Добавьте диски/папки в config.yaml (index.roots).");
    }
    let missing: Vec<String> = roots
        .iter()
        .filter(|r| !Path::new(r).is_dir())
        .cloned()
        .collect();
    if missing.is_empty() {
        Check::new(
            "roots",
            "ok",
            format!("Корни индексации: {}", roots.join(", ")),
        )
    } else {
        Check::new(
            "roots",
            "warn",
            format!("Корни индексации не существуют: {}", missing.join(", ")),
        )
        .fix("Поправьте index.roots — укажите существующий диск/папку.")
    }
}

/// Пункт 3: чат-роль (не критично для поиска).
fn check_chat(cfg: &Config) -> Check {
    let model = dig(cfg, "chat.model")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    match probe_role(cfg, "chat", PROBE_TIMEOUT) {
        Probe::Llama(_) => Check::new(
            "chat",
            "ok",
            format!("Чат-сервер llama.cpp отвечает ('{model}')"),
        ),
        Probe::Foreign => {
            let port = role_addr(cfg, "chat").1;
            Check::new(
                "chat",
                "warn",
                format!("Порт {port} занят посторонним сервисом (не чат-роль llama-server)"),
            )
            .fix("Освободите порт или смените llm_server.chat.port в config.yaml.")
        }
        Probe::Down => Check::new(
            "chat",
            "warn",
            "Чат-сервер не запущен — ask_my_files без LLM-ответа (только список файлов)",
        )
        .fix("Запустите владельца портов: bin\\llm_host.exe run (или llm-host task)"),
    }
}

/// Пункт 4: embedding-роль (критично для поиска).
fn emb_check(cfg: &Config, probe: &Probe) -> Check {
    let model = dig(cfg, "embedding.model")
        .and_then(|v| v.as_str())
        .unwrap_or("text-embedding-bge-m3");
    match probe {
        Probe::Llama(_) => Check::new(
            "emb",
            "ok",
            format!("Эмбеддинги: llama-server отвечает ('{model}')"),
        ),
        Probe::Foreign => {
            let port = role_addr(cfg, "embedding").1;
            Check::new(
                "emb",
                "fail",
                format!("Порт {port} занят посторонним сервисом (не роль embedding)"),
            )
            .fix("Освободите порт или смените llm_server.embedding.port в config.yaml.")
        }
        Probe::Down => {
            let path = resolve_model(cfg, "embedding");
            if !path.is_file() {
                Check::new(
                    "emb",
                    "fail",
                    format!("GGUF-модель эмбеддингов не найдена: {}", path.display()),
                )
                .fix("Скачайте модель в общий llama-рантайм или запустите установщик рантайма.")
            } else {
                Check::new("emb", "fail", "Эмбеддинг-сервер llama.cpp не запущен").fix(
                    "Запустите владельца портов: bin\\llm_host.exe run. \
                     Без него поиск работает только по ключевым словам.",
                )
            }
        }
    }
}

/// Пункт 4b: контекст embedding-инстанса меньше `EMB_CONTEXT` → предупреждение.
fn emb_ctx_check(probe: &Probe) -> Option<Check> {
    let props = match probe {
        Probe::Llama(p) => p,
        _ => return None,
    };
    let ctx = props_context(props)?;
    if ctx < EMB_CONTEXT {
        Some(
            Check::new(
                "embctx",
                "warn",
                format!(
                    "Эмбеддинг-инстанс запущен с контекстом {ctx} (нужно {EMB_CONTEXT}) — \
                     длинные фрагменты индексируются неполно"
                ),
            )
            .fix(format!(
                "Контекст задаёт llm_server.embedding.ctx_per_slot ({EMB_CONTEXT}): \
                 перезапустите роль, затем переиндексация."
            )),
        )
    } else {
        None
    }
}

/// Пункт 5: Tesseract OCR.
fn check_ocr(cfg: &Config) -> Check {
    if tesseract_ready(cfg) {
        Check::new("ocr", "ok", "Tesseract OCR найден")
    } else {
        Check::new(
            "ocr",
            "warn",
            "Tesseract OCR не найден — картинки и сканы без текстового слоя не индексируются",
        )
        .fix("Установите Tesseract (с языком 'rus') и укажите index.ocr_tesseract_cmd.")
    }
}

/// Пункт 6: ffmpeg.
fn check_ffmpeg() -> Check {
    if which("ffmpeg.exe").is_some() || which("ffmpeg").is_some() {
        Check::new("ffmpeg", "ok", "ffmpeg найден")
    } else {
        Check::new(
            "ffmpeg",
            "warn",
            "ffmpeg не найден — видео без транскрипции",
        )
        .fix("Установите Gyan.FFmpeg и перезапустите watcher/индексацию.")
    }
}

/// Возможности воркера одним запуском: `(normalize, mpp)` (пункты 7c и 8 `diag.run_checks`).
fn worker_caps() -> Result<(bool, bool)> {
    let root = config::project_root();
    let sc = build_sidecar(&root)?;
    let caps = sc.capabilities();
    let norm = caps.has("normalize");
    let mpp = caps.has("mpp");
    sc.shutdown();
    Ok((norm, mpp))
}

/// Пункт 7c: лемматизатор (`normalize`).
fn check_lemmatizer(has: bool) -> Check {
    if has {
        Check::new(
            "lemmatizer",
            "ok",
            "pymorphy3 установлен — русская морфология в ключевом поиске",
        )
    } else {
        Check::new(
            "lemmatizer",
            "warn",
            "pymorphy3 не установлен — ключевой поиск без русской морфологии",
        )
        .fix("Установите pymorphy3 + pymorphy3-dicts-ru, затем hds reindex-fts.")
    }
}

/// Пункт 8 (`diag.run_checks`): MS Project `.mpp` через mpxj/Java.
fn check_mpp(has: bool) -> Check {
    if has {
        Check::new(
            "mpp",
            "ok",
            "mpxj (MS Project) установлен — файлы .mpp индексируются",
        )
    } else {
        Check::new("mpp", "warn", "mpxj/Java нет — файлы .mpp не индексируются")
            .fix("Соберите sidecar с mpxj (installers\\build_sidecar.ps1) или установите Java 11+.")
    }
}

/// Пункт 7d: реранкер (только если включён в конфиге).
fn check_rerank(cfg: &Config) -> Option<Check> {
    if !dig(cfg, "rerank.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let url = dig(cfg, "rerank.url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://localhost:8012/v1")
        .trim_end_matches('/')
        .to_string();
    let ok = crate::embed::split_base(&url)
        .ok()
        .map(|(host, port, _)| {
            http::request(&host, port, "GET", "/health", &[], None, PROBE_TIMEOUT)
                .map(|r| r.status == 200)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if ok {
        Some(Check::new(
            "rerank",
            "ok",
            "Реранкер (llama-server) отвечает",
        ))
    } else {
        Some(
            Check::new(
                "rerank",
                "warn",
                "rerank.enabled включён, но реранкер не отвечает — ask_my_files без реранкинга",
            )
            .fix("Запустите роль реранкера (владелец портов — llm-host)."),
        )
    }
}

/// `gpu-observability` (L1, `W4_REPORT.md` §15): видно ли, **кто держит VRAM** и не
/// завис ли движок.
///
/// Источник — heartbeat резидента (`data/llm-host.heartbeat.json`, его пишет отдельный
/// поток `llm-host`), поэтому проверка работает **даже когда HTTP резидента молчит**
/// (ровно тот случай, который в инциденте §14 выглядел как «резидент не отвечает»).
/// Отдельно смотрим свежесть лога резидента: heartbeat живой, а лог молчит — движок
/// держит вызов.
pub fn check_gpu_observability() -> Check {
    let root = hds_core::config::project_root();
    let hb_path = root.join("data").join("llm-host.heartbeat.json");
    let log_path = root.join("data").join("logs").join("llm-host.log");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let log_age = std::fs::metadata(&log_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| now.saturating_sub(d.as_secs()));

    let title = "GPU: кто держит VRAM и занят ли движок";
    let hb: Option<serde_json::Value> = std::fs::read_to_string(&hb_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let Some(hb) = hb else {
        return Check::new("gpu-observability", "warn", title)
            .msg(format!(
                "heartbeat {} не найден — резидент не запущен или это сборка до L1",
                hb_path.display()
            ))
            .fix("запустите `llm_host run` (наблюдаемость L1, `W4_REPORT.md` §15)");
    };

    let ts = hb.get("ts_unix").and_then(|v| v.as_u64()).unwrap_or(0);
    let age = now.saturating_sub(ts);
    let busy = hb
        .get("busy")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let ours = hb.get("vram_ours_mib").and_then(|v| v.as_u64());
    let foreign = hb.get("vram_foreign_mib").and_then(|v| v.as_u64());
    let fresh = age <= 30;

    let mut check = Check::new(
        "gpu-observability",
        if fresh { "ok" } else { "warn" },
        title,
    )
    .msg(format!(
        "heartbeat {} с назад; движок: {}; VRAM: наш процесс {} МиБ, чужие {} МиБ; лог резидента {} с назад",
        age,
        busy.clone().unwrap_or_else(|| "свободен".into()),
        ours.map(|v| v.to_string())
            .unwrap_or_else(|| "—".into()),
        foreign
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into()),
        log_age
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".into()),
    ));
    if !fresh {
        check = check.fix(
            "резидент не пишет heartbeat: `llm_host status` покажет диагноз, \
             `llm_host stop --force` снимает зависший (`W4_REPORT.md` §14)",
        );
    }
    if busy.is_some() {
        check =
            check.fix("движок занят: если это долго — `llm_host stop --force` и `llm_host run`");
    }
    check
}

/// Все проверки (композиция; сеть/воркер — здесь, чистые части — отдельно).
pub fn run_checks(cfg: &Config) -> Vec<Check> {
    let mut checks = vec![check_db(cfg), check_roots(cfg), check_chat(cfg)];

    // embedding-роль пробуем один раз — переиспользуем для контекста (как Python)
    let emb_probe = probe_role(cfg, "embedding", PROBE_TIMEOUT);
    checks.push(emb_check(cfg, &emb_probe));
    if let Some(c) = emb_ctx_check(&emb_probe) {
        checks.push(c);
    }

    checks.push(check_ocr(cfg));
    checks.push(check_ffmpeg());
    match worker_caps() {
        Ok((norm, mpp)) => {
            checks.push(check_lemmatizer(norm));
            checks.push(check_mpp(mpp));
        }
        Err(e) => {
            checks.push(check_lemmatizer(false).msg(e.message()));
            checks.push(check_mpp(false).msg(e.message()));
        }
    }
    if let Some(c) = check_rerank(cfg) {
        checks.push(c);
    }

    checks.push(check_gpu_observability());
    checks.push(
        Check::new(
            "gpu-manual",
            "warn",
            "whisper / Vulkan проверяются вручную (GPU-чек-лист W3)",
        )
        .msg("Аппаратные пути (CUDA/Vulkan/Metal) в CI не проверяются: нет GPU/раннера."),
    );
    checks
}
