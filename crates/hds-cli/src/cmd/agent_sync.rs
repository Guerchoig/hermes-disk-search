//! `hds agent sync` — демон двустороннего зеркала папки проекта и
//! `<P>\workspace` (фаза A4, TR-7.1…7.9).
//!
//! **A4.3:** к прямому пути добавлен обратный — маркер
//! `hds-agent/<P>/<id>` сверяется с реестром, новый файл агента выпускается в
//! папку проекта, изменённый образ собирается в `.hds-agent\pending` со
//! снимком основания; защита от петель — `issued`/`seen` (TR-7.5/7.7).
//! **A4.4:** решения по вариантам — отдельная команда `hds agent result`
//! (`crates/hds-cli/src/cmd/agent_result.rs` + `hds_agent::results`); валидатор —
//! A4.5.
//!
//! **A4.6:** режим демона — `--interval SEC` (дефолт `agent.sync.interval_sec`,
//! 5 с); проход — тот же `Mirror::sync_once`, сон интервала шагами не больше
//! 500 мс, остановка по `<root>\index.stop` (`hds stop`) и Ctrl+C; lock демона —
//! `sync.lock` в `<P>\agent-state` (свой файл, боевой `watch.lock` не мешает);
//! журнал — `<P>\agent-state\sync.log` (`start`/`pass`/`problem`/`error`/`stop`,
//! только пути, метки, статусы и числа — I8).
//!
//! Порядок именно такой: реестр — то, что переживает перезапуск и потерю
//! папки, и без него остальные шаги нечем проверять.
//!
//! Ошибка прохода **не** валит демон: она станет строкой `error` в журнале, и
//! цикл продолжится (TR-7.2). Разовый `--once` по-прежнему честно возвращает
//! код 1 — совместимость A4.1…A4.5.
//!
//! Порядок остановки демона: цикл → `drop` воркеров (Python завершается сам) →
//! `sync.lock` → снять `index.stop` → строка `stop` в журнале. Ctrl+C — смерть
//! процесса: `sync.lock` остаётся и снимается следующим стартом как устаревший
//! (`hds_index::watch::lock_is_stale`), строка `stop` не пишется — журнал
//! сохраняет всё до последнего начатого прохода.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_agent::mirror::{LazyAnonymizer, LazyExtractor, Mirror, MirrorError, SyncReport};
use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::{projects, settings};
use hds_core::config::Config;
use hds_core::timefmt::{fmt_local_datetime, now_epoch};
use hds_index::watch::WatchLock;

/// Размер журнала, после которого он уезжает в `sync.log.1` (A4.6).
pub const LOG_ROTATE_BYTES: u64 = 1024 * 1024;
/// Шаг сна в цикле демона: `index.stop` замечается не позже этого (A4.6).
pub const SLEEP_STEP: Duration = Duration::from_millis(500);
/// Сколько проблем прохода попадает в журнал (остальные — только числом).
pub const LOG_PROBLEMS: usize = 10;

