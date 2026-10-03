//! MCP-инструменты disk-search — порт `hds/mcp_server.py`: поиск, RAG, статус,
//! индексация. Возвращают готовые строки (как Python-инструменты).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use hds_core::config::{db_abs_path, dig, load, project_root, Config};
use hds_core::db;
use hds_index::heartbeat::{self, HeartbeatFile};
use hds_index::transcribe::MediaRouter;
use hds_index::{Embedder, RunIndexArgs, Sidecar};
use hds_search::{format_location, SearchResult};

/// Запущена ли фоновая индексация этим процессом MCP (порт `_idx_state["running"]`).
static INDEXING: AtomicBool = AtomicBool::new(false);

/// Подключение к БД (порт `_conn`).
fn connect(cfg: &Config) -> Result<rusqlite::Connection, String> {
    let dim = dig(cfg, "embedding.dim")
        .and_then(|v| v.as_i64())
        .unwrap_or(1024);
    db::connect(&db_abs_path(cfg), dim).map_err(|e| e.message())
}

/// Запустить Python-воркер (лемматизация/извлечение) — как `build_sidecar` в CLI.
/// Для пакетных операций (индексация/переиндексация): отдельный процесс,
/// владелец глушит его после работы.
fn sidecar(root: &Path) -> Result<Sidecar, String> {
    let py = hds_extract::discover_python(root)
        .ok_or_else(|| format!("не найден интерпретатор воркера в {}", root.display()))?;
    Sidecar::spawn(&py, root, false).map_err(|e| e.message())
}

/// Общий sidecar-воркер: ОДИН процесс на всё время жизни MCP-сервера.
///
/// Раньше каждый вызов поиска/RAG поднимал свой `python.exe` и глуш его после
/// ответа — на Windows это давало видимую консоль на каждый запрос (детачед
/// родитель без консоли → ребёнку создаётся новая консоль; плюс повторные
/// холодные старты интерпретатора). Теперь воркер живёт между запросами и
/// сам выходит по idle-таймауту (60 с в `WorkerConfig`); если он успел
/// выйти или упал — при следующем вызове поднимается свежий.
static SIDECAR: OnceLock<Mutex<Option<Arc<Sidecar>>>> = OnceLock::new();

fn shared_sidecar(root: &Path) -> Result<Arc<Sidecar>, String> {
    let slot = SIDECAR.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .map_err(|_| "sidecar: блокировка отравлена".to_string())?;
    // Воркер умер (idle-выход, сбой) → заменить свежим при этом же вызове.
    if guard.as_ref().is_some_and(|s| !s.is_alive()) {
        *guard = None;
    }
    if let Some(s) = guard.as_ref() {
        return Ok(Arc::clone(s));
    }
    let py = hds_extract::discover_python(root)
        .ok_or_else(|| format!("не найден интерпретатор воркера в {}", root.display()))?;
    let s = Sidecar::spawn(&py, root, false).map_err(|e| e.message())?;
    let arc = Arc::new(s);
    *guard = Some(Arc::clone(&arc));
    Ok(arc)
}

/// Форматирование результатов (порт `_format_results`).
fn format_results(results: &[SearchResult]) -> String {
    let mut lines = Vec::new();
    for (i, r) in results.iter().enumerate() {
        lines.push(format!(
            "[{}] {}",
            i + 1,
            format_location(&r.path, r.page, r.t_start)
        ));
        let snip: String = r.snippet.replace('\n', " ").chars().take(800).collect();
        lines.push(format!("    {snip}"));
    }
    lines.join("\n")
}

