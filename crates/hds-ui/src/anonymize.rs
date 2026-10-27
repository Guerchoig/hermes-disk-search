//! Карточка UI «Анонимизация» (`PLAN_ANONYMIZER_SIDECAR.md` §2.5, фаза P5).
//!
//! По образцу `transcribe.rs`:
//! * вход — текстовое поле пути (файл или папка); браузер не отдаёт абсолютный
//!   путь из `<input type=file>`, поэтому путь вводится текстом, а сервер делает
//!   обзор `GET /api/anonymize/ls?path=…` (верхний уровень папки, форматы — из
//!   `hello.formats` воркера, §2.5/§3);
//! * выходной каталог — `anonymize.output_dir` из конфига, правится в карточке и
//!   сохраняется (`POST /api/config/anonymize-dirs`);
//! * 4 акта: «Скрыть всё» (`anonymize`), «Скрыть указанное» (`anonymize_values`),
//!   «Показать всё» (`deanonymize`), «Показать указанное»
//!   (`deanonymize_values`); одна активная пачка — вторая отклоняется;
//! * прогресс — пофайловый: акт идёт в фоновом потоке, карточка поллит
//!   `GET /api/anonymize/status` (как `llm-host/job`);
//! * отчёт по файлам: вход → выход, токены (token/type/count), счётчики
//!   matched/unmatched, warnings. **Значения PII наружу не отдаются** (§2.5):
//!   из ответов воркера берутся только токены и длины списков.
//!
//! GPU: `anonymize.gpu: auto` — lease у `llm_host` (как в CLI
//! `crates/hds-cli/src/cmd/anonymize.rs`), `off` — всегда CPU; недоступный lease —
//! fallback CPU с предупреждением в отчёте. Папку перечисляет Rust
//! (`hds_anonymize::list_files`), RPC — по одному файлу (§3).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use hds_anonymize::protocol::{out_path, Mode, TokenCount};
use hds_anonymize::{discover_python, Client, ClientConfig, Settings};
use hds_core::config::{load, project_root, Config};
use serde_json::{json, Value};

/// Форматы P1/P1b — запасной список, если воркер недоступен и `hello.formats`
/// получить нельзя (обзор папки должен работать даже без venv).
const FALLBACK_FORMATS: [&str; 5] = ["docx", "xlsx", "txt", "md", "xml"];

/// Повтор попытки получить форматы у воркера после неудачи (сек): не спавним
/// Python на каждый запрос обзора, но и не «запоминаем» поломку навсегда.
const FORMATS_RETRY_SECS: u64 = 60;

/// Предел «зависшего» акта (сек): после него новая пачка разрешена, даже если
/// поток не выставил результат (защита от deadlock — как у `LLM_JOB`).
const MAX_JOB_SECS: u64 = 6 * 3600;

/// Сетевой таймаут lease-запроса (не бюджет ожидания — его держит `llm_host`).
const LEASE_NET_TIMEOUT_SECS: u64 = 10;

/// Действие акта — 4 кнопки карточки (§2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// «Скрыть всё» — полный NER (`anonymize`).
    Anonymize,
    /// «Скрыть указанное» — детерминированный список значений (`anonymize_values`).
    AnonymizeValues,
    /// «Показать всё» — все маппинги БД (`deanonymize`).
    Deanonymize,
    /// «Показать указанное» — перечисленные токены (`deanonymize_values`).
    DeanonymizeValues,
}

impl Action {
    /// Код действия в API/отчёте.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Anonymize => "anonymize",
            Action::AnonymizeValues => "values",
            Action::Deanonymize => "deanonymize",
            Action::DeanonymizeValues => "deanon-values",
        }
    }

    /// Подпись кнопки для отчёта.
    pub fn title(self) -> &'static str {
        match self {
            Action::Anonymize => "скрыть всё",
            Action::AnonymizeValues => "скрыть указанное",
            Action::Deanonymize => "показать всё",
            Action::DeanonymizeValues => "показать указанное",
        }
    }

    /// Разбор кода действия из тела запроса.
    pub fn parse(s: &str) -> Option<Action> {
        match s.trim() {
            "anonymize" => Some(Action::Anonymize),
            "values" | "anonymize-values" => Some(Action::AnonymizeValues),
            "deanonymize" | "deanon" => Some(Action::Deanonymize),
            "deanon-values" | "deanonymize-values" => Some(Action::DeanonymizeValues),
            _ => None,
        }
    }

    /// Нужен ли список значений/токенов (иначе акт не имеет смысла).
    pub fn needs_items(self) -> bool {
        matches!(self, Action::AnonymizeValues | Action::DeanonymizeValues)
    }
}

