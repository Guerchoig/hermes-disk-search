//! `hds check` — порт `hds/diag.py::run_checks` «по смыслу»: компоненты окружения
//! и предупреждения. Формат вывода — как `cmd_check` (`[ok]/[--]/[!!]` + `-> fix`).
//!
//! Что покрываем в Rust: БД, корни, чат-роль, embedding-роль (+контекст), OCR
//! (tesseract), ffmpeg, лемматизатор (через `capabilities` воркера), реранк.
//! Осознанно **не** покрываем whisper/mpxj/vulkan — они проверяются Python-версией
//! до W5 (воркер этих возможностей не отдаёт); выводим одну поясняющую заметку.

use hds_core::config::{db_abs_path, dig, Config};
use hds_core::{db, EMB_CONTEXT};

use crate::support::{
    build_sidecar, probe_role, props_context, tesseract_ready, which, Probe, PROBE_TIMEOUT,
};

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

/// Проверка БД (как пункт 1 `diag.run_checks`): `ok`, либо `fail`/`warn` (занята).
pub fn check_db(cfg: &Config) -> Check {
    let path = db_abs_path(cfg);
    match db::connect(&path, crate::support::dim_of(cfg)) {
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
        .filter(|r| !std::path::Path::new(r).is_dir())
        .cloned()
        .collect();
    if missing.is_empty() {
        Check::new("roots", "ok", format!("Корни индексации: {}", roots.join(", ")))
    } else {
        Check::new(
            "roots",
            "warn",
            format!("Корни индексации не существуют: {}", missing.join(", ")),
        )
        .fix("Поправьте index.roots — укажите существующий диск/папку.")
    }
}

/// `cmd_check`: печать проверок и итог; 1 — есть `fail`. `json` — машинный вывод
/// (`{"ok":…, "checks":[…]}`), его использует веб-интерфейс (`/api/diagnostics`),
/// чтобы не дублировать логику проверок.
pub fn cmd_check(json: bool) -> i32 {
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "ok": false, "error": e.message() })
                );
            } else {
                println!("== hermes-disk-search: проверка окружения ==");
                println!("[!!] config.yaml недоступен: {}", e.message());
                println!("Итог: есть критические проблемы");
            }
            return 1;
        }
    };
    let checks = run_checks(&cfg);
    let ok = !checks.iter().any(|c| c.status == "fail");

    if json {
        let items: Vec<serde_json::Value> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id, "status": c.status, "title": c.title, "msg": c.msg, "fix": c.fix
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "ok": ok, "checks": items }));
        return if ok { 0 } else { 1 };
    }

    println!("== hermes-disk-search: проверка окружения ==");
    for c in &checks {
        if c.status == "ok" {
            println!("[ok] {}", c.title);
            continue;
        }
        println!(
            "[{}] {}",
            if c.status == "fail" { "!!" } else { "--" },
            c.title
        );
        if !c.msg.is_empty() {
            println!("     {}", c.msg);
        }
        if !c.fix.is_empty() {
            println!("     -> {}", c.fix);
        }
    }
    println!(
        "Итог: {}",
        if ok {
            "основные компоненты готовы"
        } else {
            "есть критические проблемы"
        }
    );
    if ok {
        0
    } else {
        1
    }
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
    checks.push(check_lemmatizer());
    if let Some(c) = check_rerank(cfg) {
        checks.push(c);
    }

    checks.push(
        Check::new(
            "python-only",
            "warn",
            "whisper / mpxj / Vulkan проверяются Python-версией (до W5)",
        )
        .msg("В Rust эти компоненты не проверяются: извлечение медиа/`.mpp` остаётся в Python.")
        .fix("Пока смотрите: python -m hds.cli check."),
    );
    checks
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
            let port = crate::support::role_addr(cfg, "chat").1;
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
        .fix("Запустите владельца портов: cargo run -p hds-llama --release --bin llm_host -- run"),
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
            let port = crate::support::role_addr(cfg, "embedding").1;
            Check::new(
                "emb",
                "fail",
                format!("Порт {port} занят посторонним сервисом (не роль embedding)"),
            )
            .fix("Освободите порт или смените llm_server.embedding.port в config.yaml.")
        }
        Probe::Down => {
            let path = crate::support::resolve_model(cfg, "embedding");
            if !path.is_file() {
                Check::new(
                    "emb",
                    "fail",
                    format!("GGUF-модель эмбеддингов не найдена: {}", path.display()),
                )
                .fix("Скачайте модель в общий llama-рантайм или запустите установщик рантайма.")
            } else {
                Check::new("emb", "fail", "Эмбеддинг-сервер llama.cpp не запущен").fix(
                    "Запустите владельца портов: cargo run -p hds-llama --release --bin llm_host -- run. \
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
        Check::new("ffmpeg", "warn", "ffmpeg не найден — видео без транскрипции")
            .fix("Установите Gyan.FFmpeg и перезапустите watcher/индексацию.")
    }
}

/// Пункт 7c: лемматизатор — по `capabilities` воркера (`normalize`).
fn check_lemmatizer() -> Check {
    let root = hds_core::config::project_root();
    match build_sidecar(&root) {
        Ok(sc) => {
            let has = sc.capabilities().has("normalize");
            sc.shutdown();
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
        Err(e) => Check::new(
            "lemmatizer",
            "warn",
            "Воркер лемматизации не запустился — морфология недоступна",
        )
        .msg(e.message())
        .fix("Проверьте sidecar/python или .venv (интерпретатор воркера)."),
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
    let ok = hds_index::embed::split_base(&url)
        .ok()
        .map(|(host, port, _)| {
            hds_core::http::request(&host, port, "GET", "/health", &[], None, PROBE_TIMEOUT)
                .map(|r| r.status == 200)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if ok {
        Some(Check::new("rerank", "ok", "Реранкер (llama-server) отвечает"))
    } else {
        Some(
            Check::new(
                "rerank",
                "warn",
                "rerank.enabled включён, но реранкер не отвечает — ask_my_files без реранкинга",
            )
            .fix("Запустите роль реранкера (владелец портов — llm_host)."),
        )
    }
}