/// Точка входа `hds agent sync ...`.
pub fn cmd_sync(cfg: &Config, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return 0;
    }
    let parsed = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("agent sync: {message}");
            eprintln!("{}", usage());
            return 2;
        }
    };
    let id = parsed.project.clone();
    let Some(db_path) = settings::registry_db(cfg, &id) else {
        eprintln!("agent sync: '{id}' не похож на идентификатор проекта (P + 4 цифры, D21)");
        return 2;
    };
    let project_path = match projects::get(cfg, &id) {
        Some(project) => {
            println!("[i] проект {} — папка {}", project.id, project.path);
            PathBuf::from(project.path)
        }
        None => {
            eprintln!("agent sync: проект {id} не зарегистрирован (hds agent project add)");
            return 2;
        }
    };
    if !settings::settings(cfg).enabled {
        eprintln!("agent sync: анонимная зона выключена (agent.enabled: false)");
        return 1;
    }
    if !project_path.is_dir() {
        eprintln!(
            "agent sync: папка проекта не найдена: {}",
            project_path.display()
        );
        return 1;
    }

    let sync = settings::sync_settings(cfg);
    let interval_sec = parsed.interval.unwrap_or(sync.interval_sec);
    let Some(state_dir) = settings::state_dir_for(cfg, &id) else {
        eprintln!("agent sync: не задан agent.root — нет каталога agent-state для журнала");
        return 1;
    };
    // Каталог нужен до lock: `WatchLock` занимает файл через `create_new`, а
    // на отсутствующем каталоге это ошибка, которую он трактует как «занято».
    if let Err(e) = std::fs::create_dir_all(&state_dir) {
        eprintln!("agent sync: не создать {}: {e}", state_dir.display());
        return 1;
    }
    // Lock демона — свой файл: боевой `watch.lock` индексатора не мешает, а
    // второй демон зеркала по тому же проекту не нужен. Занят — «уже запущен»
    // и код 0, как у `hds watch` (A4.6).
    let lock = if parsed.daemon() {
        match WatchLock::acquire_named(&state_dir, "sync.lock", 3) {
            Ok(Some(lock)) => Some(lock),
            Ok(None) => {
                println!("[i] демон зеркала уже запущен (sync.lock) — выхожу");
                return 0;
            }
            Err(e) => {
                eprintln!("agent sync: не занять sync.lock: {}", e.message());
                return 1;
            }
        }
    } else {
        None
    };
    let log_path = state_dir.join("sync.log");
    let mut log = match SyncLog::open(&log_path) {
        Ok(log) => log,
        Err(e) => {
            eprintln!("agent sync: журнал {}: {e}", log_path.display());
            return 1;
        }
    };

    let registry = match ArtifactRegistry::open(&db_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("agent sync: {e}");
            return 1;
        }
    };

    let (count, pending) = match (registry.count(), registry.list_pending()) {
        (Ok(count), Ok(pending)) => (count, pending.len()),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("agent sync: {e}");
            return 1;
        }
    };
    println!("[ok] реестр: {}", registry.path().display());
    println!("[ok] записей: {count}, ждут решения: {pending}");
    println!("[ok] журнал: {}", log.path().display());

    let root = hds_core::config::project_root();
    let workspace = settings::workspace_dir(cfg, &id).unwrap_or_default();
    let mut anonymizer = LazyAnonymizer::new(anonymizer_factory(cfg, &root));
    let mut extractor = LazyExtractor::new(extractor_factory(&root));
    let mut mirror = match Mirror::from_config(
        cfg,
        &id,
        &project_path,
        &registry,
        &mut anonymizer,
        &mut extractor,
    ) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("agent sync: {e}");
            return 1;
        }
    };

    // Один экземпляр зеркала и воркеров на все проходы: поднимать Python и
    // перечитывать настройки каждые пять секунд незачем (A4.6).
    let stop_file = root.join("index.stop");
    let (passes, changes, reason, code) = if parsed.daemon() {
        let interval = Duration::from_secs(interval_sec);
        log.start("daemon", &id, &project_path, &workspace, Some(interval));
        println!(
            "[..] демон зеркала: интервал {interval_sec} с; остановка — `hds stop` \
             (index.stop) или Ctrl+C"
        );
        let summary = run_loop(
            interval,
            None,
            || stop_file.exists(),
            || match mirror.sync_once() {
                Ok(report) => PassResult::Report(report),
                Err(e) => PassResult::Failed(e.to_string()),
            },
            &mut log,
        );
        (
            summary.passes,
            summary.changes,
            summary.reason,
            // Демон не умирает от ошибок прохода: они уже в журнале.
            0,
        )
    } else {
        log.start("once", &id, &project_path, &workspace, None);
        match mirror.sync_once() {
            Ok(report) => {
                log.pass(&report);
                log.problems(&report);
                print_report(&report);
                // Совместимость A4.1…A4.5: ошибки отдельных файлов — код 1.
                let code = if report.errors > 0 { 1 } else { 0 };
                (1, report.changes(), StopReason::Once, code)
            }
            Err(e) => {
                log.error(&e.to_string());
                eprintln!("agent sync: {e}");
                (0, 0, StopReason::Error, 1)
            }
        }
    };

    // Порядок остановки (A4.6): цикл → воркеры (shutdown Python) → lock →
    // index.stop → строка stop. Воркеры бросаем явно, до снятия lock: иначе
    // следующий демон поднимется, пока Python ещё пишет во временные файлы.
    drop(mirror);
    drop(anonymizer);
    drop(extractor);
    drop(lock);
    if reason == StopReason::StopFile && stop_file.exists() {
        // Сигнал отработан — иначе следующий старт принял бы его за новый.
        let _ = std::fs::remove_file(&stop_file);
    }
    log.stop(reason.as_str(), passes, changes);
    println!(
        "[ok] остановлено: проходов {passes}, изменений {changes}, причина {}; журнал {}",
        reason.as_str(),
        log.path().display()
    );
    code
}