/// Состояние фонового акта: одна активная пачка на процесс UI (§2.5).
struct AnonJob {
    action: Action,
    path: String,
    out_dir: String,
    total: usize,
    done: usize,
    failed: usize,
    current: String,
    stage: String,
    detail: String,
    started: Instant,
    /// Отчёт по завершённым файлам (без значений PII).
    files: Vec<Value>,
    result: Option<Value>,
}

static ANON_JOB: OnceLock<Mutex<Option<AnonJob>>> = OnceLock::new();

/// Кэш форматов: `(список, источник, когда получен)`. `worker` — навсегда,
/// `fallback` — до истечения [`FORMATS_RETRY_SECS`].
type FormatsCache = (Vec<String>, &'static str, Instant);
static ANON_FORMATS: OnceLock<Mutex<Option<FormatsCache>>> = OnceLock::new();

/// Нейтральное состояние «акта нет» (карточка показывает «готов к запуску»).
fn idle_status() -> Value {
    json!({
        "running": false,
        "action": Value::Null,
        "title": Value::Null,
        "path": Value::Null,
        "out_dir": Value::Null,
        "total": 0,
        "done": 0,
        "failed": 0,
        "current": "",
        "stage": "",
        "detail": "",
        "elapsed_sec": 0,
        "files": [],
        "result": Value::Null,
    })
}

/// Изменить состояние текущего акта (вне акта — ничего не делает).
fn anon_update(f: impl FnOnce(&mut AnonJob)) {
    let slot = ANON_JOB.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        if let Some(j) = g.as_mut() {
            f(j);
        }
    }
}

/// Идёт ли акт прямо сейчас.
fn job_running() -> bool {
    ANON_JOB
        .get()
        .and_then(|m| m.lock().ok())
        .map(|g| g.as_ref().map(|j| j.result.is_none()).unwrap_or(false))
        .unwrap_or(false)
}

/// Начать акт. `false` — другая пачка ещё идёт (или зависла дольше
/// [`MAX_JOB_SECS`] — считаем поток мёртвым и поднимаем новый).
fn job_start(action: Action, path: &str, out_dir: &str) -> bool {
    let slot = ANON_JOB.get_or_init(|| Mutex::new(None));
    let mut g = match slot.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if let Some(j) = g.as_ref() {
        if j.result.is_none() && j.started.elapsed().as_secs() < MAX_JOB_SECS {
            return false;
        }
    }
    *g = Some(AnonJob {
        action,
        path: path.to_string(),
        out_dir: out_dir.to_string(),
        total: 0,
        done: 0,
        failed: 0,
        current: String::new(),
        stage: "старт".to_string(),
        detail: "подготовка акта".to_string(),
        started: Instant::now(),
        files: Vec::new(),
        result: None,
    });
    true
}

/// Завершить акт результатом (остаётся доступным до следующего старта).
fn job_finish(res: Value) {
    anon_update(|j| {
        j.stage = "готово".to_string();
        j.current.clear();
        j.result = Some(res);
    });
}

/// Запасной список форматов (воркер недоступен).
fn fallback_formats() -> Vec<String> {
    FALLBACK_FORMATS.iter().map(|s| s.to_string()).collect()
}

fn cached_formats() -> Option<(Vec<String>, &'static str, Instant)> {
    ANON_FORMATS
        .get()
        .and_then(|m| m.lock().ok())
        .and_then(|g| g.clone())
}

fn store_formats(formats: Vec<String>, source: &'static str) {
    let slot = ANON_FORMATS.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        *g = Some((formats, source, Instant::now()));
    }
}

