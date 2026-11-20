//! `hds agent sync` — демон двустороннего зеркала папки проекта и
//! `<P>\workspace` (фаза A4, TR-7.1…7.9).
//!
//! **A4.3:** к прямому пути добавлен обратный — маркер
//! `hds-agent/<P>/<id>` сверяется с реестром, новый файл агента выпускается в
//! папку проекта, изменённый образ собирается в `.hds-agent\pending` со
//! снимком основания; защита от петель — `issued`/`seen` (TR-7.5/7.7).
//! **A4.4:** решения по вариантам — отдельная команда `hds agent result`
//! (`crates/hds-cli/src/cmd/agent_result.rs` + `hds_agent::results`); валидатор —
//! A4.5, циклический режим (`--interval`) — A4.6.
//!
//! Порядок именно такой: реестр — то, что переживает перезапуск и потерю
//! папки, и без него остальные шаги нечем проверять.
//!
//! Ошибка реестра **не** валит будущий демон: в цикле она станет статусом
//! `error` и записью в журнал (TR-7.2). CLI же честно возвращает код 1.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_agent::mirror::{LazyAnonymizer, LazyExtractor, Mirror, MirrorError, SyncReport};
use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::{projects, settings};
use hds_core::config::Config;

/// Точка входа `hds agent sync ...`.
pub fn cmd_sync(cfg: &Config, args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", usage());
        return 0;
    }
    let Some(id) = crate::cmd::agent::flag_val(args, "--project") else {
        eprintln!("agent sync: нужен --project <P0001>");
        eprintln!("{}", usage());
        return 2;
    };
    if let Some(other) = unknown_flag(args) {
        eprintln!("agent sync: неизвестный аргумент '{other}'");
        eprintln!("{}", usage());
        return 2;
    }
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

    let root = hds_core::config::project_root();
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
    let report = match mirror.sync_once() {
        Ok(report) => report,
        Err(e) => {
            eprintln!("agent sync: {e}");
            return 1;
        }
    };
    print_report(&report);
    if report.errors > 0 {
        return 1;
    }
    0
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

/// Первый аргумент, который не является флагом, известным подкоманде.
fn unknown_flag(args: &[String]) -> Option<&str> {
    let mut skip_value = false;
    for arg in args {
        if skip_value {
            skip_value = false;
            continue;
        }
        match arg.as_str() {
            "--project" => skip_value = true,
            "--once" => {}
            other if other.starts_with("--project=") => {}
            other => return Some(other),
        }
    }
    None
}

fn usage() -> String {
    "использование: hds agent sync --project <P> [--once]\n\
     \x20 (A4.2: один проход прямого пути — обход папки проекта, wait_stable,\n\
     \x20  образы docx/xlsx/txt/md/xml и текстовые образы pdf/pptx/mpp,\n\
     \x20  unsupported/too_big, удаление и переименование. Обратный путь — A4.3,\n\
     \x20  --interval и цикл — A4.6; путь реестра:\n\
     \x20  <state_dir>\\data\\agent\\registry\\<P>.db, TR-7.2)"
        .to_string()
}