/// Ленивое поднятие воркера анонимизации (Python поднимается только при
/// первом файле, которому нужен образ).
///
/// `pub(crate)`: тем же способом поднимает воркер `hds agent result` —
/// пересоздание образа после решения идёт по тому же пути (A4.4).
pub(crate) fn anonymizer_factory(
    cfg: &Config,
    root: &Path,
) -> impl FnMut() -> Result<hds_anonymize::Client, MirrorError> {
    let root = root.to_path_buf();
    let cfg = cfg.clone();
    move || {
        let python = hds_anonymize::discover_python(&root).ok_or_else(|| {
            MirrorError::Config(
                "не найден интерпретатор воркера анонимизации \
                 (HDS_ANONYMIZE_PYTHON / sidecar\\venv-anonymize / sidecar\\python)"
                    .to_string(),
            )
        })?;
        let anon = hds_anonymize::Settings::from_config(&cfg);
        let mut client_cfg = hds_anonymize::ClientConfig::new(&python, &root);
        client_cfg.idle_timeout = Duration::from_secs(anon.idle_timeout_sec.max(1));
        client_cfg.request_timeout = Duration::from_secs(anon.request_timeout_sec.max(60));
        client_cfg.extra_args = anon.worker_args(&root);
        if let Some(script) = std::env::var_os("HDS_ANONYMIZE_WORKER") {
            if !script.is_empty() {
                client_cfg.script = Some(PathBuf::from(script));
            }
        }
        hds_anonymize::Client::spawn(client_cfg).map_err(|e| MirrorError::Anonymizer(e.to_string()))
    }
}

/// Ленивое поднятие воркера извлечения (нужен только pdf/pptx/mpp).
///
/// `HDS_EXTRACT_WORKER` подменяет скрипт воркера (диагностика и тесты, как
/// `HDS_ANONYMIZE_WORKER`): при `HDS_ROOT`, указывающем на каталог данных,
/// штатный `root/sidecar/...` не находится.
pub(crate) fn extractor_factory(
    root: &Path,
) -> impl FnMut() -> Result<hds_extract::Worker, MirrorError> {
    let root = root.to_path_buf();
    move || {
        let python = hds_extract::discover_python(&root).ok_or_else(|| {
            MirrorError::Config(
                "не найден интерпретатор воркера извлечения \
                 (HDS_EXTRACT_PYTHON / sidecar\\python / .venv)"
                    .to_string(),
            )
        })?;
        let mut worker_cfg = hds_extract::WorkerConfig::new(&python, &root);
        worker_cfg.idle_timeout = Duration::from_secs(600);
        if let Some(script) = std::env::var_os("HDS_EXTRACT_WORKER") {
            if !script.is_empty() {
                worker_cfg.script = PathBuf::from(script);
            }
        }
        hds_extract::Worker::spawn(worker_cfg).map_err(|e| MirrorError::Extractor(e.to_string()))
    }
}

/// Почему кончился цикл демона (и чем закончился разовый проход).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Отработано заданное число проходов (`max_passes` в тестах).
    MaxPasses,
    /// `<root>\index.stop` — `hds stop` (Ctrl+C просто завершает процесс).
    StopFile,
    /// Разовый проход `--once` закончен.
    Once,
    /// Разовый проход не удался (код 1, как в A4.1…A4.5).
    Error,
}

impl StopReason {
    /// Метка для строки `stop` в журнале.
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::MaxPasses => "max_passes",
            StopReason::StopFile => "index.stop",
            StopReason::Once => "once",
            StopReason::Error => "error",
        }
    }
}

/// Результат одного прохода для цикла: отчёт или текст ошибки (без значений).
pub enum PassResult {
    /// Проход удался — счётчики [`SyncReport`].
    Report(SyncReport),
    /// Проход не удался; текст пишется в журнал строкой `error`.
    Failed(String),
}

/// Итог цикла: сколько проходов, сколько изменений и почему остановились.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopSummary {
    pub passes: usize,
    pub changes: usize,
    pub reason: StopReason,
}