/// Форматы для обзора папки: кэш → `hello` воркера → запасной список.
///
/// Воркер спавнится только если пачка не идёт (иначе два воркера на одну БД) и
/// кэш не свеж. Источник (`worker`/`fallback`) возвращается в ответ, чтобы
/// карточка показала предупреждение, если форматы взяты по умолчанию.
fn worker_formats(cfg: &Config, root: &Path) -> (Vec<String>, &'static str) {
    if let Some((formats, source, at)) = cached_formats() {
        let fresh = source == "worker" || at.elapsed().as_secs() < FORMATS_RETRY_SECS;
        if fresh && !formats.is_empty() {
            return (formats, source);
        }
    }
    if job_running() {
        return (fallback_formats(), "fallback");
    }
    let settings = Settings::from_config(cfg);
    if let Some(python) = discover_python(root) {
        if let Ok(mut client) = Client::spawn(ClientConfig::from_settings(&python, root, &settings))
        {
            match client.hello() {
                Ok(h) if !h.formats.is_empty() => {
                    let formats = h.formats;
                    client.shutdown();
                    store_formats(formats.clone(), "worker");
                    return (formats, "worker");
                }
                _ => client.shutdown(),
            }
        }
    }
    let formats = fallback_formats();
    store_formats(formats.clone(), "fallback");
    (formats, "fallback")
}

/// Похоже ли имя на результат анонимизации (`.anonymized`/`.deanon`).
/// Такие файлы НЕ скрываем из обзора (деанонимизация идёт именно по ним), но
/// помечаем в карточке, чтобы повторный «Скрыть всё» был осознанным (реш. №2).
fn is_processed_name(p: &Path) -> bool {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.contains(".anonymized.") || name.contains(".deanon.")
}

/// Абсолютный путь: относительный ввод разрешается от корня проекта.
fn resolve_path(raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        p
    } else {
        project_root().join(p)
    }
}

/// Элемент списка файлов для карточки (пути и размеры — не PII-значения).
fn file_entry(p: &Path, out_dir: &Path) -> Option<Value> {
    let name = p.file_name().and_then(|n| n.to_str())?.to_string();
    let md = std::fs::metadata(p).ok();
    let size = md.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = md
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    Some(json!({
        "name": name,
        "path": p.display().to_string(),
        "size": size,
        "mtime": mtime,
        "ext": ext,
        "anonymized_out": out_path(p, out_dir, Mode::Anonymize).display().to_string(),
        "deanon_out": out_path(p, out_dir, Mode::Deanonymize).display().to_string(),
        "processed": is_processed_name(p),
    }))
}

/// Токены акта → JSON (только токен/тип/число замен, §2.5).
fn tokens_json(tokens: &[TokenCount]) -> Value {
    Value::Array(
        tokens
            .iter()
            .map(|t| {
                json!({
                    "token": t.token,
                    "type": t.entity_type,
                    "count": t.count,
                })
            })
            .collect(),
    )
}

/// Список значений/токенов из тела запроса: массив строк или многострочный
/// текст (по одному на строку). Пустые строки отбрасываются.
///
/// Значения PII **не** возвращаются в ответ и не попадают в состояние акта.
fn items_of(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split('\n')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// `GET /api/anonymize/ls?path=…`: обзор входа (§2.5).
///
/// Папка — файлы верхнего уровня в форматах `hello.formats` воркера (решение
/// №8); файл — он сам (формат проверяется по тому же списку). В ответе также
/// `out_dir`, БД, режим GPU и источник форматов.
pub fn ls_json(path: &str) -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let root = project_root();
    let settings = Settings::from_config(&cfg);
    let out_dir = settings.output_dir_abs(&root);
    let (formats, source) = worker_formats(&cfg, &root);
    let mut out = json!({
        "out_dir": out_dir.display().to_string(),
        "db_path": settings.db_abs(&root).display().to_string(),
        "formats": formats,
        "formats_source": source,
        "gpu": settings.gpu,
        "path": Value::Null,
        "kind": Value::Null,
        "count": 0,
        "files": [],
    });
    let raw = path.trim().trim_matches('"');
    if raw.is_empty() {
        out["error"] = json!("путь не задан — введите файл или папку");
        return out;
    }
    let p = resolve_path(raw);
    out["path"] = json!(p.display().to_string());
    if !p.exists() {
        out["error"] = json!(format!("путь не найден: {}", p.display()));
        return out;
    }
    let mut entries: Vec<Value> = Vec::new();
    if p.is_dir() {
        out["kind"] = json!("dir");
        for f in hds_anonymize::list_files(&p, &formats) {
            if let Some(e) = file_entry(&f, &out_dir) {
                entries.push(e);
            }
        }
        if entries.is_empty() {
            out["note"] = json!(format!(
                "в папке нет файлов поддерживаемых форматов ({})",
                formats.join(", ")
            ));
        }
    } else if p.is_file() {
        out["kind"] = json!("file");
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !formats.iter().any(|f| f == &ext) {
            out["error"] = json!(format!(".{ext} не поддерживается ({})", formats.join(", ")));
            return out;
        }
        if is_processed_name(&p) {
            out["note"] = json!("файл похож на результат анонимизации (.anonymized/.deanon)");
        }
        if let Some(e) = file_entry(&p, &out_dir) {
            entries.push(e);
        }
    } else {
        out["error"] = json!("путь не является файлом или папкой");
        return out;
    }
    out["count"] = json!(entries.len());
    out["files"] = Value::Array(entries);
    out
}

