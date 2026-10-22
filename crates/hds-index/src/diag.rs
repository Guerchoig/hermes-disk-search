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
use hds_core::roots::roots_from_cfg;
use hds_core::{db, http, tools, EMB_CONTEXT};
use hds_extract::discover_python;
use serde_json::Value;

use crate::Sidecar;
use crate::{pipeline, walk};

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
///
/// Реализация переехала в `hds_core::tools` (`PLAN_DIARIZATION_NEMOTRON.md` W0):
/// тот же поиск нужен `hds-diarize`/`llm_host`, которые не зависят от `hds-index`.
pub use hds_core::tools::which;

/// Стандартные каталоги установки `tesseract` (кроме `PATH`).
///
/// Нужны потому, что службы (`LocalSystem`/LaunchDaemon) стартуют с минимальным
/// `PATH`: `tesseract.exe` из `C:\Program Files\Tesseract-OCR` (машинная
/// установка `UB-Mannheim.TesseractOCR`) в нём отсутствует, и проверка ложно
/// сообщала «не найден» (живой инцидент 15.10.2026, релиз 0.20.1).
#[cfg(windows)]
fn standard_tesseract_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for key in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(key) {
            v.push(
                PathBuf::from(pf)
                    .join("Tesseract-OCR")
                    .join("tesseract.exe"),
            );
        }
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        v.push(
            PathBuf::from(&la)
                .join("Programs")
                .join("Tesseract-OCR")
                .join("tesseract.exe"),
        );
        v.push(
            PathBuf::from(&la)
                .join("Tesseract-OCR")
                .join("tesseract.exe"),
        );
    }
    v
}