/// Цикл демона: проверка остановки → проход → сон интервала шагами ≤ 500 мс.
///
/// Порядок «проверка → проход → сон» выбран намеренно: `index.stop`,
/// созданный до старта, останавливает демон **до** первого прохода (сигнал
/// уже отработан — второй раз обходить папку незачем), а после прохода
/// проверка повторяется на каждом шаге сна, поэтому `hds stop` замечается не
/// позже [`SLEEP_STEP`], а не «через интервал».
///
/// Ошибка прохода (`PassResult::Failed`) демон не убивает: строка `error` в
/// журнал — и следующий проход. Это прямое требование TR-7.2.
pub fn run_loop<F, P>(
    interval: Duration,
    max_passes: Option<usize>,
    should_stop: F,
    mut one_pass: P,
    log: &mut SyncLog,
) -> LoopSummary
where
    F: Fn() -> bool,
    P: FnMut() -> PassResult,
{
    let mut passes = 0usize;
    let mut changes = 0usize;
    let reason;
    loop {
        if should_stop() {
            reason = StopReason::StopFile;
            break;
        }
        match one_pass() {
            PassResult::Report(report) => {
                log.pass(&report);
                log.problems(&report);
                print_report(&report);
                changes += report.changes();
            }
            PassResult::Failed(message) => {
                log.error(&message);
                eprintln!("[!] проход не удался (демон продолжает): {message}");
            }
        }
        passes += 1;
        if let Some(max) = max_passes {
            if passes >= max {
                reason = StopReason::MaxPasses;
                break;
            }
        }
        if !sleep_interval(interval, &should_stop) {
            reason = StopReason::StopFile;
            break;
        }
    }
    LoopSummary {
        passes,
        changes,
        reason,
    }
}

/// Сон интервала шагами не больше [`SLEEP_STEP`]; `false` — пришёл сигнал.
fn sleep_interval(interval: Duration, should_stop: &impl Fn() -> bool) -> bool {
    let mut left = interval;
    while !left.is_zero() {
        if should_stop() {
            return false;
        }
        let step = left.min(SLEEP_STEP);
        std::thread::sleep(step);
        left -= step;
    }
    true
}

/// Журнал зеркала: `<agent.root>\P<NNNN>\agent-state\sync.log` (A4.6, I8).
///
/// Строки: `start` (режим, проект, интервал, папки), `pass` (все счётчики
/// [`SyncReport`] + `changes` + `problems`), `problem` (не больше
/// [`LOG_PROBLEMS`] штук за проход), `error` (проход не удался), `stop`
/// (причина, проходы, изменения). Содержимого документов и значений из базы
/// меток здесь нет — только пути, метки, статусы и числа.
///
/// Открывается и в режиме `--once`: журнал нужен и разовому прогону, иначе
/// «что было вчера» не восстановить. Пишется без буфера (`File`), поэтому
/// строки видны сразу — журнал читают во время работы демона.
pub struct SyncLog {
    path: PathBuf,
    file: File,
}