/// Предел записей в одном каталоге для окна выбора («Обзор…»): защита от
/// гигантских папок, в ответе есть признак `truncated`.
const BROWSE_LIMIT: usize = 2000;

/// `GET /api/anonymize/browse?path=…`: серверный выбор файла/папки для кнопки
/// «Обзор…» (браузер не отдаёт абсолютный путь из `<input type=file>`).
///
/// * пустой `path` — список корней (диски Windows, `/` и домашний каталог);
/// * каталог — подпапки и файлы поддерживаемых форматов;
/// * файл — его каталог (удобно, если в поле уже стоит путь к файлу).
///
/// Файлы неподдерживаемых форматов не показываются: выбрать их для акта нельзя.
pub fn browse_json(path: &str) -> Value {
    let cfg = load().unwrap_or(Config::Null);
    let root = project_root();
    let settings = Settings::from_config(&cfg);
    let (formats, source) = worker_formats(&cfg, &root);
    let mut out = json!({
        "out_dir": settings.output_dir_abs(&root).display().to_string(),
        "db_path": settings.db_abs(&root).display().to_string(),
        "formats": formats,
        "formats_source": source,
        "path": Value::Null,
        "parent": Value::Null,
        "roots": browse_roots(),
        "dirs": [],
        "files": [],
        "truncated": false,
    });
    let raw = path.trim().trim_matches('"');
    if raw.is_empty() {
        return out;
    }
    let mut p = resolve_path(raw);
    if p.is_file() {
        p = p.parent().map(Path::to_path_buf).unwrap_or(p);
    }
    out["path"] = json!(p.display().to_string());
    if !p.is_dir() {
        out["error"] = json!(format!("путь не найден: {}", p.display()));
        return out;
    }
    out["parent"] = p
        .parent()
        .filter(|x| !x.as_os_str().is_empty())
        .map(|x| json!(x.display().to_string()))
        .unwrap_or(Value::Null);
    match browse_entries(&p, &formats) {
        Ok((dirs, files, truncated)) => {
            out["dirs"] = Value::Array(dirs);
            out["files"] = Value::Array(files);
            out["truncated"] = json!(truncated);
        }
        Err(e) => out["error"] = json!(format!("чтение папки: {e}")),
    }
    out
}

/// Корни для начала выбора: диски Windows или `/` (и `$HOME`) на прочих ОС.
fn browse_roots() -> Vec<Value> {
    let mut v: Vec<Value> = Vec::new();
    #[cfg(windows)]
    {
        for c in b'A'..=b'Z' {
            let p = format!("{}:\\", c as char);
            if Path::new(&p).is_dir() {
                v.push(json!({ "name": p, "path": p }));
            }
        }
    }
    #[cfg(not(windows))]
    {
        v.push(json!({ "name": "/", "path": "/" }));
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let p = PathBuf::from(home).display().to_string();
            v.push(json!({ "name": p, "path": p }));
        }
    }
    v
}