#[cfg(not(windows))]
fn standard_tesseract_paths() -> Vec<PathBuf> {
    [
        "/opt/homebrew/bin/tesseract",
        "/usr/local/bin/tesseract",
        "/usr/bin/tesseract",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// Путь к `tesseract`: `index.ocr_tesseract_cmd` → `PATH` → стандартные каталоги.
pub fn tesseract_exe(cfg: &Config) -> Option<PathBuf> {
    if let Some(cmd) = dig(cfg, "index.ocr_tesseract_cmd").and_then(|v| v.as_str()) {
        let p = Path::new(cmd.trim());
        if !cmd.trim().is_empty() && p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    // `hds_core::tools::which` сам пробует `.exe` на Windows.
    if let Some(p) = which("tesseract") {
        return Some(p);
    }
    standard_tesseract_paths().into_iter().find(|p| p.is_file())
}

/// `tesseract` доступен (бинарь найден — путь из конфига, `PATH` или стандартный каталог).
pub fn tesseract_ready(cfg: &Config) -> bool {
    tesseract_exe(cfg).is_some()
}

/// Языки OCR из `index.ocr_lang` (`"rus+eng"` → `["rus", "eng"]`).
pub fn ocr_langs(cfg: &Config) -> Vec<String> {
    dig(cfg, "index.ocr_lang")
        .and_then(|v| v.as_str())
        .unwrap_or("rus+eng")
        .split('+')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Каталоги `tessdata` в порядке поиска: `index.ocr_tessdata` → рядом с бинарём
/// → пользовательский (как `hds/extractors.py`).
pub fn tessdata_dirs(cfg: &Config, tess: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = dig(cfg, "index.ocr_tessdata").and_then(|x| x.as_str()) {
        if !d.trim().is_empty() {
            v.push(PathBuf::from(d.trim()));
        }
    }
    if let Some(dir) = tess.parent() {
        v.push(dir.join("tessdata"));
        // macOS (Homebrew): bin/../share/tessdata
        if let Some(prefix) = dir.parent() {
            v.push(prefix.join("share").join("tessdata"));
        }
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(la).join("Tesseract-OCR").join("tessdata"));
    }
    v
}

/// Языковые пакеты из `langs`, отсутствующие во **всех** каталогах `tessdata`.
pub fn missing_langs(cfg: &Config, tess: &Path) -> Vec<String> {
    let dirs = tessdata_dirs(cfg, tess);
    ocr_langs(cfg)
        .into_iter()
        .filter(|lang| {
            !dirs
                .iter()
                .any(|d| d.join(format!("{lang}.traineddata")).is_file())
        })
        .collect()
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
    // Рантайм (`hds-llama`) при отсутствии ключа берёт `shared:<role>`; диагностика
    // обязана разрешать так же. Иначе минимальный server-конфиг установщика (без
    // `llm_server.<role>.model`) даёт путь `<state>/<role>` и ложный fail в `hds check`.
    let spec = dig(cfg, &format!("llm_server.{role}.model"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("shared:{role}"));
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
///
/// Пишущее соединение (`open_rw`) — пункт должен создать схему на свежей базе
/// и показать фактические PRAGMA (M4 T1: journal_mode/busy_timeout — числа).
pub fn check_db(cfg: &Config) -> Check {
    let path = config::db_abs_path(cfg);
    let opts = db::DbOptions {
        busy_timeout_ms: config::db_busy_timeout_ms(cfg),
        journal_mode: config::db_journal_mode(cfg),
    };
    match db::open_rw(&path, dim_of(cfg), &opts) {
        Ok(conn) => {
            let jm: String = conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .unwrap_or_default();
            let bt: i64 = conn
                .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                .unwrap_or(0);
            Check::new(
                "db",
                "ok",
                format!(
                    "База данных: {} (journal_mode={}, busy_timeout={} мс)",
                    path.display(),
                    jm,
                    bt
                ),
            )
        }
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
/// M1 (§3.1): показывает «корень → проект», предупреждает о дублях корня.
pub fn check_roots(cfg: &Config) -> Check {
    let roots = roots_from_cfg(cfg);
    if roots.is_empty() {
        return Check::new("roots", "warn", "Корни индексации не заданы")
            .fix("Добавьте диски/папки в config.yaml (index.roots).");
    }
    let missing: Vec<String> = roots
        .iter()
        .filter(|r| !Path::new(&r.path).is_dir())
        .map(|r| r.path.clone())
        .collect();
    if !missing.is_empty() {
        return Check::new(
            "roots",
            "warn",
            format!("Корни индексации не существуют: {}", missing.join(", ")),
        )
        .fix("Поправьте index.roots — укажите существующий диск/папку.");
    }
    let list = roots
        .iter()
        .map(|r| format!("{} → {}", r.path, r.project))
        .collect::<Vec<_>>()
        .join("; ");
    // Один и тот же путь дважды — лишний обход (проекты дублировать можно).
    let mut dup: Vec<String> = Vec::new();
    for (i, r) in roots.iter().enumerate() {
        if roots[..i]
            .iter()
            .any(|x| x.path.eq_ignore_ascii_case(&r.path))
            && !dup.contains(&r.path)
        {
            dup.push(r.path.clone());
        }
    }
    let check = Check::new("roots", "ok", format!("Корни индексации: {list}"));
    if dup.is_empty() {
        check
    } else {
        check.msg(format!("Дубли корней (лишний обход): {}", dup.join(", ")))
    }
}

/// Пункт 3: чат-роль (не критично для поиска).
///
/// M4 §3.1: показываем **фактическое** число слотов (`/props.total_slots`) рядом
/// с настроенным `llm_server.parallel` (с профильным дефолтом, см.
/// `hds_core::config::llm_parallel`); расхождение — warning с числами (резидент
/// поднят со старым конфигом). Занятая/свободная VRAM — в пункте
/// `gpu-observability`.
fn check_chat(cfg: &Config) -> Check {
    let model = dig(cfg, "chat.model")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let configured = hds_core::config::llm_parallel(cfg);
    match probe_role(cfg, "chat", PROBE_TIMEOUT) {
        Probe::Llama(props) => {
            let actual = props
                .get("total_slots")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            if actual > 0 && actual != configured {
                Check::new(
                    "chat",
                    "warn",
                    format!(
                        "Чат-сервер отвечает ('{model}'), но слотов {actual}, а настроено \
                         llm_server.parallel: {configured} — резидент поднят со старым конфигом"
                    ),
                )
                .fix(
                    "Перезапустите владельца портов (`llm_host stop`, затем `llm_host run`) \
                     или поправьте llm_server.parallel в config.yaml.",
                )
            } else {
                Check::new(
                    "chat",
                    "ok",
                    format!("Чат-сервер llama.cpp отвечает ('{model}'), слотов {actual}"),
                )
            }
        }
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

/// Пункт 5: Tesseract OCR — бинарь (конфиг / `PATH` / стандартные каталоги) и
/// языковые пакеты `index.ocr_lang`.
fn check_ocr(cfg: &Config) -> Check {
    let Some(tess) = tesseract_exe(cfg) else {
        return Check::new(
            "ocr",
            "warn",
            "Tesseract OCR не найден — картинки и сканы без текстового слоя не индексируются",
        )
        .fix(
            "Установите Tesseract с языком 'rus' и укажите index.ocr_tesseract_cmd: \
             winget install --scope machine -e --id UB-Mannheim.TesseractOCR",
        );
    };
    let missing = missing_langs(cfg, &tess);
    if missing.is_empty() {
        return Check::new("ocr", "ok", "Tesseract OCR найден").msg(format!(
            "{}; языки {} ({})",
            tess.display(),
            ocr_langs(cfg).join("+"),
            tessdata_dirs(cfg, &tess)
                .first()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "tessdata не найден".to_string()),
        ));
    }
    Check::new(
        "ocr",
        "warn",
        "Tesseract OCR найден, но нет языковых пакетов",
    )
    .msg(format!(
        "нет: {}; искать в: {}",
        missing.join(", "),
        tessdata_dirs(cfg, &tess)
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join("; "),
    ))
    .fix(
        "Положите <lang>.traineddata в каталог tessdata рядом с tesseract(.exe) \
             или укажите index.ocr_tessdata (файлы: github.com/tesseract-ocr/tessdata).",
    )
}

/// Путь к CLI `ffmpeg`: `index.ffmpeg_cmd` → `PATH` → стандартные каталоги.
///
/// Реализация — `hds_core::tools::find_ffmpeg` (W0): тот же поиск использует
/// новый путь диаризации (`hds-diarize::decode` в `llm_host`), который не может
/// зависеть от `hds-index`. Поведение и порядок поиска не изменились.
pub fn ffmpeg_exe(cfg: &Config) -> Option<PathBuf> {
    tools::find_ffmpeg(cfg)
}

/// Каталог движка: `index.whisper_engine_dir` → `HDS_ENGINE_DIR` (machine env,
/// её ставит серверный установщик) → `%APPDATA%\OpenResearchTools\
/// TranscribeOffline\Engine`, если он существует.
pub fn engine_dir(cfg: &Config) -> Option<PathBuf> {
    if let Some(s) = dig(cfg, "index.whisper_engine_dir").and_then(|v| v.as_str()) {
        let p = PathBuf::from(s.trim());
        if !s.trim().is_empty() && p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(env) = std::env::var("HDS_ENGINE_DIR") {
        let p = PathBuf::from(env.trim());
        if !env.trim().is_empty() && p.is_dir() {
            return Some(p);
        }
    }
    let base = directories::BaseDirs::new()?;
    let p = base
        .config_dir()
        .join("OpenResearchTools")
        .join("TranscribeOffline")
        .join("Engine");
    p.is_dir().then_some(p)
}

/// Вердикт по ffmpeg — **чистая** функция (тестируется без ФС).
///
/// В Rust-конвейере CLI `ffmpeg` не вызывается вовсе: медиа не индексируется, а
/// ASR идёт через `llm-host` (`/internal/transcribe`), где декодирование делает
/// движок своими библиотеками (`Engine\vendor\ffmpeg`). Поэтому при живом
/// движке отсутствие CLI — не предупреждение, а справка (инцидент 15.10.2026:
/// сервер показывал warn про ffmpeg, хотя транскрибация работает).
pub fn ffmpeg_verdict(cli: Option<&Path>, engine_ok: bool) -> Check {
    if let Some(p) = cli {
        return Check::new("ffmpeg", "ok", "ffmpeg найден").msg(p.display().to_string());
    }
    if engine_ok {
        return Check::new(
            "ffmpeg",
            "ok",
            "ffmpeg CLI не требуется: медиа декодирует движок",
        )
        .msg(
            "ASR идёт через llm-host (/internal/transcribe); движок несёт свои \
                 библиотеки декодирования (Engine\\vendor\\ffmpeg). CLI нужен только \
                 Python-путям (hds/extract_av.py) и спайкам.",
        );
    }
    Check::new(
        "ffmpeg",
        "warn",
        "ffmpeg не найден — видео без транскрипции",
    )
    .fix(
        "Установите Gyan.FFmpeg машинно (winget install --scope machine -e --id Gyan.FFmpeg) \
         или укажите index.ffmpeg_cmd.",
    )
}

/// Пункт 6: ffmpeg — путь из конфига / `PATH` / стандартных каталогов + наличие
/// движка (см. [`ffmpeg_verdict`]).
fn check_ffmpeg(cfg: &Config) -> Check {
    ffmpeg_verdict(ffmpeg_exe(cfg).as_deref(), engine_dir(cfg).is_some())
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
pub fn check_gpu_observability(cfg: &Config) -> Check {
    // M2 T2: файлы резидента — в каталоге состояния (server — ProgramData /
    // Application Support), не в корне проекта.
    let data_root = hds_core::config::state_dir(cfg).join("data");
    let hb_path = data_root.join("llm-host.heartbeat.json");
    let log_path = data_root.join("logs").join("llm-host.log");
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

/// Backend диаризации, который выберет `llm_host` (`diarization.backend` → авто).
///
/// W5: `hds check`/`hds whisper-check` показывают тот же выбор, что сделает хост
/// (`host::pick_diar_backend`/`resolved_diar_backend`), чтобы расхождение было
/// видно до первого задания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiarBackend {
    /// Nemotron-3 (Sortformer v3, 8 спикеров) через `hds-diarize`.
    Nemotron3,
    /// Прежний путь движка (sortformer GGUF) — рабочий fallback.
    Sortformer,
    /// Диаризация выключена (как `diarization: false`).
    Off,
}

impl DiarBackend {
    /// Строка как в конфиге (`nemotron3|sortformer|off`).
    pub fn as_str(&self) -> &'static str {
        match self {
            DiarBackend::Nemotron3 => "nemotron3",
            DiarBackend::Sortformer => "sortformer",
            DiarBackend::Off => "off",
        }
    }
}

/// Что означает `device: auto` на этой платформе (`Diarizer::choose_device`).
fn platform_diar_ep() -> &'static str {
    if cfg!(windows) {
        "directml"
    } else if cfg!(target_os = "macos") {
        "coreml"
    } else {
        "cpu"
    }
}

/// Результат проверки модели Nemotron-3 в [`DiarSummary`] (F5, ревью M2).
///
/// Раньше это был `Option<Result<(), String>>`: `None` путало «файла нет» и
/// «не проверяли», а `diarization_summary` хешировала 400 МБ **до** выбора
/// backend — то есть и при `backend: off`/`sortformer`, где модель не нужна.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NemotronCheck {
    /// Файла модели нет (ни явного, ни автопоиска).
    Missing,
    /// Backend не `nemotron3` — модель не проверяли (sha256 не считается).
    NotChecked,
    /// Размер и sha256 сошлись.
    Ok,
    /// Файл есть, но проверка не прошла (причина — размер/sha256/чтение).
    Failed(String),
}

impl NemotronCheck {
    /// Из результата `hds_diarize::verify_model`.
    fn from_result(r: std::result::Result<(), String>) -> NemotronCheck {
        match r {
            Ok(()) => NemotronCheck::Ok,
            Err(e) => NemotronCheck::Failed(e),
        }
    }

    /// Модель проверена фактически (sha256 считался): `Ok`/`Failed`.
    ///
    /// `Missing` (файла нет) и `NotChecked` (backend не nemotron3) — `false`.
    pub fn checked(&self) -> bool {
        matches!(self, NemotronCheck::Ok | NemotronCheck::Failed(_))
    }
}

/// Сводка диаризации для `hds check` и `hds whisper-check` (W5).
///
/// Собирается из конфига и файловой системы; модель проверяется
/// `hds_diarize::verify_model` (размер + sha256, результат кэшируется по
/// `(путь, размер, mtime)` — 400 МБ хешируются не чаще раза за процесс) и
/// **только** когда backend действительно `nemotron3` (F5).
#[derive(Debug, Clone)]
pub struct DiarSummary {
    /// `diarization.backend` как в конфиге (`None` — секции/ключа нет).
    pub configured: Option<String>,
    /// Backend по правилу хоста: конфиг → авто (решения владельца №10/№13).
    pub backend: DiarBackend,
    /// `diarization.nemotron3.device` (по умолчанию `auto`).
    pub device: String,
    /// Что означает `auto` на этой платформе.
    pub device_auto: &'static str,
    /// `diarization.nemotron3.chunk_min`.
    pub chunk_min: f64,
    /// Путь модели Nemotron-3 (явный из конфига или найденный автопоиском).
    pub nemotron_model: Option<PathBuf>,
    /// Проверка модели (см. [`NemotronCheck`]).
    pub nemotron: NemotronCheck,
    /// Sortformer-модель движка (нужна только legacy-ветке).
    pub sortformer_model: Option<PathBuf>,
    /// CLI `ffmpeg` найден: для nemotron3 **обязателен** (им декодируется WAV).
    pub ffmpeg: bool,
    /// Предупреждения разбора конфига (хост их пишет в лог, но не падает).
    pub warnings: Vec<String>,
}

impl DiarSummary {
    /// Модель Nemotron-3 есть и проходит размер+sha256.
    pub fn nemotron_ok(&self) -> bool {
        matches!(self.nemotron, NemotronCheck::Ok)
    }
}

/// Собрать сводку по конфигу (`diarization.*`, `auto_transcribe.diarization_model`).
pub fn diarization_summary(cfg: &Config) -> DiarSummary {
    let mut warnings: Vec<String> = Vec::new();
    let configured = dig(cfg, "diarization.backend")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    // Список строгий, как `hds_llama::diar::Backend::parse`: неизвестное значение
    // не роняет хост — backend выбирается автоматически (предупреждение ниже).
    let parsed = match configured.as_deref() {
        None => None,
        Some(raw) => match raw.to_ascii_lowercase().as_str() {
            "nemotron3" => Some(DiarBackend::Nemotron3),
            "sortformer" => Some(DiarBackend::Sortformer),
            "off" => Some(DiarBackend::Off),
            _ => {
                warnings.push(format!(
                    "diarization.backend '{raw}' неизвестен (ожидается nemotron3|sortformer|off) \
                     — хост выберет backend автоматически"
                ));
                None
            }
        },
    };

    let nemotron_model = crate::autotranscribe::nemotron_model_for(cfg);
    // F5 (M2): сначала выбираем backend, потом (и только если он nemotron3 или
    // режим «авто») проверяем модель. При `off`/`sortformer` 400 МБ не читаются.
    let mut auto_result: Option<std::result::Result<(), String>> = None;
    let auto = match nemotron_model.as_deref() {
        Some(p) if p.is_file() => {
            let r = hds_diarize::verify_model(p).map_err(|e| e.to_string());
            let ok = r.is_ok();
            auto_result = Some(r);
            if ok {
                DiarBackend::Nemotron3
            } else {
                DiarBackend::Sortformer
            }
        }
        _ => DiarBackend::Sortformer,
    };
    let backend = parsed.unwrap_or(auto);
    let verify_now = |p: &Path| {
        NemotronCheck::from_result(hds_diarize::verify_model(p).map_err(|e| e.to_string()))
    };
    let nemotron = if parsed.is_none() {
        // Авто-правило модель уже проверило (или её нет) — это и определило выбор.
        match (&nemotron_model, auto_result) {
            (None, _) => NemotronCheck::Missing,
            (Some(_), Some(r)) => NemotronCheck::from_result(r),
            (Some(_), None) => NemotronCheck::Missing,
        }
    } else {
        match backend {
            DiarBackend::Nemotron3 => match nemotron_model.as_deref() {
                None => NemotronCheck::Missing,
                Some(p) => verify_now(p),
            },
            // off/sortformer: модель Nemotron-3 не нужна — sha256 не считаем.
            _ => NemotronCheck::NotChecked,
        }
    };

    let device = dig(cfg, "diarization.nemotron3.device")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "auto".to_string());
    match hds_diarize::Device::parse(&device) {
        None => warnings.push(format!(
            "diarization.nemotron3.device '{device}' неизвестен (auto|cpu|directml|coreml) — \
             хост возьмёт auto"
        )),
        Some(hds_diarize::Device::DirectMl) if !cfg!(windows) => warnings.push(
            "diarization.nemotron3.device 'directml' доступен только на Windows — \
             прогон упадёт на этом EP"
                .to_string(),
        ),
        Some(hds_diarize::Device::CoreMl) if !cfg!(target_os = "macos") => warnings.push(
            "diarization.nemotron3.device 'coreml' доступен только на macOS — \
             прогон упадёт на этом EP"
                .to_string(),
        ),
        _ => {}
    }
    let chunk_min = dig(cfg, "diarization.nemotron3.chunk_min")
        .and_then(|v| v.as_f64())
        .unwrap_or(10.0);
    if !(2.0..=30.0).contains(&chunk_min) {
        warnings.push(format!(
            "diarization.nemotron3.chunk_min {chunk_min} вне 2..=30 — хост оставит 10"
        ));
    }

    DiarSummary {
        configured,
        backend,
        device,
        device_auto: platform_diar_ep(),
        chunk_min,
        nemotron_model,
        nemotron,
        sortformer_model: crate::autotranscribe::diarization_model_for(cfg),
        ffmpeg: ffmpeg_exe(cfg).is_some(),
        warnings,
    }
}

/// Вердикт пункта `diarization` по готовой сводке (диск не трогает — поэтому
/// тестируется без 400-МБ модели).
///
/// Правила (`DIARIZATION_NEMOTRON_W4_HANDOFF.md` W5):
/// * `backend: off` — `ok` (диаризация выключена осознанно);
/// * `nemotron3` (явно или авто) без модели / с несошедшимся sha256 / без `ffmpeg` —
///   `fail`: задание упадёт, fallback в `speech` нет;
/// * секции нет и сработал legacy-fallback — `warn` с причиной;
/// * `sortformer` явно — прежнее поведение (`warn`, если модели нет).
pub fn diarization_check(s: &DiarSummary) -> Check {
    let notes = s.warnings.join("; ");
    let device = format!("EP {} (auto → {})", s.device, s.device_auto);
    let mut c = match s.backend {
        DiarBackend::Off => Check::new("diarization", "ok", "диаризация выключена (backend=off)"),
        DiarBackend::Sortformer => sortformer_legacy_check(s, &device),
        DiarBackend::Nemotron3 => nemotron3_check(s, &device),
    };
    if !notes.is_empty() {
        c.msg = if c.msg.is_empty() {
            notes
        } else {
            format!("{}; {notes}", c.msg)
        };
    }
    c
}

/// T5 + W5: пункт `diarization` — backend, модель (путь + sha256), EP, `ffmpeg`.
///
/// Без модели автотранскрибация с диаризацией падает: задание `mode: transcript`
/// требует модель и **не** деградирует в `speech` (`PLAN_AUTO_TRANSCRIBE` §0 п.6).
pub fn check_diarization(cfg: &Config) -> Check {
    diarization_check(&diarization_summary(cfg))
}

/// F2 (B2): что **клиент** автотранскрибации попросит у хоста телом
/// `/internal/transcribe`.
///
/// `requested` — [`crate::transcribe::TranscribeConfig::requested_backend`]:
/// `Some` только когда ключ `auto_transcribe.diarization_backend` задан явно.
/// `None` (ключа нет) → `None`: поле не отправляется, backend выберет хост по
/// `diarization.backend` → авто (решения №3/№10).
///
/// Зачем отдельный пункт: раньше клиент всегда посылал `nemotron3`, а `hds check`
/// и `--dry-run` показывали хостовый конфиг (`[ok] sortformer`/`off`), и задание
/// падало уже на хосте — диагностика расходилась с фактом.
pub fn requested_backend_check(cfg: &Config, requested: Option<&str>) -> Option<Check> {
    requested_backend_check_with(
        requested,
        crate::autotranscribe::nemotron_model_for(cfg).as_deref(),
        crate::autotranscribe::diarization_model_for(cfg).as_deref(),
    )
}

/// Та же проверка по заранее найденным моделям (файловую систему не трогает —
/// поэтому тестируется детерминированно, в т.ч. на машине с установленным
/// sortformer).
fn requested_backend_check_with(
    requested: Option<&str>,
    nemotron: Option<&Path>,
    sortformer: Option<&Path>,
) -> Option<Check> {
    let raw = requested?.trim();
    if raw.is_empty() {
        return None;
    }
    let value = raw.to_ascii_lowercase();
    Some(match value.as_str() {
        "off" => Check::new(
            "transcribe_request",
            "ok",
            "конвейер автотранскрибации запросит: off (диаризация выключена в клиенте)",
        ),
        "nemotron3" => match nemotron {
            None => Check::new(
                "transcribe_request",
                "fail",
                "конвейер автотранскрибации запросит: nemotron3 — модель не найдена",
            )
            .msg(format!(
                "{} нет ни в одном каталоге моделей (HDS_MODELS_DIR → \
                 %ProgramData%\\hermes-disk-search\\models → %APPDATA%\\OpenResearchTools\\models); \
                 явный запрос сильнее конфига хоста — задание упадёт (fallback в speech нет)",
                hds_diarize::NEMOTRON_FILE
            ))
            .fix(
                "installers\\fetch_nemotron3_model.ps1, либо \
                 auto_transcribe.diarization_backend: sortformer (или убрать ключ — решит хост)",
            ),
            Some(p) => match hds_diarize::verify_model(p) {
                Ok(()) => Check::new(
                    "transcribe_request",
                    "ok",
                    format!(
                        "конвейер автотранскрибации запросит: nemotron3 ({:.0} МБ, sha256 ok)",
                        size_mb(p)
                    ),
                )
                .msg(format!("модель: {}", p.display())),
                Err(e) => Check::new(
                    "transcribe_request",
                    "fail",
                    format!(
                        "конвейер автотранскрибации запросит: nemotron3 — модель {} не прошла проверку",
                        p.display()
                    ),
                )
                .msg(e.to_string())
                .fix("Перекачайте модель: installers\\fetch_nemotron3_model.ps1 -Force"),
            },
        },
        "sortformer" => match sortformer {
            Some(p) => Check::new(
                "transcribe_request",
                "ok",
                format!(
                    "конвейер автотранскрибации запросит: sortformer (legacy), {:.0} МБ",
                    size_mb(p)
                ),
            )
            .msg(format!("модель: {}", p.display())),
            None => Check::new(
                "transcribe_request",
                "warn",
                "конвейер автотранскрибации запросит: sortformer — .gguf не найден",
            )
            .msg(
                "явный запрос sortformer сильнее конфига хоста: задание `mode: transcript` \
                 без модели отказывает (fallback в speech нет)",
            )
            .fix(
                "installers\\fetch_diarization_model.ps1 (или auto_transcribe.diarization_model), \
                 либо убрать ключ — решит хост",
            ),
        },
        other => Check::new(
            "transcribe_request",
            "warn",
            format!(
                "конвейер автотранскрибации запросит: '{other}' — неизвестный backend"
            ),
        )
        .msg(
            "ожидается nemotron3|sortformer|off; хост запишет предупреждение и выберет \
             backend по своему конфигу",
        ),
    })
}

/// F2: что клиент запросит телом (или `None`, если поля в теле нет).
///
/// Рядом с хостовой строкой это даёт `hds check` полную картину: конфиг хоста —
/// и фактический запрос конвейера.
pub fn requested_backend_of(cfg: &Config) -> Option<String> {
    crate::transcribe::TranscribeConfig::from_auto_transcribe(cfg)
        .requested_backend()
        .map(|s| s.to_string())
}

/// P1: сводка нового python-пути транскрибации (`transcribe.*`).
#[derive(Debug, Clone)]
pub struct TranscribeSummary {
    /// `transcribe.engine`: `python` | `legacy`.
    pub engine: String,
    /// `transcribe.profile`: `cuda|cpu|dml`.
    pub profile: String,
    /// Найденный интерпретатор воркера (`HDS_TRANSCRIBE_PYTHON` → venv профиля).
    pub python: Option<PathBuf>,
    /// Каталог артефактов (`transcript.json`/`diarization.json`).
    pub state_dir: PathBuf,
    /// Строка ASR для отчёта: `e2e_rnnt / torch / cuda / batch 8`.
    pub asr: String,
    /// Строка диаризации: модель, устройство, размер блока.
    pub diarization: String,
    /// Заметки о маппинге старых ключей / неподдержанных режимах.
    pub notices: Vec<String>,
}

/// Собрать сводку `transcribe.*` (не запускает воркер и не грузит модели).
pub fn transcribe_summary(cfg: &Config) -> TranscribeSummary {
    let settings = hds_transcribe::Settings::from_config(cfg);
    let root = hds_core::config::project_root();
    let python = settings.python(&root);
    let asr = format!(
        "{} / {} / {} / batch {}",
        settings.asr.variant, settings.asr.backend, settings.asr.device, settings.asr.batch_size
    );
    let diarization = format!(
        "{} / {} / блоки {:.0} мин{}",
        settings.diarization.model,
        settings.diarization.device,
        settings.diarization.block_minutes,
        if settings.diarization.enabled {
            ""
        } else {
            " (выключена)"
        }
    );
    TranscribeSummary {
        engine: settings.engine.clone(),
        profile: settings.profile.clone(),
        python,
        state_dir: settings.state_dir.clone(),
        asr,
        diarization,
        notices: settings.notices.clone(),
    }
}

/// Пункт для legacy-ветки (sortformer GGUF): регресс не ломаем.
fn sortformer_legacy_check(s: &DiarSummary, device: &str) -> Check {
    let legacy = match s.sortformer_model.as_deref() {
        Some(p) => format!("модель: {} ({:.0} МБ)", p.display(), size_mb(p)),
        None => "sortformer-модель не найдена".to_string(),
    };
    let explicit = s
        .configured
        .as_deref()
        .map(|v| v.trim().eq_ignore_ascii_case("sortformer"))
        .unwrap_or(false);
    if explicit {
        return match s.sortformer_model.as_deref() {
            Some(p) => Check::new(
                "diarization",
                "ok",
                format!(
                    "диаризация: sortformer (legacy), {device} — {} ({:.0} МБ)",
                    p.display(),
                    size_mb(p)
                ),
            )
            .msg(format!("{legacy}; {device}")),
            None => Check::new(
                "diarization",
                "warn",
                "модель диаризации (sortformer) не найдена",
            )
            .msg(
                "`hds transcribe-watch` будет падать: задание `mode: transcript` без \
                         модели отказывает (fallback в speech нет)",
            )
            .fix(
                "installers\\fetch_diarization_model.ps1 \
                         (или auto_transcribe.diarization_model)",
            ),
        };
    }
    // Авто: секции нет (или backend неизвестен) и Nemotron-3 недоступен.
    let reason = match (&s.nemotron_model, &s.nemotron) {
        (Some(p), NemotronCheck::Failed(e)) => {
            format!("модель Nemotron-3 {} не прошла проверку: {e}", p.display())
        }
        (Some(p), NemotronCheck::Ok) => format!(
            "модель Nemotron-3 {} в порядке, но backend задан как sortformer",
            p.display()
        ),
        _ => "модель Nemotron-3 не найдена".to_string(),
    };
    Check::new(
        "diarization",
        "warn",
        "диаризация: sortformer (legacy) — сработал автоподбор backend",
    )
    .msg(format!("{reason}; {legacy}; {device}"))
    .fix(
        "W5: `diarization.backend: nemotron3` + installers\\fetch_nemotron3_model.ps1 \
         (8 спикеров вместо 4); sortformer остаётся fallback.",
    )
}

/// Пункт для Nemotron-3: модель обязательна (жёсткий отказ задания) + `ffmpeg`.
fn nemotron3_check(s: &DiarSummary, device: &str) -> Check {
    let Some(path) = s.nemotron_model.as_deref() else {
        return Check::new(
            "diarization",
            "fail",
            "диаризация: модель Nemotron-3 не найдена",
        )
        .msg(format!(
            "backend=nemotron3 ищет {} в каталогах моделей (HDS_MODELS_DIR → \
             %ProgramData%\\hermes-disk-search\\models → %APPDATA%\\OpenResearchTools\\models); \
             задание упадёт, fallback в speech нет",
            hds_diarize::NEMOTRON_FILE
        ))
        .fix("installers\\fetch_nemotron3_model.ps1 (или diarization.nemotron3.model)");
    };
    if let NemotronCheck::Failed(e) = &s.nemotron {
        return Check::new(
            "diarization",
            "fail",
            "диаризация: модель Nemotron-3 не прошла проверку",
        )
        .msg(format!(
            "{e}; ожидалось {} байт, sha256 {}",
            hds_diarize::NEMOTRON_SIZE,
            hds_diarize::NEMOTRON_SHA256
        ))
        .fix("Перекачайте модель: installers\\fetch_nemotron3_model.ps1 -Force");
    }
    // F5: backend nemotron3, но модель не проверяли — «sha256 ok» писать нельзя.
    if !s.nemotron_ok() {
        return Check::new(
            "diarization",
            "warn",
            "диаризация: модель Nemotron-3 не проверялась",
        )
        .msg(format!(
            "{} найдена, но sha256 не считался; хост проверит её при первом задании.",
            path.display()
        ))
        .fix("Запустите `hds check` ещё раз или `installers\\fetch_nemotron3_model.ps1 -Force`");
    }
    let source = match s.configured.as_deref() {
        Some(v) if v.trim().eq_ignore_ascii_case("nemotron3") => "конфиг".to_string(),
        Some(v) => format!("авто, diarization.backend '{v}'"),
        None => "авто: секции diarization нет".to_string(),
    };
    // `hds check` печатает у `ok` только заголовок — путь/sha256/EP/ffmpeg должны
    // быть видны сразу (полные данные остаются и в `msg` для `/api/diagnostics`).
    let flag = if s.ffmpeg {
        "ffmpeg найден"
    } else {
        "ffmpeg НЕ найден"
    };
    let title = format!(
        "диаризация: nemotron3 (Sortformer v3), {device} — {} ({:.0} МБ, sha256 ok), {flag}",
        path.display(),
        size_mb(path)
    );
    let msg = format!(
        "модель: {} ({:.0} МБ, sha256 ok); backend: {source}; {device}; chunk_min {}",
        path.display(),
        size_mb(path),
        s.chunk_min
    );
    if s.ffmpeg {
        return Check::new("diarization", "ok", title).msg(msg);
    }
    Check::new("diarization", "fail", title)
        .msg(format!(
            "{msg}; ffmpeg не найден — WAV для диаризации декодировать нечем"
        ))
        .fix(
            "Установите Gyan.FFmpeg машинно (winget install --scope machine -e --id Gyan.FFmpeg) \
             или укажите index.ffmpeg_cmd.",
        )
}

/// Размер файла в МБ (0, если не читается).
fn size_mb(p: &Path) -> f64 {
    std::fs::metadata(p)
        .map(|m| m.len() as f64 / 1_048_576.0)
        .unwrap_or(0.0)
}

/// M2 T1 (`PLAN_M2_SERVER_PROFILE.md` §3.1): профиль установки + рекомендации
/// для `server`. Авторизация определяется **только** `access.enabled` — профиль
/// без неё не включает auth, лишь предупреждает (решение заказчика 07.10.2026).
fn check_profile(cfg: &Config) -> Check {
    let p = hds_core::config::profile(cfg);
    if !p.is_server() {
        return Check::new("profile", "ok", "профиль установки: desktop").msg(format!(
            "дефолт: состояние рядом с программой, автозапуск AtLogOn — поведение без изменений; \
             каталог состояния: {}",
            hds_core::config::state_dir(cfg).display()
        ));
    }
    let enabled = hds_core::config::dig(cfg, "access.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_ui = hds_core::config::dig(cfg, "ui")
        .and_then(|v| v.as_mapping())
        .map(|m| !m.is_empty())
        .unwrap_or(false);
    let mut msgs = vec![format!(
        "серверный режим: системный каталог состояния ({}), службы без интерактивного входа, headless-UI",
        hds_core::config::state_dir(cfg).display()
    )];
    let mut fixes: Vec<String> = Vec::new();
    let mut status = "ok";
    if !enabled {
        status = "warn";
        msgs.push(
            "access.enabled выключен: MCP/UI открыты без токенов — для сервера включите авторизацию"
                .to_string(),
        );
        fixes.push(
            "access: { enabled: true } + `hds user add --admin` (README_SERVER_MULTIUSER.md)"
                .into(),
        );
    }
    if !has_ui {
        status = "warn";
        msgs.push(
            "секция ui (host/port) не задана: hds ui слушает 127.0.0.1:8765 по умолчанию"
                .to_string(),
        );
        fixes.push(
            "ui: { host: \"127.0.0.1\", port: 8765 } — наружу публикует reverse-proxy".into(),
        );
    }
    let mut c = Check::new("profile", status, "профиль установки: server").msg(msgs.join("; "));
    if !fixes.is_empty() {
        c = c.fix(fixes.join("; "));
    }
    c
}

/// M3 T2: аудит — вкл/выкл (по профилю/ключу), путь и размер JSONL.
fn check_audit(cfg: &Config) -> Check {
    if !hds_core::audit::enabled(cfg) {
        return Check::new(
            "audit",
            "ok",
            "аудит выключен (desktop по умолчанию; включается audit.enabled)",
        );
    }
    let path = hds_core::audit::default_path(cfg);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let max = hds_core::audit::max_mb(cfg);
    Check::new(
        "audit",
        "ok",
        format!(
            "аудит включён: {} ({})",
            path.display(),
            if size >= 1024 * 1024 {
                format!("{:.1} МиБ", size as f64 / 1024.0 / 1024.0)
            } else {
                format!("{} КиБ", size.div_ceil(1024))
            }
        ),
    )
    .msg(format!(
        "JSONL-лог событий доступа; ротация при {max} МБ -> audit.jsonl.1"
    ))
}

/// F8 (T0 M4): «сетевой периметр» — фактическая привязка host (`ui`/`mcp_http`),
/// доверие forwarded-заголовкам и состояние rate-limit. Не-loopback host
/// публикует сервис напрямую (в обход reverse-proxy) — предупреждаем; связка
/// «не-loopback + trusted_proxy» позволяет подделать IP заголовком.
fn check_perimeter(cfg: &Config) -> Check {
    let ui_host = hds_core::config::dig(cfg, "ui.host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1");
    let mcp_host = hds_core::config::dig(cfg, "mcp_http.host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1");
    let trusted = hds_core::net::trusted_proxy(cfg);
    let rl_enabled = hds_core::config::dig(cfg, "rate_limit.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| hds_core::config::profile(cfg).is_server());
    let non_loop = !is_loopback(ui_host) || !is_loopback(mcp_host);
    let mut msgs = vec![format!(
        "ui.host={ui_host}, mcp_http.host={mcp_host}; trusted_proxy={trusted}; rate_limit={}",
        if rl_enabled {
            "включён"
        } else {
            "выключен"
        }
    )];
    let mut fixes: Vec<String> = Vec::new();
    let mut status = "ok";
    if non_loop {
        status = "warn";
        msgs.push(format!(
            "host не loopback (ui={ui_host}, mcp={mcp_host}): сервис опубликован напрямую — наружу \
             должен публиковать reverse-proxy с TLS (installers/proxy/), а приложение слушать 127.0.0.1"
        ));
        fixes.push(
            "ui.host/mcp_http.host: \"127.0.0.1\" (наружу — только через reverse-proxy)".into(),
        );
    }
    if trusted && non_loop {
        status = "warn";
        msgs.push(
            "trusted_proxy: true при не-loopback host: клиент может ПОДДЕЛАТЬ IP заголовком \
             X-Forwarded-For (обход IP-лимитов и подмена в аудите)"
                .into(),
        );
        fixes.push(
            "trusted_proxy включать только когда прокси в приватной сети, а прямой доступ к \
             :8787/:8765 закрыт"
                .into(),
        );
    }
    let mut c = Check::new(
        "perimeter",
        status,
        "сетевой периметр (host/trusted_proxy/rate-limit)",
    )
    .msg(msgs.join("; "));
    if !fixes.is_empty() {
        c = c.fix(fixes.join("; "));
    }
    c
}

/// loopback ли адрес (`127.0.0.1`/`localhost`/`::1`).
fn is_loopback(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    h == "127.0.0.1" || h == "localhost" || h == "::1" || h == "[::1]"
}

/// M4 T3: очередь тяжёлых RAG — как настроена и что показывает фактический
/// статус MCP-процесса (снапшот `<state_dir>/data/mcp-queue.json`, пишется
/// раз в 2 с: глубина, ожидание p50/p95, отказы, «кто кого ждёт»).
///
/// Очередь живёт в процессе `hds mcp-http`, поэтому отсутствие/устаревание
/// снапшота при включённой очереди — это диагноз (MCP не запущен либо поднят
/// старым бинарём), а не «всё хорошо».
fn check_queue(cfg: &Config) -> Check {
    let q = hds_core::queue::QueueConfig::from_config(cfg);
    let snap = if q.enabled {
        hds_core::queue::read_snapshot(&hds_core::queue::snapshot_path(cfg))
    } else {
        None
    };
    queue_check(cfg, &q, snap)
}

/// Ядро `check_queue` без файлового ввода-вывода (снапшот передаётся снаружи) —
/// так статусы проверяются юнит-тестами без песочницы.
fn queue_check(cfg: &Config, q: &hds_core::queue::QueueConfig, snap: Option<Value>) -> Check {
    if !q.enabled {
        return Check::new(
            "queue",
            "ok",
            "очередь тяжёлых RAG выключена (desktop по умолчанию; включается queue.enabled)",
        );
    }
    let path = hds_core::queue::snapshot_path(cfg);
    let mut fixes: Vec<String> = Vec::new();
    if q.wait_timeout_sec >= hds_core::queue::CLIENT_TIMEOUT_SEC {
        fixes.push(format!(
            "queue.wait_timeout_sec < {} (таймаут MCP-клиента): иначе клиент обрывает \
             запрос раньше, чем придёт 503",
            hds_core::queue::CLIENT_TIMEOUT_SEC
        ));
    }
    // F3 (M5 T0.3): рассинхрон слотов чата и лимита исполнения — 5-й
    // пользователь ждал бы при свободном слоте чата.
    let par = hds_core::config::llm_parallel(cfg);
    if par > q.max_inflight as i64 {
        fixes.push(format!(
            "rate_limit.max_inflight ({}) >= llm_server.parallel ({}): лимит исполнения \
             очереди меньше слотов чата — лишние слоты простаивают",
            q.max_inflight, par
        ));
    }
    let configured = format!(
        "очередь включена: политика {}, глубина ожидания ≤ {}, одновременно исполняется ≤ {} \
         (на пользователя ≤ {}), ожидание ≤ {} с",
        q.policy.as_str(),
        q.max_depth,
        q.max_inflight,
        q.max_inflight_per_user,
        q.wait_timeout_sec
    );
    match snap {
        Some(v) => {
            let age = hds_core::queue::snapshot_age_secs(&v).unwrap_or(f64::MAX);
            let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            let users: Vec<String> = v
                .get("per_user")
                .and_then(|p| p.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|u| {
                            Some(format!(
                                "{}: ждёт {}, исполняет {}",
                                u.get("user")?.as_str()?,
                                u.get("waiting")?.as_u64()?,
                                u.get("executing")?.as_u64()?
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let msg = format!(
                "{configured}; сейчас: исполняется {}, ждёт {}, ожидание p50/p95 {} / {} мс, \
                 отказов (очередь полна) {}, отказов (предел пользователя) {}, таймаутов ожидания {}, \
                 обрывов клиента {}{}",
                n("executing"),
                n("depth"),
                n("wait_p50_ms"),
                n("wait_p95_ms"),
                n("rejected_full"),
                n("rejected_user"),
                n("timeouts"),
                n("aborted"),
                if users.is_empty() {
                    String::new()
                } else {
                    format!("; кто ждёт — {}", users.join(", "))
                }
            );
            if age > hds_core::queue::SNAPSHOT_STALE_SEC {
                Check::new(
                    "queue",
                    "warn",
                    format!("очередь включена, но статус устарел ({age:.0} с назад)"),
                )
                .msg(msg)
                .fix(
                    "Проверьте процесс MCP-http (`hds mcp-http status`/`start`) — снапшот \
                     data/mcp-queue.json должен обновляться каждые 2 с; старый бинарь его не пишет.",
                )
            } else if fixes.is_empty() {
                Check::new(
                    "queue",
                    "ok",
                    format!("очередь тяжёлых RAG ({} с)", age.round()),
                )
                .msg(msg)
            } else {
                Check::new("queue", "warn", "очередь тяжёлых RAG: проверьте настройки")
                    .msg(msg)
                    .fix(fixes.join("; "))
            }
        }
        None => Check::new(
            "queue",
            "warn",
            "очередь тяжёлых RAG включена, но её статус не пишется",
        )
        .msg(format!(
            "{configured}; снапшот {} отсутствует",
            path.display()
        ))
        .fix(if fixes.is_empty() {
            "Запустите MCP-http (`hds mcp-http start`) — снапшот очереди пишет только он."
                .to_string()
        } else {
            format!(
                "Запустите MCP-http (`hds mcp-http start`); {}",
                fixes.join("; ")
            )
        }),
    }
}

/// M5 T1 (§3.2): пункт `chat_provider` — источник RAG-ответа: `local` (чат-роль
/// llm-host) или `remote` (внешний API `chat.base_url`). Для remote без
/// `base_url`/`api_key` — warn (сервер не заработает / ключ не задан);
/// напоминание про анонимизацию (W9) — без неё наружу уходят вопрос и фрагменты.
fn check_chat_provider(cfg: &Config) -> Check {
    let provider = hds_core::config::chat_provider(cfg);
    if provider != "remote" {
        return Check::new(
            "chat_provider",
            "ok",
            "чат-роль локальная (llm-host, фасад :8010); provider=remote включается ключом chat.provider",
        );
    }
    let base = hds_core::config::dig(cfg, "chat.base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let key = hds_core::config::chat_api_key(cfg);
    let mut fixes: Vec<String> = Vec::new();
    if base.is_empty() || !base.starts_with("http://") && !base.starts_with("https://") {
        fixes.push("задайте chat.base_url (внешний OpenAI-совместимый API)".to_string());
    }
    if key.is_empty() || key == "-" {
        fixes.push("задайте chat.api_key (внешний API без ключа — обычно 401)".to_string());
    }
    let msg = format!(
        "чат ВНЕШНИЙ ({base}): RAG-ответ генерирует внешний API; chat-роль llm-host не \
         поднимается (VRAM освобождена). Внимание: вопрос и фрагменты локальных файлов \
         уходят провайдеру как есть — анонимизация ещё не реализована (W9)"
    );
    if fixes.is_empty() {
        Check::new("chat_provider", "ok", "внешний chat-provider (remote)").msg(msg)
    } else {
        Check::new(
            "chat_provider",
            "warn",
            "внешний chat-provider (remote): проверьте настройки",
        )
        .msg(msg)
        .fix(fixes.join("; "))
    }
}

/// Разобрать раскладку `gpu.devices` из YAML (`Value` из `config.yaml`).
///
/// Возвращает `(назначения, предупреждения)`: назначения — `{индекс карты → роли}`,
/// предупреждения — кривые записи и неизвестные роли (как в llm-host `config.rs`,
/// но из YAML `hds_core::Config`: `hds-index` не зависит от `hds-llama`).
fn gpu_layout_from_yaml(cfg: &Config) -> (Vec<(i32, Vec<String>)>, Vec<String>) {
    let mut out = Vec::new();
    let mut warns = Vec::new();
    let roles = ["chat", "embedding", "rerank", "whisper"];
    if let Some(list) = dig(cfg, "gpu.devices").and_then(|v| v.as_sequence()) {
        for (i, item) in list.iter().enumerate() {
            let index = item.get("index").and_then(|v| v.as_i64());
            let assigned: Vec<String> = item
                .get("roles")
                .and_then(|v| v.as_sequence())
                .map(|seq| {
                    seq.iter()
                        .filter_map(|r| r.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            match index {
                Some(ix) if ix >= 0 && !assigned.is_empty() => {
                    for r in &assigned {
                        if !roles.contains(&r.as_str()) {
                            warns.push(format!(
                                "gpu.devices[{i}]: неизвестная роль «{r}» — ожидается одна из: {}",
                                roles.join(", ")
                            ));
                        }
                    }
                    out.push((ix as i32, assigned));
                }
                _ => warns.push(format!(
                    "gpu.devices[{i}]: запись пропущена — нужны поля index (число ≥ 0) \
                     и непустой список roles"
                )),
            }
        }
    }
    (out, warns)
}

/// Карта роли из конфига с приоритетами M6 §3.1 (как в `hds-llama::config`):
/// `llm.<role>.device` → `llm_server.<role>.device` → `gpu.devices[].roles`.
/// Возвращает `(индекс, число источников)`.
fn role_device_from_cfg(cfg: &Config, role: &str) -> (Option<i32>, usize) {
    let mut sources = 0usize;
    let mut index: Option<i32> = None;
    // M6-1 (код-ревью M6, 10.10.2026): новый ключ `llm.<role>.device` приоритетнее
    // `llm_server.<role>.device` — зеркально `hds-llama::config::build_role`.
    // Раньше читался только старый ключ: роль по `llm.<role>.device` «исчезала» из
    // пункта `gpu_roles`, а двойное назначение с `gpu.devices` не ловилось.
    let explicit = dig(cfg, &format!("llm.{role}.device"))
        .and_then(|v| v.as_i64())
        .or_else(|| dig(cfg, &format!("llm_server.{role}.device")).and_then(|v| v.as_i64()));
    if let Some(ix) = explicit.filter(|v| *v >= 0) {
        sources += 1;
        index = Some(ix as i32);
    }
    let (layout, _) = gpu_layout_from_yaml(cfg);
    for (n, ix) in layout
        .iter()
        .filter(|(_, rs)| rs.iter().any(|r| r == role))
        .map(|(ix, _)| *ix)
        .enumerate()
    {
        sources += 1;
        if n == 0 && index.is_none() {
            index = Some(ix);
        }
    }
    (index, sources)
}

/// Вендор ускорителя по описанию движка (имя GPU): nvidia | amd | intel | apple | прочее.
fn device_vendor(desc: &str) -> &'static str {
    let d = desc.to_lowercase();
    if d.contains("nvidia") || d.contains("geforce") {
        "nvidia"
    } else if d.contains("amd") || d.contains("radeon") {
        "amd"
    } else if d.contains("intel") {
        "intel"
    } else if d.contains("apple") {
        "apple"
    } else {
        "unknown"
    }
}

/// Похоже ли устройство на iGPU/UMA по описанию (эвристика по именам вендоров:
/// `AMD Radeon(TM) Graphics`, `Intel(R) UHD/Iris Graphics` — интегрированные).
fn looks_like_igpu(desc: &str) -> bool {
    let d = desc.to_lowercase();
    d.contains("(tm) graphics") || d.contains("uhd graphics") || d.contains("iris")
}

/// Живые устройства фасада (`GET :8010/internal/devices`) — `None`, если резидент
/// не ответил (пункт `gpu_roles` честно сообщает об этом).
fn fetch_devices_json(cfg: &Config) -> Option<Value> {
    let (host, port) = role_addr(cfg, "chat");
    http::request(
        &host,
        port,
        "GET",
        "/internal/devices",
        &[],
        None,
        PROBE_TIMEOUT,
    )
    .ok()
    .filter(|r| (200..300).contains(&r.status))
    .and_then(|r| r.json().ok())
}

/// Ускорители из JSON устройств (`backend != CPU`).
fn accelerators_of(dev_json: &Value) -> Vec<&Value> {
    dev_json
        .get("devices")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|d| {
                    d.get("backend")
                        .and_then(|v| v.as_str())
                        .map(|b| !b.eq_ignore_ascii_case("CPU"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// M6 T4 §3.4: подсказка о пресете раскладки — сколько ускорителей/бэкендов
/// нашлось и какой пресет уместен. Информация для оператора (в `msg` пункта
/// `gpu_roles`), **статус пункта не меняет**. `accelerators` — живые ускорители
/// (пусто, если резидент недоступен).
fn preset_hint_text(preset: Option<&str>, autodetect: bool, accelerators: &[&Value]) -> String {
    let n = accelerators.len();
    let mut vendors: Vec<&str> = accelerators
        .iter()
        .filter_map(|d| d.get("description").and_then(|v| v.as_str()))
        .map(device_vendor)
        .filter(|v| *v != "unknown")
        .collect();
    vendors.sort_unstable();
    vendors.dedup();
    let family = if n == 0 {
        "неизвестно".to_string()
    } else if vendors.is_empty() {
        "unknown".to_string()
    } else {
        vendors.join(" + ")
    };
    let suggest = if vendors.len() > 1 {
        "нет (смешанные вендоры, §2 — не поддерживаются)".to_string()
    } else {
        match vendors.first().copied() {
            Some("nvidia") => {
                if n >= 2 {
                    "nvidia-n".to_string()
                } else {
                    "nvidia-one".to_string()
                }
            }
            Some("amd") => "amd-n".to_string(),
            Some("apple") => "apple".to_string(),
            _ => "не определён".to_string(),
        }
    };
    let set = match preset {
        Some(p) if !p.is_empty() => format!("задан gpu.preset={p}"),
        _ => "не задан".to_string(),
    };
    let auto = if autodetect {
        "gpu.autodetect=true"
    } else {
        "gpu.autodetect=false (по умолчанию)"
    };
    let counted = if n == 0 {
        "ускорителей в движке не видно (резидент не ответил — смотрите `llm-host devices`)"
            .to_string()
    } else {
        format!("ускорителей в движке {n} ({family})")
    };
    format!(
        "пресет раскладки: {set}; {counted}; уместен gpu.preset={suggest}; {auto} — раскладка \
         только по явному ключу, `gpu.devices` сильнее пресета (§3.4)"
    )
}

/// M6 T1 (`PLAN_M6_GPU_MULTIVENDOR.md` §3.1/§3.5/§5): пункт `gpu_roles` —
/// раскладка ролей по картам и границы поддержки.
///
/// Проверяет по конфигу: «одна роль — ровно одна карта» (двойное назначение —
/// warn с числами), наличие карт. По живым устройствам фасада
/// (`GET /internal/devices`): backend/свободная VRAM карты каждой роли, warn на
/// недоступную карту, смешанные вендоры (§2: не поддерживаются) и iGPU/UMA.
fn check_gpu_roles(cfg: &Config) -> Check {
    let (_, mut warns) = gpu_layout_from_yaml(cfg);
    // «Одна роль — ровно одна карта» + карты явно не на CPU, если их нет.
    let mut role_lines: Vec<String> = Vec::new();
    for role in ["chat", "embedding", "rerank", "whisper"] {
        let (ix, sources) = role_device_from_cfg(cfg, role);
        match (ix, sources) {
            (Some(ix), 1) => role_lines.push(format!("{role} → карта {ix}")),
            (Some(_), n) if n > 1 => warns.push(format!(
                "{role}: роль назначена более чем на одну карту ({n} источника: \
                 llm_server.{role}.device и/или gpu.devices) — одна роль работает \
                 ровно на одной карте (§3.1)"
            )),
            _ => {}
        }
    }

    // M6 T4 §3.4: пресет/автодетект раскладки — подсказка в пункте (статус не меняет).
    let preset = dig(cfg, "gpu.preset")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_lowercase());
    let autodetect = dig(cfg, "gpu.autodetect")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let preset_active = preset
        .as_deref()
        .map(|p| !p.is_empty() && p != "none" && p != "off")
        .unwrap_or(false);

    let empty_layout = role_lines.is_empty()
        && !warns
            .iter()
            .any(|w| w.contains("роль назначена") || w.contains("gpu.devices"));
    if empty_layout && warns.is_empty() && !preset_active && !autodetect {
        // Компат: раскладки/пресета нет — все роли на карте device_index.
        let di = dig(cfg, "gpu.device_index")
            .and_then(|v| v.as_i64())
            .unwrap_or(1);
        let live = fetch_devices_json(cfg);
        let acc: Vec<&Value> = live.as_ref().map(accelerators_of).unwrap_or_default();
        let hint = preset_hint_text(preset.as_deref(), autodetect, &acc);
        return Check::new(
            "gpu_roles",
            "ok",
            "раскладка ролей по картам не задана (gpu.devices) — все роли на карте gpu.device_index",
        )
        .msg(format!(
            "gpu.device_index={di} (0 = CPU, 1..N = ускорители движка); \
             раскладка задаётся списком gpu.devices: [{{ index: 1, roles: [chat] }}, …]; {hint}"
        ));
    }

    // Контекст раскладки из конфига (для случая, когда роли не расписаны
    // по картам, но задан пресет — его раскладку разрешает llm-host по числу карт).
    let ctx = if !role_lines.is_empty() {
        role_lines.join("; ")
    } else if preset_active {
        format!(
            "пресет gpu.preset={} — раскладку разрешает llm-host по числу карт",
            preset.as_deref().unwrap_or("")
        )
    } else {
        "раскладка из конфига прочитана".to_string()
    };

    self::check_gpu_roles_live(cfg, role_lines, warns, ctx, preset, autodetect)
}

/// Часть пункта `gpu_roles` с живыми устройствами (отделена для читаемости).
fn check_gpu_roles_live(
    cfg: &Config,
    role_lines: Vec<String>,
    warns: Vec<String>,
    ctx: String,
    preset: Option<String>,
    autodetect: bool,
) -> Check {
    // Живые устройства фасада (`/internal/devices`).
    let Some(dev_json) = fetch_devices_json(cfg) else {
        let (host, port) = role_addr(cfg, "chat");
        let hint = preset_hint_text(preset.as_deref(), autodetect, &[]);
        let mut check = Check::new(
            "gpu_roles",
            "warn",
            "раскладка ролей по картам задана, но устройства резидента недоступны",
        )
        .msg(format!(
            "{ctx}; {hint}; резидент не отвечает на GET {host}:{port}/internal/devices — \
             проверить карты/бэкенды можно `llm-host devices`"
        ));
        for w in &warns {
            check = check.fix(w.clone());
        }
        return check;
    };

    let devices: Vec<Value> = dev_json
        .get("devices")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let accelerators: Vec<&Value> = accelerators_of(&dev_json);

    let mut lines: Vec<String> = Vec::new();
    let mut fixes: Vec<String> = warns;
    // Для каждой роли: карта существует? backend? свободная VRAM?
    for line in role_lines {
        let role = line.split(" → ").next().unwrap_or("").to_string();
        let ix = role_device_from_cfg(cfg, &role).0.unwrap_or(0);
        if ix == 0 {
            if let Some(cpu) = devices
                .iter()
                .find(|d| d.get("backend").and_then(|v| v.as_str()) == Some("CPU"))
            {
                let free = cpu
                    .get("memory_free_mib")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                lines.push(format!("{line} (CPU, свободно {free:.0} МиБ)"));
            } else {
                lines.push(format!("{line} (CPU)"));
            }
            continue;
        }
        match accelerators.get((ix - 1) as usize) {
            Some(acc) => {
                let backend = acc.get("backend").and_then(|v| v.as_str()).unwrap_or("?");
                let desc = acc
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let free = acc
                    .get("memory_free_mib")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                // M6-4 (код-ревью M6): при пустом описании устройства не печатаем
                // «CUDA ,» — склеиваем backend/desc без лишнего пробела.
                let dev_txt = if desc.trim().is_empty() {
                    backend.to_string()
                } else {
                    format!("{backend} {desc}")
                };
                lines.push(format!(
                    "{line} ({dev_txt}, свободно {free:.0} МиБ — замер движка, R29 \
                     недостоверен для бюджета)"
                ));
                if looks_like_igpu(desc) {
                    fixes.push(format!(
                        "карта {ix} ({desc}) похожа на iGPU/UMA — UMA не поддерживается в \
                         проде (§2); назначайте роли на дискретные карты"
                    ));
                }
            }
            None => fixes.push(format!(
                "{line}: карта {ix} не найдена — ускорителей в движке {}; проверьте индексы \
                 `llm-host devices` (с Vulkan-бэкендом порядок ускорителей меняется!)",
                accelerators.len()
            )),
        }
    }

    // §2: смешанные вендоры не поддерживаются.
    let mut vendors: Vec<&str> = accelerators
        .iter()
        .filter_map(|d| d.get("description").and_then(|v| v.as_str()))
        .map(device_vendor)
        .filter(|v| *v != "unknown")
        .collect();
    vendors.sort_unstable();
    vendors.dedup();
    if vendors.len() > 1 {
        fixes.push(format!(
            "смешанные вендоры ускорителей ({}) не поддерживаются (§2): прод — карты \
             одного семейства (NVIDIA или AMD под Windows; либо один Mac/Metal)",
            vendors.join(" + ")
        ));
    }

    let status = if fixes.is_empty() { "ok" } else { "warn" };
    // M6 T4 §3.4: подсказка о пресете — с числами (§5), статус не меняет.
    let hint = preset_hint_text(preset.as_deref(), autodetect, &accelerators);
    let msg = if lines.is_empty() {
        hint
    } else {
        format!("{}; {hint}", lines.join("; "))
    };
    Check::new(
        "gpu_roles",
        status,
        "раскладка ролей по картам (gpu.devices)",
    )
    .msg(msg)
    .fix(fixes.join("; "))
}

/// Все проверки (композиция; сеть/воркер — здесь, чистые части — отдельно).
/// R1.4 (PLAN_TRANSCRIBE_SERVICE): `index.transcribe` — legacy-ключ. ASR вынесена из конвейера
/// индексации: медиа пропускается как `skipped_media`, тексты медиа даёт только автотранскрибация.
pub fn check_index_transcribe_legacy(cfg: &Config) -> Check {
    let on = dig(cfg, "index.transcribe")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if on {
        Check::new(
            "index-transcribe",
            "warn",
            "ключ index.transcribe устарел и не действует",
        )
        .msg(
            "ASR в индексации отключена: медиа пропускается (skipped_media); тексты медиа даёт \
             только автотранскрибация (`hds transcribe-watch`).",
        )
        .fix(
            "уберите `index.transcribe` из config.yaml (или поставьте false); транскрибацию \
             настраивайте в секции `auto_transcribe`",
        )
    } else {
        Check::new(
            "index-transcribe",
            "ok",
            "ASR в индексации отключена (медиа пропускается)",
        )
    }
}

/// R2.4 (PLAN_TRANSCRIBE_SERVICE): пункт `auto-transcribe-dirs` — каталоги `inbox_dir`/`out_dir`
/// абсолютные, существуют, различны, не вложены друг в друга и **исключены** из индексации
/// (неявно через `auto_transcribe.*` или явно в `index.exclude_paths`).
pub fn check_auto_transcribe_dirs(cfg: &Config) -> Check {
    let enabled = dig(cfg, "auto_transcribe.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let inbox = dig(cfg, "auto_transcribe.inbox_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let out = dig(cfg, "auto_transcribe.out_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !enabled && inbox.is_empty() && out.is_empty() {
        return Check::new(
            "auto-transcribe-dirs",
            "ok",
            "автотранскрибация не настроена",
        );
    }
    let mut problems: Vec<String> = Vec::new();
    if inbox.is_empty() {
        problems.push("не задан `auto_transcribe.inbox_dir`".into());
    }
    if out.is_empty() {
        problems.push("не задан `auto_transcribe.out_dir`".into());
    }
    for (name, path) in [("inbox_dir", &inbox), ("out_dir", &out)] {
        if path.is_empty() {
            continue;
        }
        let p = Path::new(path);
        if !p.is_absolute() {
            problems.push(format!("{name} не абсолютный: {path}"));
        } else if !p.is_dir() {
            problems.push(format!("{name} не существует: {path}"));
        }
    }
    if !inbox.is_empty() && !out.is_empty() {
        if inbox == out {
            problems.push("inbox_dir и out_dir совпадают".into());
        } else {
            let ni = walk::normalize_path(&inbox);
            let no = walk::normalize_path(&out);
            if ni.starts_with(&format!("{no}/")) || no.starts_with(&format!("{ni}/")) {
                problems.push("inbox_dir и out_dir вложены друг в друга".into());
            }
        }
    }
    // обе папки исключены из индексации (неявные/явные excludes)
    let (excl, _) = pipeline::index_filter(cfg);
    for (name, path) in [("inbox_dir", &inbox), ("out_dir", &out)] {
        if !path.is_empty()
            && !excl.matches_prefix(Path::new(path))
            && !excl.excludes_path(Path::new(path))
        {
            problems.push(format!("{name} не исключён из индексации ({path})"));
        }
    }
    if problems.is_empty() {
        Check::new(
            "auto-transcribe-dirs",
            "ok",
            "каталоги автотранскрибации корректны и исключены",
        )
        .msg("Транскрипты ищутся только после переноса из `out_dir` в индексируемую папку.")
    } else {
        Check::new("auto-transcribe-dirs", "warn", "каталоги автотранскрибации")
            .msg(problems.join("; "))
            .fix(
                "задайте абсолютные, разные, не вложенные `auto_transcribe.inbox_dir`/`out_dir`; \
                 они неявно исключаются из индексации",
            )
    }
}

/// R3.11 (PLAN_TRANSCRIBE_SERVICE): единственный демон автотранскрибации. `transcribe.lock`
/// с живым PID — норма; залипший файл без процесса — предупреждение (мусор/попытка второго демона).
pub fn check_transcribe_daemon() -> Check {
    let lock = config::project_root().join("transcribe.lock");
    if !lock.exists() {
        return Check::new("transcribe-daemon", "ok", "демон transcribe не запущен");
    }
    let pid = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    match pid {
        Some(p) if crate::watch::pid_alive(p) => {
            Check::new("transcribe-daemon", "ok", "демон transcribe запущен (один)")
                .msg(format!("PID {p} жив (transcribe.lock)"))
        }
        _ => Check::new("transcribe-daemon", "warn", "залипший transcribe.lock")
            .msg("файл transcribe.lock без живого процесса")
            .fix("если служба/задача transcribe не запущена — удалите transcribe.lock"),
    }
}

/// Пункт «тень конфига»: действующий `config.yaml` лежит в корне поставки, а в
/// каталоге состояния есть свой — то есть серверный конфиг **не читается**
/// (`config_path()` отдаёт приоритет корню проекта).
///
/// Живой инцидент 15.10.2026 (релиз 0.20.1): в корне поставки остался
/// desktop-образец `config.yaml` (его создал `setup.ps1`), и серверная установка
/// работала в desktop-режиме — UI без токена, чужая БД, heartbeat «не найден».
/// Тихая деградация — обязан быть `warn`.
pub fn check_config_shadow(cfg: &Config) -> Option<Check> {
    let sys_cfg = config::system_state_dir()?.join("config.yaml");
    if !sys_cfg.is_file() {
        return None;
    }
    let effective = config::config_path();
    if same_path(&effective, &sys_cfg) {
        return None; // действует конфиг каталога состояния — «тени» нет
    }
    let root_cfg = config::project_root().join("config.yaml");
    if !same_path(&effective, &root_cfg) {
        return None; // HDS_CONFIG указывает на третий файл — осознанный выбор
    }
    let sys_server = config::load_from(&sys_cfg)
        .ok()
        .map(|c| config::profile(&c).is_server())
        .unwrap_or(false);
    Some(
        Check::new(
            "config-shadow",
            "warn",
            "Корневой config.yaml перекрывает конфиг каталога состояния",
        )
        .msg(format!(
            "действует {} (profile: {}), а {} ({}) не читается",
            effective.display(),
            if config::profile(cfg).is_server() {
                "server"
            } else {
                "desktop"
            },
            sys_cfg.display(),
            if sys_server { "server" } else { "desktop" },
        ))
        .fix(
            "Удалите/переименуйте корневой config.yaml либо закрепите конфиг явно: \
             HDS_CONFIG=<каталог состояния>\\config.yaml (это делает \
             installers\\install_server.ps1 — для служб и для машины).",
        ),
    )
}

/// Пункт «раскладка установки»: из какой копии запускаются **службы** (сервер) и
/// **задания автозапуска** (настольная установка).
///
/// Зачем (инцидент 16.10.2026): и `binPath` служб, и задания Планировщика хранят
/// **абсолютный** путь к программе. Плоская установка (`<корень>\bin\hds.exe`)
/// после `update.cmd` оставалась на прежних файлах: обновление кладёт новую версию в
/// `app\<ver>` и переключает указатель `app\current`, а пути запуска не менялись.
/// Тихая деградация — обязан быть `warn`.
///
/// `None` — ни служб, ни заданий нет (dev-запуск из репозитория): пункт не печатается.
pub fn check_install_layout() -> Option<Check> {
    let mut roles: Vec<RoleBin> = Vec::new();
    for role in hds_core::service::ROLES {
        if let (Some(root), Some(exe)) = (
            hds_core::service::installed_root_of(role),
            hds_core::service::installed_exe(role),
        ) {
            roles.push(RoleBin {
                role: role.to_string(),
                root,
                exe,
            });
        }
    }
    for (task, exe) in hds_core::service::logon_task_exes() {
        if let Some(root) = hds_core::service::install_root_from_binpath(&exe.display().to_string())
        {
            roles.push(RoleBin {
                role: task,
                root,
                exe,
            });
        }
    }
    layout_check(&roles, &|p| p.is_file())
}

/// Бинарь, записанный за ролью службы: роль, корень установки и путь к бинарю.
struct RoleBin {
    role: String,
    root: PathBuf,
    exe: PathBuf,
}

/// Имя бинаря в поставке (`hds.exe` на Windows, `hds` на остальных платформах).
#[cfg(windows)]
const EXE_NAME: &str = "hds.exe";
#[cfg(not(windows))]
const EXE_NAME: &str = "hds";

/// Решение по раскладке — чистая функция: проверка наличия файла передаётся
/// параметром, поэтому тест не трогает ни SCM, ни файловую систему.
///
/// Сравниваются **каталоги** запуска (`<корень>\app\current\bin`), а не имена файлов:
/// имя зависит от платформы (`hds` / `hds.exe`), а смысл пункта — из какой копии
/// работают роли (красный CI 16.10.2026 на ubuntu: сравнение с `EXE_NAME` давало
/// ложное предупреждение).
fn layout_check(bins: &[RoleBin], exists: &dyn Fn(&Path) -> bool) -> Option<Check> {
    let first = bins.first()?;
    let pointer = first.root.join("app").join("current");
    let expected_dir = pointer.join("bin");
    let expected = expected_dir.join(EXE_NAME);
    let versioned_ready = exists(&expected) || exists(&pointer);
    let foreign: Vec<&RoleBin> = bins
        .iter()
        .filter(|b| match b.exe.parent() {
            Some(dir) => !same_path(dir, &expected_dir),
            None => true,
        })
        .collect();
    if versioned_ready && !foreign.is_empty() {
        let list = foreign
            .iter()
            .map(|b| format!("{} -> {}", b.role, b.exe.display()))
            .collect::<Vec<_>>()
            .join(", ");
        return Some(
            Check::new(
                "layout",
                "warn",
                "Раскладка по версиям есть, но службы/задания запускаются не из неё",
            )
            .msg(format!(
                "ожидается {} (указатель app\\current), а записано: {}",
                expected.display(),
                list
            ))
            .fix(
                "Перерегистрируйте роли на активную версию: \
                 `<app\\current>\\bin\\hds.exe` service install <роль> для каждой (сервер) и \
                 повторный запуск `install-server.cmd` из каталога app\\current; задания \
                 автозапуска и ярлык на рабочем столе пересоздайте из app\\current. \
                 Обновление 0.20.3 и новее делает это само (update.cmd).",
            ),
        );
    }
    if versioned_ready {
        return Some(
            Check::new(
                "layout",
                "ok",
                "Раскладка по версиям: запуск из app\\current\\bin",
            )
            .msg(format!(
                "активная версия: {} (обновление и откат подменяют указатель app\\current)",
                expected.display()
            )),
        );
    }
    Some(
        Check::new("layout", "ok", "Простая раскладка: версии ещё не выделены")
            .msg(format!(
                "запуск из {}; каталога app\\current пока нет",
                first.exe.display()
            ))
            .fix(
                "Рабочее состояние: первое обновление создаст app\\<версия>, переведёт на него \
                 службы/задания и оставит прежнюю версию рядом (0.20.3 и новее). Чтобы держать \
                 версии сразу — установка сервера с корнем: `install-server.cmd -Root <машинная папка>`.",
            ),
    )
}

/// Один и тот же файл (по каноническому пути, иначе — по строке).
fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// P1: `transcribe.engine` — python (sidecar) или legacy (llm_host).
///
/// Для python проверяем наличие интерпретатора воркера (env → venv профиля →
/// sidecar/python → `.venv`): без него задания падают ещё до модели.
fn check_transcribe(cfg: &Config) -> Check {
    let s = transcribe_summary(cfg);
    if s.engine != "python" {
        return Check::new(
            "transcribe",
            "ok",
            format!("транскрибация: legacy, профиль {}", s.profile),
        )
        .msg(format!("ASR: {}; диаризация: {}", s.asr, s.diarization));
    }
    let title = format!("транскрибация: python, профиль {}", s.profile);
    match &s.python {
        Some(python) => Check::new("transcribe", "ok", title).msg(format!(
            "интерпретатор: {}; ASR: {}; диаризация: {}",
            python.display(),
            s.asr,
            s.diarization
        )),
        None => Check::new("transcribe", "fail", title)
            .msg(
                "интерпретатор воркера не найден (HDS_TRANSCRIBE_PYTHON / sidecar/venv-transcribe)",
            )
            .fix(
                "Соберите venv профиля (sidecar\\hds_transcribe\\requirements*.lock) \
                 или задайте HDS_TRANSCRIBE_PYTHON.",
            ),
    }
}

pub fn run_checks(cfg: &Config) -> Vec<Check> {
    let mut checks = vec![
        check_profile(cfg),
        check_db(cfg),
        check_roots(cfg),
        check_chat(cfg),
    ];
    checks.push(check_diarization(cfg));
    // F2 (B2): фактический запрос конвейера — рядом с хостовым выбором. Строки
    // нет, если клиентского ключа нет (backend решит хост).
    if let Some(c) = requested_backend_check(cfg, requested_backend_of(cfg).as_deref()) {
        checks.push(c);
    }
    checks.push(check_index_transcribe_legacy(cfg));
    checks.push(check_auto_transcribe_dirs(cfg));
    checks.push(check_transcribe_daemon());
    // P1: новый путь транскрибации (python-sidecar) — движок/профиль/интерпретатор.
    checks.push(check_transcribe(cfg));

    // embedding-роль пробуем один раз — переиспользуем для контекста (как Python)
    let emb_probe = probe_role(cfg, "embedding", PROBE_TIMEOUT);
    checks.push(emb_check(cfg, &emb_probe));
    if let Some(c) = emb_ctx_check(&emb_probe) {
        checks.push(c);
    }

    checks.push(check_ocr(cfg));
    checks.push(check_ffmpeg(cfg));
    if let Some(c) = check_config_shadow(cfg) {
        checks.push(c);
    }
    if let Some(c) = check_install_layout() {
        checks.push(c);
    }
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

    checks.push(check_gpu_observability(cfg));
    checks.push(check_gpu_roles(cfg));
    checks.push(check_audit(cfg));
    checks.push(check_perimeter(cfg));
    checks.push(check_queue(cfg));
    checks.push(check_chat_provider(cfg));
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

#[cfg(test)]
mod tests {
    use super::{check_perimeter, check_profile, layout_check, RoleBin};
    use serde_yaml::Value;
    use std::path::PathBuf;

    fn cfg(text: &str) -> Value {
        serde_yaml::from_str(text).unwrap()
    }

    /// M4 T3: статусы пункта `queue` — выключено/нет снапшота/свежий/устаревший
    /// и предупреждение про `wait_timeout_sec` ≥ клиентского таймаута.
    #[test]
    fn queue_check_statuses() {
        use super::queue_check;
        use hds_core::queue::{QueueConfig, CLIENT_TIMEOUT_SEC};
        use serde_json::json;

        let snapshot = |ts: f64| {
            json!({
                "ts": ts,
                "enabled": true,
                "policy": "rr",
                "max_inflight": 4,
                "max_depth": 20,
                "wait_timeout_sec": 240,
                "depth": 2,
                "executing": 1,
                "granted": 7,
                "rejected_full": 1,
                "timeouts": 2,
                "aborted": 0,
                "total_wait_ms": 4200,
                "wait_p50_ms": 1200,
                "wait_p95_ms": 3000,
                "per_user": [{ "user": "user:anna", "waiting": 2, "executing": 1 }],
            })
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();

        // desktop (по умолчанию) — очередь выключена, пункт ok.
        let c = cfg("index:\n  roots: []");
        let q = QueueConfig::from_config(&c);
        let chk = queue_check(&c, &q, None);
        assert_eq!(chk.status, "ok");
        assert!(chk.title.contains("выключена"), "title={}", chk.title);

        // server без снапшота — warn: статус никто не пишет (MCP не запущен).
        let c = cfg("profile: server");
        let q = QueueConfig::from_config(&c);
        let chk = queue_check(&c, &q, None);
        assert_eq!(chk.status, "warn", "msg={}", chk.msg);
        assert!(chk.msg.contains("отсутствует"), "msg={}", chk.msg);
        assert!(chk.fix.contains("mcp-http"), "fix={}", chk.fix);

        // server + свежий снапшот — ok с числами и «кто кого ждёт».
        let chk = queue_check(&c, &q, Some(snapshot(now)));
        assert_eq!(chk.status, "ok", "msg={}", chk.msg);
        assert!(chk.msg.contains("исполняется 1"), "msg={}", chk.msg);
        assert!(chk.msg.contains("ждёт 2"), "msg={}", chk.msg);
        assert!(chk.msg.contains("1200 / 3000"), "msg={}", chk.msg);
        assert!(chk.msg.contains("user:anna: ждёт 2"), "msg={}", chk.msg);

        // server + устаревший снапшот — warn с подсказкой про MCP-процесс.
        let chk = queue_check(&c, &q, Some(snapshot(now - 120.0)));
        assert_eq!(chk.status, "warn");
        assert!(chk.title.contains("устарел"), "title={}", chk.title);
        assert!(chk.fix.contains("mcp-http"), "fix={}", chk.fix);

        // server + ожидание ≥ клиентского таймаута — warn с числами.
        let c2 = cfg("profile: server\nqueue:\n  wait_timeout_sec: 300");
        let q2 = QueueConfig::from_config(&c2);
        assert!(q2.wait_timeout_sec >= CLIENT_TIMEOUT_SEC);
        let chk = queue_check(&c2, &q2, Some(snapshot(now)));
        assert_eq!(chk.status, "warn");
        assert!(
            chk.fix.contains(&CLIENT_TIMEOUT_SEC.to_string()),
            "fix={}",
            chk.fix
        );
    }

    /// F3 (M5 T0.3): рассинхрон `llm_server.parallel` ↔ `rate_limit.max_inflight`
    /// помечается warn с числами; согласованные дефолты — не помечаются.
    #[test]
    fn queue_check_warns_on_inflight_parallel_mismatch() {
        use super::queue_check;
        use hds_core::queue::QueueConfig;

        // Явный рассинхрон: слотов чата 5, исполнение 3 → warn с числами.
        let c = cfg("profile: server\nllm_server:\n  parallel: 5\n\
             rate_limit:\n  max_inflight: 3\nqueue:\n  enabled: true");
        let q = QueueConfig::from_config(&c);
        let chk = queue_check(&c, &q, None);
        assert_eq!(chk.status, "warn", "msg={}", chk.msg);
        assert!(
            chk.fix.contains("llm_server.parallel (5)"),
            "fix={}",
            chk.fix
        );
        assert!(chk.fix.contains("max_inflight (3)"), "fix={}", chk.fix);

        // Согласованные дефолты server (parallel 5 / inflight 5) — warn только
        // про отсутствие снапшота, не про рассинхрон.
        let c = cfg("profile: server");
        let q = QueueConfig::from_config(&c);
        assert_eq!(q.max_inflight, 5, "дефолт server — как parallel (F3)");
        let chk = queue_check(&c, &q, None);
        assert!(!chk.fix.contains("max_inflight (3)"), "fix={}", chk.fix);
    }

    /// M5 T1 (§3.2): пункт `chat_provider` — local по умолчанию; remote без
    /// base_url/api_key — warn с подсказками; полный remote — ok.
    #[test]
    fn chat_provider_check_statuses() {
        use super::check_chat_provider;

        // Дефолт (нет ключа provider) — local, ok.
        let c = check_chat_provider(&cfg("index:\n  roots: []"));
        assert_eq!(c.status, "ok");
        assert!(c.title.contains("локальная"), "title={}", c.title);

        // remote без base_url и api_key — warn с двумя подсказками.
        let c = check_chat_provider(&cfg("chat:\n  provider: remote"));
        assert_eq!(c.status, "warn", "msg={}", c.msg);
        assert!(c.fix.contains("chat.base_url"), "fix={}", c.fix);
        assert!(c.fix.contains("chat.api_key"), "fix={}", c.fix);

        // Полный remote (https + ключ) — ok, с напоминанием об анонимизации.
        let c = check_chat_provider(&cfg(
            "chat:\n  provider: remote\n  base_url: \"https://api.example.com/v1\"\n  api_key: \"sk-test\"",
        ));
        assert_eq!(c.status, "ok", "msg={}", c.msg);
        assert!(
            c.msg.contains("W9"),
            "напоминание про анонимизацию: {}",
            c.msg
        );
        assert!(
            !c.msg.contains("sk-test"),
            "секрет не печатается: {}",
            c.msg
        );
    }

    /// M6 T1 (§3.1/§3.5): пункт `gpu_roles` — без раскладки ok (компат);
    /// двойное назначение роли и кривые записи — warn (fix содержит числа).
    /// HTTP-ветка (живые устройства) в юнит-тестах не проверяется — правила
    /// «одна роль — одна карта» дублируются fail-fast'ом реестра (registry_plan.rs).
    #[test]
    fn gpu_roles_check_statuses() {
        use super::check_gpu_roles;

        // Раскладки нет — ok, компат device_index.
        let c = check_gpu_roles(&cfg("index:\n  roots: []"));
        assert_eq!(c.status, "ok", "msg={}", c.msg);
        assert!(c.title.contains("не задана"), "title={}", c.title);
        assert!(c.msg.contains("gpu.device_index=1"), "msg={}", c.msg);
        // M6 T4 §3.4: подсказка о пресете — в msg, статус не меняет.
        assert!(c.msg.contains("пресет раскладки"), "msg={}", c.msg);

        // Двойное назначение embedding (devices + role-ключ) — warn с числами.
        let c = check_gpu_roles(&cfg(
            "gpu:\n  devices:\n    - { index: 1, roles: [chat, embedding] }\n\
             llm_server:\n  embedding:\n    device: 2",
        ));
        assert_eq!(c.status, "warn", "msg={}", c.msg);
        assert!(c.fix.contains("более чем на одну карту"), "fix={}", c.fix);
        assert!(c.fix.contains("embedding"), "fix={}", c.fix);

        // Кривая запись gpu.devices — warn с номером записи, молча не пропускается.
        let c = check_gpu_roles(&cfg("gpu:\n  devices:\n    - {}"));
        assert_eq!(c.status, "warn", "msg={}", c.msg);
        assert!(c.fix.contains("gpu.devices[0]"), "fix={}", c.fix);
    }

    /// M6-1 (код-ревью M6, 10.10.2026): карта роли читается и из нового ключа
    /// `llm.<role>.device` (приоритетнее `llm_server.<role>.device`) — зеркально
    /// рантайму `hds-llama::config::build_role`. До фикса роль «исчезала» из
    /// `gpu_roles`, а двойное назначение с `gpu.devices` не ловилось.
    #[test]
    fn role_device_prefers_llm_key() {
        use super::role_device_from_cfg;

        // Новый ключ `llm.<role>.device`.
        let (ix, n) = role_device_from_cfg(&cfg("llm:\n  chat:\n    device: 2"), "chat");
        assert_eq!(ix, Some(2), "карта из llm.chat.device");
        assert_eq!(n, 1);

        // Старый ключ `llm_server.<role>.device` — фолбэк сохранён.
        let (ix, n) = role_device_from_cfg(&cfg("llm_server:\n  chat:\n    device: 3"), "chat");
        assert_eq!(ix, Some(3), "фолбэк на llm_server.chat.device");
        assert_eq!(n, 1);

        // Новый ключ + запись gpu.devices → два источника (двойное назначение).
        let (ix, n) = role_device_from_cfg(
            &cfg(
                "llm:\n  chat:\n    device: 2\ngpu:\n  devices:\n    - { index: 1, roles: [chat] }",
            ),
            "chat",
        );
        assert_eq!(ix, Some(2), "role-ключ сильнее раскладки");
        assert_eq!(n, 2, "двойное назначение видно: {n} источников");

        // Отрицательный индекс игнорируется (как в рантайме: filter(>=0)).
        let (ix, n) = role_device_from_cfg(&cfg("llm:\n  chat:\n    device: -1"), "chat");
        assert_eq!(ix, None);
        assert_eq!(n, 0);
    }

    /// M6 T4 §3.4: подсказка о пресете — подбор по вендорам/числу карт,
    /// смешанные вендоры → «пресеты недоступны»; статус пункта не меняется.
    #[test]
    fn preset_hint_suggests_by_vendor() {
        use super::preset_hint_text;
        use serde_json::json;

        let dev = |desc: &str| json!({ "backend": "CUDA", "description": desc });
        let one = [dev("NVIDIA GeForce RTX 3060")];
        let two = [
            dev("NVIDIA GeForce RTX 3060"),
            dev("NVIDIA GeForce RTX 4090"),
        ];
        let amd = [dev("AMD Radeon RX 7900 XTX")];
        let mixed = [
            dev("NVIDIA GeForce RTX 3060"),
            dev("AMD Radeon RX 7900 XTX"),
        ];
        fn refs_fn(v: &[serde_json::Value]) -> Vec<&serde_json::Value> {
            v.iter().collect()
        }

        assert!(
            preset_hint_text(None, false, &refs_fn(&one)).contains("gpu.preset=nvidia-one"),
            "одна NVIDIA → nvidia-one"
        );
        assert!(
            preset_hint_text(None, false, &refs_fn(&two)).contains("gpu.preset=nvidia-n"),
            "две NVIDIA → nvidia-n"
        );
        assert!(
            preset_hint_text(None, false, &refs_fn(&amd)).contains("gpu.preset=amd-n"),
            "AMD → amd-n"
        );
        assert!(
            preset_hint_text(None, false, &refs_fn(&mixed)).contains("смешанные вендоры"),
            "микс → пресеты недоступны"
        );
        assert!(
            preset_hint_text(Some("amd-n"), false, &refs_fn(&one))
                .contains("задан gpu.preset=amd-n"),
            "явно заданный пресет виден в подсказке"
        );
        assert!(
            preset_hint_text(None, false, &[]).contains("не видно"),
            "без живых устройств — честное «не видно»"
        );
        let auto = preset_hint_text(None, true, &refs_fn(&one));
        assert!(auto.contains("autodetect=true"), "autodetect виден: {auto}");
        assert!(
            auto.contains("уместен gpu.preset=nvidia-one"),
            "автодетект подсказывает пресет: {auto}"
        );
    }

    /// M2 T1: desktop — ok; server без access.enabled/ui — warn; полный server — ok.
    #[test]
    fn profile_check_statuses() {
        // Нет ключа profile → desktop, ok.
        let c = check_profile(&cfg("index:\n  roots: []"));
        assert_eq!(c.status, "ok");
        assert!(c.title.contains("desktop"), "title={}", c.title);

        // server без access.enabled и без ui → warn (два предупреждения).
        let c = check_profile(&cfg("profile: server"));
        assert_eq!(c.status, "warn", "msg={}", c.msg);
        assert!(c.msg.contains("access.enabled"), "msg={}", c.msg);
        assert!(c.msg.contains("ui"), "msg={}", c.msg);
        assert!(!c.fix.is_empty());

        // server + access.enabled, ui нет → warn (только про ui).
        let c = check_profile(&cfg("profile: server\naccess:\n  enabled: true"));
        assert_eq!(c.status, "warn");
        assert!(c.msg.contains("ui"), "msg={}", c.msg);
        assert!(!c.msg.contains("access.enabled выключен"), "msg={}", c.msg);

        // server + access.enabled + ui → ok.
        let c = check_profile(&cfg(
            "profile: server\naccess:\n  enabled: true\nui:\n  host: 127.0.0.1\n  port: 8765",
        ));
        assert_eq!(c.status, "ok");
    }

    /// F8 (T0 M4): периметр — loopback → ok; не-loopback host → warn;
    /// не-loopback + trusted_proxy → предупреждение о подделке IP.
    #[test]
    fn perimeter_check_statuses() {
        // Дефолт (loopback) — ok, состояние видно в msg.
        let c = check_perimeter(&cfg("index:\n  roots: []"));
        assert_eq!(c.status, "ok", "msg={}", c.msg);
        assert!(c.msg.contains("trusted_proxy=false"), "msg={}", c.msg);
        assert!(c.msg.contains("rate_limit=выключен"), "msg={}", c.msg);

        // Не-loopback host — warn с подсказкой.
        let c = check_perimeter(&cfg("ui:\n  host: 0.0.0.0\n"));
        assert_eq!(c.status, "warn", "msg={}", c.msg);
        assert!(c.msg.contains("не loopback"), "msg={}", c.msg);
        assert!(!c.fix.is_empty());

        // Не-loopback + trusted_proxy — warn про подделку IP.
        let c = check_perimeter(&cfg("trusted_proxy: true\nmcp_http:\n  host: 0.0.0.0\n"));
        assert_eq!(c.status, "warn");
        assert!(c.msg.contains("ПОДДЕЛАТЬ"), "msg={}", c.msg);

        // trusted_proxy при loopback — не предупреждаем.
        let c = check_perimeter(&cfg("trusted_proxy: true\nui:\n  host: 127.0.0.1\n"));
        assert_eq!(c.status, "ok", "msg={}", c.msg);
        assert!(c.msg.contains("trusted_proxy=true"), "msg={}", c.msg);

        // server-профиль → rate_limit включён в состоянии.
        let c = check_perimeter(&cfg("profile: server\nui:\n  host: 127.0.0.1\n"));
        assert!(c.msg.contains("rate_limit=включён"), "msg={}", c.msg);
    }
    /// Инцидент 15.10.2026 (релиз 0.20.1): OCR/ffmpeg не должны выглядеть
    /// «не найденными» из-за минимального `PATH` службы — проверяем разбор языков
    /// и вердикт ffmpeg (чистые функции, без ФС и без реального окружения).
    #[test]
    fn ocr_langs_split_and_ffmpeg_verdict() {
        use super::{ffmpeg_verdict, ocr_langs};

        assert_eq!(
            ocr_langs(&cfg("index:\n  ocr_lang: \"rus+eng\"")),
            vec!["rus", "eng"]
        );
        assert_eq!(ocr_langs(&cfg("index:\n  ocr_lang: eng")), vec!["eng"]);
        // Ключа нет — прежний продуктовый дефолт.
        assert_eq!(ocr_langs(&cfg("index:\n  roots: []")), vec!["rus", "eng"]);

        // CLI найден → ok независимо от движка (путь виден в msg).
        let p = std::path::Path::new("C:/tools/ffmpeg.exe");
        let c = ffmpeg_verdict(Some(p), false);
        assert_eq!(c.status, "ok");
        assert!(c.msg.contains("ffmpeg.exe"), "msg={}", c.msg);

        // CLI нет, но движок на месте → не warn, а справка (декодирует движок).
        let c = ffmpeg_verdict(None, true);
        assert_eq!(c.status, "ok", "msg={}", c.msg);
        assert!(c.msg.contains("llm-host"), "msg={}", c.msg);

        // Ни CLI, ни движка → warn с подсказкой про машинную установку.
        let c = ffmpeg_verdict(None, false);
        assert_eq!(c.status, "warn");
        assert!(c.fix.contains("winget"), "fix={}", c.fix);
    }

    /// Tesseract: отсутствующие языковые пакеты ищутся в `index.ocr_tessdata`
    /// (не только рядом с бинарём и не только в пользовательском каталоге).
    #[test]
    fn missing_ocr_langs_reads_tessdata_dir() {
        use super::missing_langs;

        let dir = std::env::temp_dir().join(format!("hds-diag-ocr-{}", std::process::id()));
        let td = dir.join("tessdata");
        std::fs::create_dir_all(&td).unwrap();
        std::fs::write(td.join("rus.traineddata"), b"x").unwrap();
        let tess = dir.join("tesseract.exe");
        std::fs::write(&tess, b"x").unwrap();

        let c = cfg(&format!(
            "index:\n  ocr_lang: \"rus+zzz\"\n  ocr_tessdata: '{}'",
            td.display()
        ));
        assert_eq!(
            missing_langs(&c, &tess),
            vec!["zzz"],
            "rus на месте, zzz нет"
        );

        std::fs::write(td.join("zzz.traineddata"), b"x").unwrap();
        assert!(missing_langs(&c, &tess).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 0.20.3: пункт `layout` — раскладка, из которой запускаются роли служб.
    ///
    /// Живой инцидент 16.10.2026: плоская установка (`<root>\bin\hds.exe`) осталась
    /// зарегистрированной на `bin\`, а обновление подменяло только `app\<ver>` за
    /// указателем `app\current` — роли продолжали работать прежними файлами, и откат
    /// указателя ни на что не влиял. Пункт обязан это показывать.
    #[test]
    fn layout_check_flags_flat_roles_next_to_a_versioned_tree() {
        let root = PathBuf::from(r"C:\hds");
        let flat = vec![RoleBin {
            role: "ui".into(),
            root: root.clone(),
            exe: root.join("bin").join("hds.exe"),
        }];

        // Версии ещё нет: простая раскладка — рабочее состояние, не warn.
        let before = layout_check(&flat, &|_| false).expect("службы есть — пункт печатается");
        assert_eq!(before.status, "ok", "{}", before.msg);
        assert!(before.title.contains("Простая"), "{}", before.title);

        // Версия есть, а роль работает из плоского bin\ — сам инцидент.
        let bad = layout_check(&flat, &|_| true).expect("службы есть");
        assert_eq!(bad.status, "warn", "{}", bad.msg);
        assert!(bad.msg.contains("hds.exe"), "{}", bad.msg);
        assert!(bad.fix.contains("service install"), "{}", bad.fix);

        // Роли переведены на активную версию — снимаем предупреждение.
        let versioned = vec![RoleBin {
            role: "ui".into(),
            root: root.clone(),
            exe: root.join("app").join("current").join("bin").join("hds.exe"),
        }];
        let good = layout_check(&versioned, &|_| true).expect("службы есть");
        assert_eq!(good.status, "ok", "{}", good.msg);
        assert!(good.title.contains(r"app\current"), "{}", good.title);

        // Служб нет (настольная установка) — пункта нет вовсе.
        assert!(layout_check(&[], &|_| true).is_none());
    }

    /// Установщик сервера генерирует минимальный конфиг без `llm_server.<role>.model`.
    /// Диагностика обязана подставлять `shared:<role>` — как рантайм, иначе `hds check`
    /// ищет GGUF в `<state>/<role>` и ложно падает (находка при установке 0.20.5).
    #[test]
    fn resolve_model_defaults_to_shared_role() {
        use super::resolve_model;
        use std::sync::Mutex;

        // `LLAMA_RUNTIME_DIR` — глобальная переменная процесса; сериализуем тест.
        static ENV: Mutex<()> = Mutex::new(());
        let _guard = ENV.lock().unwrap_or_else(|e| e.into_inner());

        let td = std::env::temp_dir().join(format!("hds-diag-shared-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        let runtime = td.join("runtime");
        let emb = runtime.join("models").join("embedding");
        std::fs::create_dir_all(&emb).unwrap();
        let gguf = emb.join("bge-m3-Q8_0.gguf");
        std::fs::write(&gguf, b"x").unwrap();
        std::env::set_var("LLAMA_RUNTIME_DIR", &runtime);

        // Минимальный server-конфиг установщика: роль не задана.
        let c = cfg("profile: server\nllm_server:\n  parallel: 1");
        assert_eq!(resolve_model(&c, "embedding"), gguf);

        // Пустое значение — тоже дефолт, а не путь от корня проекта.
        let c = cfg("llm_server:\n  embedding:\n    model: ''");
        assert_eq!(resolve_model(&c, "embedding"), gguf);

        // Явный путь по-прежнему сильнее дефолта.
        let explicit = td.join("explicit.gguf");
        let c = cfg(&format!(
            "llm_server:\n  embedding:\n    model: '{}'",
            explicit.display()
        ));
        assert_eq!(resolve_model(&c, "embedding"), explicit);

        std::env::remove_var("LLAMA_RUNTIME_DIR");
        let _ = std::fs::remove_dir_all(&td);
    }

    /// W8: `device: auto` в диагностике показывает тот EP, который выберет платформа
    /// (directml — Windows, coreml — macOS, cpu — прочее); macOS вживую не проверен.
    #[test]
    fn platform_auto_ep_matches_platform() {
        let ep = super::platform_diar_ep();
        let want = if cfg!(windows) {
            "directml"
        } else if cfg!(target_os = "macos") {
            "coreml"
        } else {
            "cpu"
        };
        assert_eq!(ep, want);
    }

    /// W5: вердикт пункта `diarization` — off / nemotron3 / legacy-fallback.
    #[test]
    fn diarization_verdict_statuses() {
        use super::{diarization_check, DiarBackend, DiarSummary, NemotronCheck};

        fn summary(configured: Option<&str>, backend: DiarBackend) -> DiarSummary {
            DiarSummary {
                configured: configured.map(|s| s.to_string()),
                backend,
                device: "auto".to_string(),
                device_auto: "cpu",
                chunk_min: 10.0,
                nemotron_model: None,
                nemotron: NemotronCheck::Missing,
                sortformer_model: None,
                ffmpeg: true,
                warnings: Vec::new(),
            }
        }
        let model = PathBuf::from("D:\\models\\nemotron3\\nemotron3_diar_v3.onnx");

        // off — диаризация выключена осознанно.
        let c = diarization_check(&summary(Some("off"), DiarBackend::Off));
        assert_eq!(c.status, "ok");
        assert!(c.title.contains("выключена"), "{}", c.title);

        // nemotron3 без модели — fail: задание упадёт, fallback в speech нет.
        let c = diarization_check(&summary(Some("nemotron3"), DiarBackend::Nemotron3));
        assert_eq!(c.status, "fail");
        assert!(c.fix.contains("fetch_nemotron3_model"), "{}", c.fix);

        // Модель есть, но не прошла размер/sha256 — тоже fail.
        let mut s = summary(Some("nemotron3"), DiarBackend::Nemotron3);
        s.nemotron_model = Some(model.clone());
        s.nemotron = NemotronCheck::Failed("размер модели не тот".to_string());
        let c = diarization_check(&s);
        assert_eq!(c.status, "fail");
        assert!(c.title.contains("не прошла проверку"), "{}", c.title);

        // Модель в порядке — ok; путь/sha256/EP/ffmpeg видны в заголовке
        // (`hds check` печатает у `ok` только его).
        s.nemotron = NemotronCheck::Ok;
        let c = diarization_check(&s);
        assert_eq!(c.status, "ok");
        assert!(c.title.contains("sha256 ok"), "{}", c.title);
        assert!(c.title.contains("ffmpeg найден"), "{}", c.title);
        assert!(c.title.contains("EP auto"), "{}", c.title);
        assert!(c.title.contains("nemotron3_diar_v3.onnx"), "{}", c.title);

        // Без ffmpeg nemotron3 работать не может — fail.
        s.ffmpeg = false;
        let c = diarization_check(&s);
        assert_eq!(c.status, "fail");
        assert!(c.title.contains("ffmpeg"), "{}", c.title);

        // Авто-подбор без Nemotron-3 — warn с причиной (legacy sortformer).
        let mut s = summary(None, DiarBackend::Sortformer);
        s.sortformer_model = Some(PathBuf::from("D:\\models\\sf\\model.gguf"));
        let c = diarization_check(&s);
        assert_eq!(c.status, "warn");
        assert!(c.msg.contains("Nemotron-3 не найдена"), "{}", c.msg);
        assert!(c.fix.contains("nemotron3"), "{}", c.fix);

        // Явный sortformer: с моделью ok (регресс), без модели — warn.
        let mut s = summary(Some("sortformer"), DiarBackend::Sortformer);
        s.sortformer_model = Some(PathBuf::from("D:\\models\\sf\\model.gguf"));
        let c = diarization_check(&s);
        assert_eq!(c.status, "ok");
        assert!(c.title.contains("legacy"), "{}", c.title);
        let c = diarization_check(&summary(Some("sortformer"), DiarBackend::Sortformer));
        assert_eq!(c.status, "warn");

        // Предупреждения разбора конфига не теряются.
        let mut s = summary(Some("nemotron3"), DiarBackend::Nemotron3);
        s.nemotron_model = Some(model);
        s.nemotron = NemotronCheck::Ok;
        s.warnings.push("chunk_min 99 вне 2..=30".to_string());
        assert!(diarization_check(&s).msg.contains("chunk_min 99"));
    }

    /// F5 (ревью M2): при `backend: off`/`sortformer` модель Nemotron-3 **не**
    /// проверяется (400 МБ не читаются) — состояние `NotChecked`; при `nemotron3`
    /// проверка остаётся (файл неверного размера — `Failed`).
    #[test]
    fn off_and_sortformer_do_not_hash_model() {
        use super::{diarization_summary, DiarBackend, NemotronCheck};

        let td = std::env::temp_dir().join(format!("hds-diar-nohash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        std::fs::create_dir_all(&td).unwrap();
        let model = td.join("nemotron3_diar_v3.onnx");
        std::fs::write(&model, b"y").unwrap();
        let model_yaml = model.display().to_string().replace('\\', "/");

        for (value, backend) in [
            ("off", DiarBackend::Off),
            ("sortformer", DiarBackend::Sortformer),
        ] {
            let c = cfg(&format!(
                "diarization:\n  backend: {value}\n  nemotron3:\n    model: '{model_yaml}'\n"
            ));
            let s = diarization_summary(&c);
            assert_eq!(s.backend, backend);
            assert_eq!(
                s.nemotron,
                NemotronCheck::NotChecked,
                "backend {value}: модель не должна проверяться"
            );
            assert!(!s.nemotron.checked() && !s.nemotron_ok());
        }

        // Нужен nemotron3 — проверяем: маленький файл не проходит по размеру.
        let c = cfg(&format!(
            "diarization:\n  backend: nemotron3\n  nemotron3:\n    model: '{model_yaml}'\n"
        ));
        let s = diarization_summary(&c);
        assert!(s.nemotron.checked(), "nemotron3 обязан проверить модель");
        assert!(
            matches!(s.nemotron, NemotronCheck::Failed(_)),
            "{:?}",
            s.nemotron
        );

        let _ = std::fs::remove_dir_all(&td);
    }

    /// W5: `diarization_summary` читает новые ключи и повторяет правило хоста
    /// «конфиг → авто»; sha256 не считается для файла неверного размера.
    #[test]
    fn diarization_summary_reads_config() {
        use super::{diarization_summary, DiarBackend, NemotronCheck};

        let td = std::env::temp_dir().join(format!("hds-diar-sum-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        std::fs::create_dir_all(&td).unwrap();
        let model = td.join("nemotron3_diar_v3.onnx");
        std::fs::write(&model, b"x").unwrap();
        let model_yaml = model.display().to_string().replace('\\', "/");

        let c = cfg(&format!(
            "diarization:\n  backend: nemotron3\n  nemotron3:\n    model: '{model_yaml}'\n    \
             device: directml\n    chunk_min: 99\n"
        ));
        let s = diarization_summary(&c);
        assert_eq!(s.backend, DiarBackend::Nemotron3);
        assert_eq!(s.configured.as_deref(), Some("nemotron3"));
        assert_eq!(s.nemotron_model.as_deref(), Some(model.as_path()));
        // Маленький файл не проходит проверку размера — и sha256 не считается.
        assert!(matches!(s.nemotron, NemotronCheck::Failed(_)));
        assert!(!s.nemotron_ok());
        assert!(s.warnings.iter().any(|w| w.contains("chunk_min")));

        // Неизвестный backend — предупреждение, дальше авто (модель не прошла ⇒ sortformer).
        let c = cfg(&format!(
            "diarization:\n  backend: bogus\n  nemotron3:\n    model: '{model_yaml}'\n"
        ));
        let s = diarization_summary(&c);
        assert_eq!(s.backend, DiarBackend::Sortformer);
        assert!(
            s.warnings.iter().any(|w| w.contains("'bogus'")),
            "{:?}",
            s.warnings
        );

        // off — прямой выбор; неизвестный device — предупреждение.
        let c = cfg("diarization:\n  backend: off\n  nemotron3:\n    device: gpu\n");
        let s = diarization_summary(&c);
        assert_eq!(s.backend, DiarBackend::Off);
        assert_eq!(s.device, "gpu");
        assert!(
            s.warnings.iter().any(|w| w.contains("'gpu'")),
            "{:?}",
            s.warnings
        );

        // Секции `diarization.backend` нет — backend решает модель. Явно задаём
        // несуществующий путь, чтобы тест не зависел от установленных моделей
        // машины: файла нет ⇒ sortformer.
        let absent = td
            .join("absent.onnx")
            .display()
            .to_string()
            .replace('\\', "/");
        let c = cfg(&format!(
            "index:\n  roots: []\ndiarization:\n  nemotron3:\n    model: '{absent}'\n"
        ));
        let s = diarization_summary(&c);
        assert!(s.configured.is_none());
        assert_eq!(s.backend, DiarBackend::Sortformer);
        assert!(
            !s.nemotron_model
                .as_deref()
                .map(|p| p.exists())
                .unwrap_or(false),
            "модель не должна существовать"
        );
        assert_eq!(s.device, "auto");

        let _ = std::fs::remove_dir_all(&td);
    }

    /// F2 (B2): фактический запрос конвейера — `None` без ключа, `off`/`ok`,
    /// явный `nemotron3` без модели — `fail` с командой установки, явный
    /// `sortformer` без `.gguf` — `warn`, неизвестное значение — `warn`.
    ///
    /// Модели передаются напрямую (`requested_backend_check_with`): результат не
    /// зависит от того, что установлено на машине (dev-машина с sortformer/whisper
    /// в `%APPDATA%` `directories` читает через Win32, env-переменной не подменить).
    #[test]
    fn requested_backend_check_statuses() {
        use super::requested_backend_check_with;

        // Ключа нет — строки нет (backend решит хост).
        assert!(requested_backend_check_with(None, None, None).is_none());
        assert!(requested_backend_check_with(Some("  "), None, None).is_none());

        // Явный off — ok (модели не нужны).
        let c = requested_backend_check_with(Some("off"), None, None).expect("off");
        assert_eq!(c.status, "ok");

        // Явный nemotron3 без модели — fail: запрос сильнее конфига хоста.
        let c = requested_backend_check_with(Some("nemotron3"), None, None).expect("nemotron3");
        assert_eq!(c.status, "fail");
        assert!(c.fix.contains("fetch_nemotron3_model"), "{}", c.fix);

        // Модель есть, но размер не тот — fail про проверку (sha256 не считается).
        let td = std::env::temp_dir().join(format!("hds-req-backend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        std::fs::create_dir_all(&td).unwrap();
        let model = td.join("nemotron3_diar_v3.onnx");
        std::fs::write(&model, b"x").unwrap();
        let hit =
            requested_backend_check_with(Some("nemotron3"), Some(&model), None).expect("nemotron3");
        assert_eq!(hit.status, "fail");
        assert!(hit.title.contains("не прошла проверку"), "{}", hit.title);

        // Явный sortformer: без `.gguf` — warn, с моделью — ok.
        let hit = requested_backend_check_with(Some("sortformer"), Some(&model), None)
            .expect("sortformer");
        assert_eq!(hit.status, "warn");
        assert!(hit.fix.contains("fetch_diarization_model"), "{}", hit.fix);
        let gguf = td.join("model.gguf");
        std::fs::write(&gguf, b"x").unwrap();
        let hit = requested_backend_check_with(Some("sortformer"), None, Some(&gguf))
            .expect("sortformer");
        assert_eq!(hit.status, "ok");
        assert!(hit.title.contains("sortformer"), "{}", hit.title);

        // Неизвестное значение — warn, задание не блокируем.
        let hit = requested_backend_check_with(Some("bogus"), None, None).expect("bogus");
        assert_eq!(hit.status, "warn");
        assert!(hit.title.contains("bogus"), "{}", hit.title);

        let _ = std::fs::remove_dir_all(&td);
    }

    /// F2: `requested_backend_of` повторяет правило клиента — `None`, пока ключ
    /// `auto_transcribe.diarization_backend` не задан явно.
    #[test]
    fn requested_backend_of_follows_client_key() {
        use super::requested_backend_of;
        assert_eq!(
            requested_backend_of(&cfg("auto_transcribe:\n  mode: transcript\n")),
            None
        );
        assert_eq!(
            requested_backend_of(&cfg(
                "auto_transcribe:\n  diarization_backend: sortformer\n"
            ))
            .as_deref(),
            Some("sortformer")
        );
        // Выключенная диаризация — поля в теле нет.
        assert_eq!(
            requested_backend_of(&cfg(
                "auto_transcribe:\n  diarization: false\n  diarization_backend: sortformer\n"
            )),
            None
        );
    }
}