impl SyncLog {
    /// Открыть журнал на дозапись; больше [`LOG_ROTATE_BYTES`] — в `sync.log.1`.
    ///
    /// Ротация только при старте: демон работает неделями, а «переименовать
    /// файл под своим же открытым дескриптором» на Windows нельзя.
    pub fn open(path: &Path) -> io::Result<SyncLog> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let too_big = std::fs::metadata(path)
            .map(|m| m.len() > LOG_ROTATE_BYTES)
            .unwrap_or(false);
        if too_big {
            let rotated = PathBuf::from(format!("{}.1", path.display()));
            let _ = std::fs::remove_file(&rotated);
            let _ = std::fs::rename(path, &rotated);
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(SyncLog {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Путь журнала (печать «журнал: …» и тесты).
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn line(&mut self, text: &str) {
        // Переводы строк в тексте ошибки разорвали бы формат: одна запись —
        // одна строка (журнал читают глазами и `Select-String`).
        let text = text.replace(['\n', '\r'], " ");
        let _ = writeln!(self.file, "{} {text}", fmt_local_datetime(now_epoch()));
    }

    /// `start`: режим (`daemon`/`once`), проект, интервал, папка и workspace.
    pub fn start(
        &mut self,
        mode: &str,
        project: &str,
        project_root: &Path,
        workspace: &Path,
        interval: Option<Duration>,
    ) {
        let interval = interval
            .map(|d| format!(" interval={}s", d.as_secs()))
            .unwrap_or_default();
        self.line(&format!(
            "start mode={mode} project={project}{interval} source={} workspace={}",
            project_root.display(),
            workspace.display()
        ));
    }

    /// `pass`: все счётчики прохода, `changes` и число проблем.
    pub fn pass(&mut self, report: &SyncReport) {
        self.line(&format!(
            "pass scanned={} created={} updated={} unchanged={} unsupported={} too_big={} \
             deferred={} removed={} renamed={} orphans={} agent_changed={} conflicts={} \
             errors={} emitted={} variants={} changes={} problems={}",
            report.scanned,
            report.created,
            report.updated,
            report.unchanged,
            report.unsupported,
            report.too_big,
            report.deferred,
            report.removed,
            report.renamed,
            report.orphans,
            report.agent_changed,
            report.conflicts,
            report.errors,
            report.emitted,
            report.variants,
            report.changes(),
            report.problems.len()
        ));
    }

    /// `problem`: первые [`LOG_PROBLEMS`] проблем прохода (остальные — числом).
    pub fn problems(&mut self, report: &SyncReport) {
        for problem in report.problems.iter().take(LOG_PROBLEMS) {
            self.line(&format!("problem {problem}"));
        }
        let rest = report.problems.len().saturating_sub(LOG_PROBLEMS);
        if rest > 0 {
            self.line(&format!(
                "problem … ещё {rest} (в журнал пишутся первые {LOG_PROBLEMS})"
            ));
        }
    }

    /// `error`: проход целиком не удался (в режиме демона это не смертельно).
    pub fn error(&mut self, message: &str) {
        self.line(&format!("error {message}"));
    }

    /// `stop`: причина, число проходов и суммарные изменения.
    pub fn stop(&mut self, reason: &str, passes: usize, changes: usize) {
        self.line(&format!(
            "stop reason={reason} passes={passes} changes={changes}"
        ));
    }
}

/// Сводка прохода — счётчики и до 10 проблем (пути и причины, без значений).
fn print_report(report: &SyncReport) {
    println!(
        "[ok] обход: {} файл(ов); новых образов: {}, обновлено: {}, без изменений: {}",
        report.scanned, report.created, report.updated, report.unchanged
    );
    println!(
        "[ok] unsupported: {}, too_big: {}, отложено: {}, удалено: {}, переименовано: {}",
        report.unsupported, report.too_big, report.deferred, report.removed, report.renamed
    );
    if report.orphans + report.agent_changed + report.conflicts > 0 {
        println!(
            "[i] orphan: {}, изменено агентом: {}, конфликтов: {}",
            report.orphans, report.agent_changed, report.conflicts
        );
    }
    if report.emitted + report.variants > 0 {
        println!(
            "[ok] обратный путь: файлов агента в проект: {}, вариантов в pending: {}",
            report.emitted, report.variants
        );
    }
    for problem in report.problems.iter().take(10) {
        eprintln!("[!] {problem}");
    }
    if report.problems.len() > 10 {
        eprintln!("[!] … ещё {} проблем(ы)", report.problems.len() - 10);
    }
    println!(
        "[i] изменений за проход: {} (повторный прогон по неизменной папке — 0)",
        report.changes()
    );
}

/// Разобранные аргументы `hds agent sync` (A4.6: `--once | --interval SEC`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncArgs {
    /// Идентификатор проекта (`P` + 4 цифры, D21).
    pub project: String,
    /// `--once` — один проход (тот же режим, что и без флагов).
    pub once: bool,
    /// `--interval SEC` — демон; `None` — разовый проход.
    pub interval: Option<u64>,
}

impl SyncArgs {
    /// Демон ли это (задан `--interval`).
    pub fn daemon(&self) -> bool {
        self.interval.is_some()
    }
}

/// Разбор аргументов: `--project <P>`/`--project=<P>`, `--once`,
/// `--interval SEC`/`--interval=SEC`.
///
/// Отказы (код 2) — те же, что были в A4.1, плюс правила A4.6: `--interval`
/// без значения, `0`, отрицательное или не число, и `--once` вместе с
/// `--interval` (один проход и демон — разные режимы, угадывать нечего).
pub fn parse_args(args: &[String]) -> Result<SyncArgs, String> {
    let mut project: Option<String> = None;
    let mut once = false;
    let mut interval: Option<u64> = None;
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if arg == "--once" {
            once = true;
            continue;
        }
        if arg == "--project" {
            let Some(value) = args.get(i) else {
                return Err("--project: нужно имя проекта".to_string());
            };
            project = Some(value.clone());
            i += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--project=") {
            project = Some(value.to_string());
            continue;
        }
        if arg == "--interval" {
            let Some(value) = args.get(i) else {
                return Err("--interval: нужно число секунд".to_string());
            };
            interval = Some(parse_interval(value)?);
            i += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--interval=") {
            interval = Some(parse_interval(value)?);
            continue;
        }
        return Err(format!("неизвестный аргумент '{arg}'"));
    }
    if once && interval.is_some() {
        return Err("--once и --interval вместе: выберите один режим".to_string());
    }
    let Some(project) = project else {
        return Err("нужен --project <P0001>".to_string());
    };
    Ok(SyncArgs {
        project,
        once,
        interval,
    })
}

/// `--interval SEC`: только положительное целое (0 — busy-loop, минус — смысла нет).
fn parse_interval(raw: &str) -> Result<u64, String> {
    match raw.trim().parse::<i64>() {
        Ok(value) if value > 0 => Ok(value as u64),
        Ok(_) => Err(format!(
            "--interval {raw}: нужно положительное число секунд"
        )),
        Err(_) => Err(format!("--interval {raw}: нужно число секунд")),
    }
}

fn usage() -> String {
    "использование: hds agent sync --project <P> [--once | --interval SEC]\n\
     \x20 (один проход: обход папки проекта, wait_stable, образы docx/xlsx/txt/md/xml\n\
     \x20  и текстовые образы pdf/pptx/mpp, unsupported/too_big, удаление и\n\
     \x20  переименование, обратный путь: agent_new → папка проекта, agent_changed →\n\
     \x20  .hds-agent\\pending; TR-7.1…7.7)\n\
     \x20 без флагов и с --once — один проход (совместимость);\n\
     \x20 --interval SEC — демон: проход, сон SEC (дефолт agent.sync.interval_sec = 5),\n\
     \x20  остановка — `hds stop` (index.stop) или Ctrl+C; lock — <P>\\agent-state\\sync.lock;\n\
     \x20  журнал — <agent.root>\\P<NNNN>\\agent-state\\sync.log (без содержимого, I8);\n\
     \x20 путь реестра: <state_dir>\\data\\agent\\registry\\<P>.db, TR-7.2)"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Временный каталог (счётчик — §7 п.4 хендоффа: тесты идут параллельно
    /// в одном процессе). Уборка — `cleanup`, а не `Drop`: тестам нужен путь.
    pub(super) fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hds-synclog-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    pub(super) fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    pub(super) fn report(created: usize) -> SyncReport {
        SyncReport {
            scanned: 1,
            created,
            ..Default::default()
        }
    }

    #[test]
    fn arguments_follow_the_contract() {
        fn args(items: &[&str]) -> Vec<String> {
            items.iter().map(|s| s.to_string()).collect()
        }
        let daemon = parse_args(&args(&["--project", "P0001", "--interval", "7"])).unwrap();
        assert_eq!(daemon.project, "P0001");
        assert!(daemon.daemon());
        assert_eq!(daemon.interval, Some(7));
        assert!(parse_args(&args(&["--project", "P0001", "--interval=3"]))
            .unwrap()
            .daemon());
        let once = parse_args(&args(&["--project=P0001", "--once"])).unwrap();
        assert!(once.once && !once.daemon());
        for bad in [
            vec!["--project", "P0001", "--interval", "0"],
            vec!["--project", "P0001", "--interval", "-5"],
            vec!["--project", "P0001", "--interval", "abc"],
            vec!["--project", "P0001", "--interval"],
            vec!["--project", "P0001", "--once", "--interval", "5"],
            vec!["--project", "P0001", "--bogus"],
            vec!["--once"],
        ] {
            assert!(
                parse_args(&args(&bad)).is_err(),
                "должен быть отказ: {bad:?}"
            );
        }
    }

    #[test]
    fn journal_rotates_at_one_megabyte() {
        let dir = temp_dir("rotate");
        let path = dir.join("sync.log");
        std::fs::write(&path, vec![b'x'; (LOG_ROTATE_BYTES + 1) as usize]).unwrap();
        drop(SyncLog::open(&path).unwrap());
        assert!(
            dir.join("sync.log.1").is_file(),
            "прежний журнал уехал в .1"
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        cleanup(&dir);
    }

    #[test]
    fn journal_lines_have_kind_counters_and_no_newlines() {
        let dir = temp_dir("lines");
        let path = dir.join("sync.log");
        let mut log = SyncLog::open(&path).unwrap();
        assert_eq!(log.path(), path.as_path());
        let mut report = report(1);
        report.updated = 2;
        report
            .problems
            .push("Протоколы\\A.docx: отказ\nхвост ошибки".to_string());
        log.start("once", "P0001", &dir, &dir.join("workspace"), None);
        log.pass(&report);
        log.problems(&report);
        log.error("реестр: занят");
        log.stop(StopReason::Once.as_str(), 1, report.changes());
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("start mode=once project=P0001"), "{text}");
        assert!(text.contains("pass scanned=1 "), "{text}");
        assert!(text.contains("changes=3 problems=1"), "{text}");
        assert!(
            text.contains("problem Протоколы\\A.docx: отказ хвост ошибки"),
            "{text}"
        );
        assert!(text.contains("error реестр: занят"), "{text}");
        assert!(
            text.contains("stop reason=once passes=1 changes=3"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 5, "одна запись — одна строка: {text}");
        cleanup(&dir);
    }

    #[test]
    fn journal_limits_problems_to_ten() {
        let dir = temp_dir("problems");
        let path = dir.join("sync.log");
        let mut log = SyncLog::open(&path).unwrap();
        let mut report = report(0);
        for i in 0..13 {
            report.problems.push(format!("файл{i}.bin: отказ"));
        }
        log.problems(&report);
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.lines().filter(|l| l.contains(" problem файл")).count(),
            10
        );
        assert!(text.contains("ещё 3"), "{text}");
        cleanup(&dir);
    }
}
#[cfg(test)]
mod loop_tests {
    use super::tests::{cleanup, report, temp_dir};
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn loop_makes_two_passes_and_writes_stop() {
        let dir = temp_dir("loop");
        let path = dir.join("sync.log");
        let mut log = SyncLog::open(&path).unwrap();
        let mut done = 0usize;
        let summary = run_loop(
            Duration::from_millis(1),
            Some(2),
            || false,
            || {
                done += 1;
                PassResult::Report(report(1))
            },
            &mut log,
        );
        log.stop(summary.reason.as_str(), summary.passes, summary.changes);
        drop(log);
        assert_eq!((summary.passes, summary.changes), (2, 2));
        assert_eq!(summary.reason, StopReason::MaxPasses);
        assert_eq!(done, 2);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.lines().filter(|l| l.contains(" pass ")).count(),
            2,
            "{text}"
        );
        assert!(
            text.contains("stop reason=max_passes passes=2 changes=2"),
            "{text}"
        );
        cleanup(&dir);
    }

    #[test]
    fn loop_stops_on_signal_without_waiting_interval() {
        let dir = temp_dir("signal");
        let path = dir.join("sync.log");
        let mut log = SyncLog::open(&path).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let checked = Arc::clone(&stop);
        let raised = Arc::clone(&stop);
        let started = std::time::Instant::now();
        let summary = run_loop(
            Duration::from_secs(60),
            None,
            move || checked.load(Ordering::Relaxed),
            move || {
                raised.store(true, Ordering::Relaxed);
                PassResult::Report(report(1))
            },
            &mut log,
        );
        assert_eq!(summary.passes, 1);
        assert_eq!(summary.reason, StopReason::StopFile);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "сон прерван сигналом, а не выждал интервал"
        );
        cleanup(&dir);
    }

    #[test]
    fn loop_survives_failed_pass() {
        let dir = temp_dir("failed");
        let path = dir.join("sync.log");
        let mut log = SyncLog::open(&path).unwrap();
        let summary = run_loop(
            Duration::from_millis(1),
            Some(2),
            || false,
            || PassResult::Failed("обход: отказано в доступе".to_string()),
            &mut log,
        );
        assert_eq!(summary.passes, 2, "демон не умирает от ошибки прохода");
        assert_eq!(summary.changes, 0);
        assert_eq!(summary.reason, StopReason::MaxPasses);
        drop(log);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.lines().filter(|l| l.contains(" error ")).count(),
            2,
            "{text}"
        );
        cleanup(&dir);
    }
}