/// `кindы` из строки `a,b` (пусто → `None`).
fn parse_kinds(kinds: &str) -> Option<Vec<String>> {
    let v: Vec<String> = kinds
        .split(',')
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Инструмент `search_local_files` (ГЛАВНЫЙ — «найди на этом компе/диске…»).
pub fn search_local_files(query: &str, limit: i64, kinds: &str) -> String {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return format!("Ошибка конфигурации: {}", e.message()),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return format!("База данных недоступна: {e}"),
    };
    let emb = Embedder::from_config(&cfg);
    let root = project_root();
    let side = match shared_sidecar(&root) {
        Ok(s) => s,
        Err(e) => return format!("Воркер извлечения/лемматизации: {e}"),
    };
    let kinds_v = parse_kinds(kinds);
    let lim = limit.clamp(1, 30) as usize;
    let res = hds_search::search(
        &conn,
        Some(&emb),
        side.as_ref(),
        &cfg,
        query,
        kinds_v.as_deref(),
        lim,
    );
    if res.is_empty() {
        return format!(
            "Ничего не найдено по запросу: {query}. Если ожидаете файлы — уточните запрос \
             или проверьте состояние индекса инструментом index_status."
        );
    }
    // Выдача упёрлась в лимит → это заведомо не весь результат: подсказываем
    // повторить поиск шире, иначе модель выдаёт первые N фрагментов за полный ответ.
    let capped = res.len() as i64 >= limit.clamp(1, 30) && res.len() < 30;
    format!(
        "Найдено {} фрагментов:\n\n{}\n\nОтвечая пользователю, приводи пути файлов и номера \
         источников [N]. Одна выдача — это фрагменты, а не список файлов: на обзорные вопросы \
         («какие есть…», «есть ли ещё…») повтори поиск другими формулировками и с другими \
         kinds и объедини результаты.{} Для готового ответа используй инструмент ask_my_files.",
        res.len(),
        format_results(&res),
        if capped {
            " Выдача упёрлась в limit — увеличьте его (до 30) и/или сделайте ещё запросы."
        } else {
            ""
        }
    )
}

/// Инструмент `ask_my_files` (RAG-ответ по содержимому файлов).
///
/// `limit` — глубина выборки фрагментов, попадающих в контекст ответа. Обзорные
/// вопросы («какие есть ТЗ/проекты/документы») требуют больше контекста, чем
/// точечные: при 8 фрагментах модель объявляет полным очевидно неполный ответ.
pub fn ask_my_files(question: &str, limit: i64) -> String {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return format!("Ошибка конфигурации: {}", e.message()),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return format!("База данных недоступна: {e}"),
    };
    let emb = Embedder::from_config(&cfg);
    let root = project_root();
    let side = match shared_sidecar(&root) {
        Ok(s) => s,
        Err(e) => return format!("Воркер извлечения/лемматизации: {e}"),
    };
    let lim = limit.clamp(1, 30) as usize;
    let out = hds_search::ask(&conn, Some(&emb), side.as_ref(), &cfg, question, lim);
    let src = out
        .sources
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "[{}] {}",
                i + 1,
                format_location(&r.path, r.page, r.t_start)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n\nИсточники:\n{}",
        out.answer,
        if src.is_empty() {
            "(нет)"
        } else {
            src.as_str()
        }
    )
}