/// Содержимое каталога для [`browse_json`]: подпапки и файлы форматов акта.
/// Порядок — по имени без учёта регистра; предел — [`BROWSE_LIMIT`].
fn browse_entries(
    dir: &Path,
    formats: &[String],
) -> std::io::Result<(Vec<Value>, Vec<Value>, bool)> {
    let mut dirs: Vec<Value> = Vec::new();
    let mut files: Vec<Value> = Vec::new();
    let mut truncated = false;
    for ent in std::fs::read_dir(dir)?.flatten() {
        let name = match ent.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let p = ent.path();
        let ft = match ent.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_dir() {
            dirs.push(json!({
                "name": name,
                "path": p.display().to_string(),
            }));
        } else if ft.is_file() {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !formats.iter().any(|f| f == &ext) {
                continue;
            }
            let size = ent.metadata().map(|m| m.len()).unwrap_or(0);
            files.push(json!({
                "name": name,
                "path": p.display().to_string(),
                "size": size,
                "ext": ext,
            }));
        }
        if dirs.len() + files.len() >= BROWSE_LIMIT {
            truncated = true;
            break;
        }
    }
    dirs.sort_by_key(browse_name_key);
    files.sort_by_key(browse_name_key);
    Ok((dirs, files, truncated))
}

/// Ключ сортировки списка выбора: имя без учёта регистра.
fn browse_name_key(v: &Value) -> String {
    v["name"].as_str().unwrap_or("").to_lowercase()
}

/// `POST /api/anonymize/run`: запустить акт в фоне (§2.5).
///
/// Тело: `{path, action, items?}`. Ответ сразу: `{ok, started}` либо ошибка
/// валидации/занятости. `items` (значения или токены) в ответ не возвращаются —
/// только их количество. Одна активная пачка: вторая получает `ok:false`.
pub fn run_json(body: &str) -> Value {
    let v = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let path_raw = v
        .get("path")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if path_raw.is_empty() {
        return json!({ "ok": false, "error": "путь не задан — введите файл или папку" });
    }
    let action = match v
        .get("action")
        .and_then(|x| x.as_str())
        .and_then(Action::parse)
    {
        Some(a) => a,
        None => {
            return json!({ "ok": false,
                "error": "неизвестное действие: ожидается anonymize | values | deanonymize | deanon-values" })
        }
    };
    let items = items_of(v.get("items"));
    if action.needs_items() && items.is_empty() {
        return json!({ "ok": false,
            "error": "нужен хотя бы один элемент списка (значения или токены, по одному в строке)" });
    }
    let path = resolve_path(&path_raw);
    if !path.exists() {
        return json!({ "ok": false, "error": format!("путь не найден: {}", path.display()) });
    }
    if !(path.is_file() || path.is_dir()) {
        return json!({ "ok": false,
            "error": format!("путь не является файлом или папкой: {}", path.display()) });
    }
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "ok": false, "error": format!("конфиг: {}", e.message()) }),
    };
    let settings = Settings::from_config(&cfg);
    let out_dir = settings.output_dir_abs(&project_root());
    let path_str = path.display().to_string();
    let out_str = out_dir.display().to_string();
    let items_count = items.len();
    if !job_start(action, &path_str, &out_str) {
        return json!({ "ok": false,
            "error": "акт уже идёт — дождитесь завершения (прогресс и отчёт в карточке)" });
    }
    let action_str = action.as_str();
    let title = action.title();
    std::thread::spawn(move || {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_batch(action, &path, &items)
        }))
        .unwrap_or_else(
            |_| json!({ "ok": false, "error": "внутренняя ошибка потока анонимизации" }),
        );
        job_finish(res);
    });
    json!({
        "ok": true,
        "started": true,
        "action": action_str,
        "title": title,
        "path": path_str,
        "out_dir": out_str,
        "items_count": items_count,
        "msg": "акт запущен — прогресс и отчёт в карточке",
    })
}