/// Инструмент `index_status` (состояние индекса).
pub fn index_status() -> String {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return format!("Ошибка конфигурации: {}", e.message()),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return format!("База данных недоступна: {e}"),
    };
    let st = db::stats(&conn).unwrap_or_default();
    let hb = HeartbeatFile::new(&project_root());
    let running = INDEXING.load(Ordering::Relaxed) || heartbeat::index_running(&hb, 30.0);
    let mut lines = vec![format!(
        "Индексация сейчас: {}",
        if running {
            "идёт"
        } else {
            "не запущена"
        }
    )];
    // прогресс — из heartbeat-файла (кросс-процессный; в Python — in-process reporter)
    if let Some(v) = hb.read() {
        let seen = v.get("seen").and_then(|x| x.as_u64());
        let processed = v.get("processed").and_then(|x| x.as_u64());
        let errors = v.get("errors").and_then(|x| x.as_u64());
        if seen.is_some() || processed.is_some() {
            lines.push(format!(
                "Прогресс: просмотрено {}, обработано {}, ошибок {}",
                seen.unwrap_or(0),
                processed.unwrap_or(0),
                errors.unwrap_or(0)
            ));
        }
        if let Some(p) = v.get("path").and_then(|x| x.as_str()) {
            let phase = v.get("phase").and_then(|x| x.as_str()).unwrap_or("");
            lines.push(format!("Сейчас: {p} ({phase})"));
        }
    }
    lines.push(format!(
        "Файлы по типам: {}",
        st.by_kind
            .iter()
            .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_else(|| "?".into()), n))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines.push(format!(
        "По статусам: {}",
        st.by_status
            .iter()
            .map(|(k, n)| format!("{}={}", k.clone().unwrap_or_default(), n))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines.push(format!("Чанков: {}", st.chunks));
    if !st.errors.is_empty() {
        lines.push(format!(
            "Примеры ошибок: {}",
            st.errors
                .iter()
                .take(5)
                .map(|(p, _)| p.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    lines.join("\n")
}

/// Инструмент `start_indexing` (фоновая индексация корней из конфига).
pub fn start_indexing(full: bool) -> String {
    let hb = HeartbeatFile::new(&project_root());
    if INDEXING.load(Ordering::Relaxed) || heartbeat::index_running(&hb, 30.0) {
        return "Индексация уже идёт. Проверьте инструментом index_status.".to_string();
    }
    INDEXING.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        let _ = run_indexing(full);
        INDEXING.store(false, Ordering::Relaxed);
    });
    format!(
        "Фоновая индексация запущена ({}). Прогресс — через index_status. Остановка — \
         инструментом stop_indexing.",
        if full {
            "полная"
        } else {
            "инкрементальная"
        }
    )
}

/// Инструмент `stop_indexing` (создать `index.stop`).
pub fn stop_indexing() -> String {
    let stop = project_root().join("index.stop");
    let _ = std::fs::write(&stop, b"");
    "Сигнал остановки отправлен. Индексатор завершит текущий файл и остановится; \
     проверьте завершение через index_status."
        .to_string()
}

/// Инструмент `reindex_path` (переиндексация файла/папки).
pub fn reindex_path(path: &str) -> String {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => return format!("Ошибка конфигурации: {}", e.message()),
    };
    let conn = match connect(&cfg) {
        Ok(c) => c,
        Err(e) => return format!("База данных недоступна: {e}"),
    };
    let emb = Embedder::from_config(&cfg);
    let root = project_root();
    let side = match sidecar(&root) {
        Ok(s) => s,
        Err(e) => return format!("Воркер извлечения/лемматизации: {e}"),
    };
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| Path::new(path).to_path_buf());
    let media = MediaRouter::new(&side, &cfg);
    let args = RunIndexArgs {
        single_paths: Some(vec![abs.clone()]),
        full: true,
        prune: false,
        quiet: true,
        ..RunIndexArgs::default()
    };
    let res = hds_index::pipeline::run_index(&conn, &cfg, &emb, &media, &side, &args);
    side.shutdown();
    match res {
        Ok(counters) => format!("Переиндексация {}: {}", abs.display(), counters),
        Err(e) => format!("Ошибка переиндексации {}: {}", abs.display(), e.message()),
    }
}

/// Фоновый прогон `run_index` (для `start_indexing`).
fn run_indexing(full: bool) -> Result<(), String> {
    let cfg = load().map_err(|e| e.message())?;
    let conn = connect(&cfg)?;
    let emb = Embedder::from_config(&cfg);
    let root = project_root();
    let side = sidecar(&root)?;
    let stop = root.join("index.stop");
    if stop.exists() {
        let _ = std::fs::remove_file(&stop); // leftover от прошлой остановки
    }
    let media = MediaRouter::new(&side, &cfg);
    let args = RunIndexArgs {
        full,
        prune: true,
        quiet: true,
        ..RunIndexArgs::default()
    };
    let r = hds_index::pipeline::run_index(&conn, &cfg, &emb, &media, &side, &args);
    side.shutdown();
    r.map(|_| ()).map_err(|e| e.message())
}