/// `GET /api/anonymize/status`: ход и результат последнего акта (§2.5).
pub fn status_json() -> Value {
    let slot = ANON_JOB.get_or_init(|| Mutex::new(None));
    let g = match slot.lock() {
        Ok(g) => g,
        Err(_) => return idle_status(),
    };
    match g.as_ref() {
        Some(j) => json!({
            "running": j.result.is_none(),
            "action": j.action.as_str(),
            "title": j.action.title(),
            "path": j.path,
            "out_dir": j.out_dir,
            "total": j.total,
            "done": j.done,
            "failed": j.failed,
            "current": j.current,
            "stage": j.stage,
            "detail": j.detail,
            "elapsed_sec": j.started.elapsed().as_secs(),
            "files": j.files,
            "result": j.result,
        }),
        None => idle_status(),
    }
}

/// Отчёт об ошибке файла: машинный код + текст + hint (без значений PII).
fn error_json(name: &str, e: &hds_anonymize::AnonymizeError) -> Value {
    json!({
        "file": name,
        "ok": false,
        "kind": e.kind.as_str(),
        "error": e.message,
        "hint": e.hint,
    })
}

/// Акт целиком (в фоновом потоке): lease → воркер → пофайловый RPC → отчёт.
///
/// Возвращает JSON-итог (он же `result` в статусе). Значения PII в отчёт не
/// попадают: для списков значений берутся только длины (`matched_count`/
/// `unmatched_count`), для токенов — сами токены (они не PII).
fn run_batch(action: Action, path: &Path, items: &[String]) -> Value {
    let started = Instant::now();
    anon_update(|j| {
        j.stage = "конфиг".to_string();
        j.detail = "чтение config.yaml".to_string();
    });
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return json!({ "ok": false, "error": format!("конфиг: {}", e.message()) }),
    };
    let root = project_root();
    let settings = Settings::from_config(&cfg);
    let out_dir = settings.output_dir_abs(&root);
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        return json!({ "ok": false,
            "error": format!("каталог выхода {}: {e}", out_dir.display()) });
    }
    let python = match discover_python(&root) {
        Some(p) => p,
        None => {
            return json!({ "ok": false, "kind": "internal",
                "error": "интерпретатор воркера анонимизации не найден",
                "hint": "поставьте venv-anonymize (HDS_ANONYMIZE_PYTHON) или sidecar\\python" })
        }
    };

    // P3 (§2.2): lease у llm_host при `anonymize.gpu: auto`; `off` — всегда CPU.
    // Недоступность хоста — fallback CPU с предупреждением; `gpu_busy` (бюджет
    // ожидания chat истёк) — акт отклоняется, повтор решает пользователь.
    let mut device = "cpu";
    let mut gpu_limit = 0u64;
    let mut lease_note = Value::Null;
    let mut _lease: Option<hds_anonymize::LeaseGrant> = None;
    if settings.gpu != "off" {
        anon_update(|j| {
            j.stage = "lease".to_string();
            j.detail = format!("запрос lease у llm_host ({})", settings.llm_host_url);
        });
        match hds_anonymize::lease::acquire(
            &settings.llm_host_url,
            settings.lease_timeout_sec,
            settings.gpu_reserve_mb,
            Duration::from_secs(LEASE_NET_TIMEOUT_SECS),
        ) {
            Ok(grant) => {
                let limit = grant.gpu_mem_limit_mb;
                device = "cuda";
                gpu_limit = limit;
                anon_update(|j| {
                    j.detail =
                        format!("lease 'anon' выдан (лимит ORT {limit} МиБ) — GLiNER на GPU");
                });
                _lease = Some(grant); // Drop → /internal/anon/exit
            }
            Err(hds_anonymize::LeaseError::Unavailable(m)) => {
                lease_note = json!(format!("lease недоступен ({m}) — GLiNER на CPU (fallback)"));
            }
            Err(busy) => {
                let e = hds_anonymize::lease::to_anonymize_error(&busy);
                return json!({
                    "ok": false, "kind": e.kind.as_str(), "error": e.message, "hint": e.hint,
                    "lease": Value::Null,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                });
            }
        }
    }

    anon_update(|j| {
        j.stage = "воркер".to_string();
        j.detail = "запуск Python-воркера".to_string();
    });
    let mut ccfg = ClientConfig::from_settings(&python, &root, &settings);
    ccfg.extra_args = settings.worker_args_for(&root, device, gpu_limit);
    let mut client = match Client::spawn(ccfg) {
        Ok(c) => c,
        Err(e) => {
            return json!({
                "ok": false, "kind": e.kind.as_str(), "error": e.message, "hint": e.hint,
                "device": device, "lease": lease_note,
                "elapsed_ms": started.elapsed().as_millis() as u64,
            })
        }
    };

    // Папку перечисляем сами (§3); форматы — из `hello` воркера (решение №8).
    let files: Vec<PathBuf> = if path.is_dir() {
        anon_update(|j| {
            j.stage = "обзор".to_string();
            j.detail = "форматы воркера (hello)".to_string();
        });
        match client.hello() {
            Ok(h) => {
                if !h.formats.is_empty() {
                    store_formats(h.formats.clone(), "worker");
                }
                hds_anonymize::list_files(path, &h.formats)
            }
            Err(e) => {
                let kind = e.kind.as_str();
                let msg = e.message.clone();
                let hint = e.hint.clone();
                client.shutdown();
                return json!({ "ok": false, "kind": kind,
                    "error": format!("hello: {msg}"), "hint": hint });
            }
        }
    } else {
        vec![path.to_path_buf()]
    };
    if files.is_empty() {
        client.shutdown();
        return json!({ "ok": false, "kind": "unsupported",
            "error": "в папке нет файлов поддерживаемых форматов",
            "hint": "поддерживаются docx/xlsx/txt/md/xml; верхний уровень папки" });
    }
    anon_update(|j| {
        j.total = files.len();
        j.detail = format!("файлов: {}", files.len());
    });

    let mut reports: Vec<Value> = Vec::with_capacity(files.len());
    let mut failed = 0usize;
    for file in &files {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.display().to_string());
        anon_update(|j| {
            j.current = name.clone();
            j.stage = "файл".to_string();
            j.detail = "обработка".to_string();
        });
        let rep = match action {
            Action::Anonymize => match client.anonymize(file, &out_dir, None) {
                Ok(r) => json!({
                    "file": name, "ok": true, "out_path": r.out_path,
                    "tokens": tokens_json(&r.tokens), "warnings": r.warnings,
                    "elapsed_ms": r.elapsed_ms,
                }),
                Err(e) => error_json(&name, &e),
            },
            Action::AnonymizeValues => match client.anonymize(file, &out_dir, Some(items)) {
                Ok(r) => json!({
                    "file": name, "ok": true, "out_path": r.out_path,
                    "tokens": tokens_json(&r.tokens),
                    "matched_count": r.matched.len(),
                    "unmatched_count": r.unmatched.len(),
                    "warnings": r.warnings, "elapsed_ms": r.elapsed_ms,
                }),
                Err(e) => error_json(&name, &e),
            },
            // `restored` — восстановленные ТОКЕНЫ (не PII): их показывать можно.
            // `unmatched` в values-режиме — исходные значения → только счётчик.
            Action::Deanonymize => match client.deanonymize(file, &out_dir, None) {
                Ok(r) => json!({
                    "file": name, "ok": true, "out_path": r.out_path,
                    "restored": r.restored, "unmatched_count": r.unmatched.len(),
                    "warnings": r.warnings, "elapsed_ms": r.elapsed_ms,
                }),
                Err(e) => error_json(&name, &e),
            },
            Action::DeanonymizeValues => match client.deanonymize(file, &out_dir, Some(items)) {
                Ok(r) => json!({
                    "file": name, "ok": true, "out_path": r.out_path,
                    "restored": r.restored, "unmatched_count": r.unmatched.len(),
                    "warnings": r.warnings, "elapsed_ms": r.elapsed_ms,
                }),
                Err(e) => error_json(&name, &e),
            },
        };
        if !rep["ok"].as_bool().unwrap_or(false) {
            failed += 1;
        }
        anon_update(|j| {
            j.done += 1;
            j.failed = failed;
            j.files.push(rep.clone());
        });
        reports.push(rep);
    }

    client.shutdown();
    let ok = failed == 0;
    let msg = if ok {
        format!("обработано файлов: {}", files.len())
    } else {
        format!("неудач: {failed} из {}", files.len())
    };
    json!({
        "ok": ok,
        "action": action.as_str(),
        "title": action.title(),
        "path": path.display().to_string(),
        "out_dir": out_dir.display().to_string(),
        "total": files.len(),
        "failed": failed,
        "device": device,
        "lease": lease_note,
        "files": reports,
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "msg": msg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_parse_and_items() {
        assert_eq!(Action::parse("anonymize"), Some(Action::Anonymize));
        assert_eq!(Action::parse("values"), Some(Action::AnonymizeValues));
        assert_eq!(Action::parse("deanonymize"), Some(Action::Deanonymize));
        assert_eq!(
            Action::parse("deanon-values"),
            Some(Action::DeanonymizeValues)
        );
        assert_eq!(Action::parse("bogus"), None);
        assert!(!Action::Anonymize.needs_items());
        assert!(Action::AnonymizeValues.needs_items());
        assert!(Action::DeanonymizeValues.needs_items());
        assert_eq!(Action::Anonymize.as_str(), "anonymize");
        assert_eq!(Action::Anonymize.title(), "скрыть всё");
    }

    #[test]
    fn items_parse_lines_and_array() {
        let text = json!("  Иван Иванов \n\n [PERSON_1]\r\nПетров ");
        assert_eq!(
            items_of(Some(&text)),
            vec![
                "Иван Иванов".to_string(),
                "[PERSON_1]".to_string(),
                "Петров".to_string()
            ]
        );
        let arr = json!(["a", " b ", "", "c"]);
        assert_eq!(
            items_of(Some(&arr)),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert!(items_of(None).is_empty());
        assert!(items_of(Some(&json!(42))).is_empty());
    }

    #[test]
    fn processed_names_detected() {
        assert!(is_processed_name(Path::new("D:/out/a.anonymized.docx")));
        assert!(is_processed_name(Path::new("D:/out/a.deanon.txt")));
        assert!(!is_processed_name(Path::new("D:/in/a.docx")));
    }

    #[test]
    fn browse_entries_lists_dirs_and_supported_files_only() {
        let dir = std::env::temp_dir().join(format!("hds-anon-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        std::fs::write(dir.join("a.docx"), b"x").unwrap();
        std::fs::write(dir.join("b.txt"), b"x").unwrap();
        std::fs::write(dir.join("skip.bin"), b"x").unwrap();
        let formats = vec!["docx".to_string(), "txt".to_string()];
        let (dirs, files, truncated) = browse_entries(&dir, &formats).unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0]["name"], "Sub");
        // поддерживаемые форматы — по имени; .bin в список не попал
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["name"], "a.docx");
        assert_eq!(files[1]["name"], "b.txt");
        assert_eq!(files[0]["ext"], "docx");
        assert!(!truncated, "маленький каталог не должен обрезаться");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_serialize_without_values() {
        let tokens = vec![TokenCount {
            token: "[PERSON_1]".to_string(),
            entity_type: "PERSON".to_string(),
            count: 3,
        }];
        let v = tokens_json(&tokens);
        assert_eq!(v[0]["token"], "[PERSON_1]");
        assert_eq!(v[0]["type"], "PERSON");
        assert_eq!(v[0]["count"], 3);
        // в JSON нет полей со значениями PII
        assert!(v[0].get("value").is_none());
    }

    #[test]
    fn run_validation_without_config() {
        // Валидация до чтения конфига/запуска воркера — безопасно в юнит-тестах.
        let v = run_json("{}");
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap_or("").contains("путь не задан"));
        let v = run_json(r#"{"path":"D:/nope","action":"bogus"}"#);
        assert_eq!(v["ok"], false);
        let v = run_json(r#"{"path":"D:/nope","action":"values"}"#);
        assert_eq!(v["ok"], false);
        assert!(v["error"]
            .as_str()
            .unwrap_or("")
            .contains("хотя бы один элемент"));
    }

    #[test]
    fn idle_status_shape() {
        let v = status_json();
        // без запущенного акта — нейтральное состояние
        if v["running"] == false {
            assert!(v["files"].is_array());
            assert!(v["result"].is_null() || v["result"].is_object());
        }
    }
}
