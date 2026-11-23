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
//! `sync.lock` → строка `stop` в журнале. Файл `index.stop` **не снимается**
//! (AG-27): он общий с `hds watch`, и второй адресат сигнала должен его увидеть;
//! устаревший файл (старше старта демона) сигналом не считается. Ctrl+C — смерть
//! процесса: `sync.lock` остаётся и снимается следующим стартом как устаревший
//! (`hds_index::watch::lock_is_stale`), строка `stop` не пишется — журнал
//! сохраняет всё до последнего начатого прохода.
//!
//! **A6.2:** `--all` — все проекты `projects.json` в одном процессе (В-A6-2).
//! У каждого проекта свой `sync.lock`, свой журнал `<P>\agent-state\sync.log`,
//! свой реестр и свои воркеры: несколько демонов по разным проектам не мешают
//! друг другу. Недоступная папка — строка `problem` в журнале проекта и
//! пропуск, остальные проекты обрабатываются. `--all` без `--once` — демон
//! (задача автозапуска запускает именно эту команду): интервал берётся из
//! `agent.sync.interval_sec`, `--interval SEC` перекрывает.
//!
//! **A6.3:** между проходами демон сверяет `mtime` и размер `config.yaml`
//! ([`LiveConfig`]) и применяет `agent.sync.*` (новый интервал — со следующего
//! сна) и `index.max_file_mb` на лету. `agent.enabled: false` останавливает
//! демон с причиной `disabled`, смена `agent.root` — предупреждение `[!]` и
//! работа на прежнем корне. Каждое применение — строка `config` в журнале (I8).
//!
//! **AG6-02:** проход прерывается **между файлами**: `Mirror::sync_once_cancellable`
//! получает `stop.triggered()`, текущий файл дорабатывается целиком (недописанный
//! образ хуже задержки), `wait_stable` не прерывается — верхняя граница задержки
//! остановки равна обработке одного файла. После сигнала `run_loop` пишет `pass`
//! и выходит с `stop reason=index.stop`: `sync.lock` снимается, `index.stop`
//! остаётся (AG-27).
//!
//! **AG6-15(а):** демон `--all` — супервизор (`Supervisor`): раз в
//! [`SUPERVISOR_TICK`] перечитывает `projects.json`, добирает новые проекты,
//! перезапускает поток при смене папки и останавливает исчезнувшие — своим
//! флагом на проект, не трогая остальные. Занятый `sync.lock` и отказ запуска —
//! кулдаун [`BUSY_RETRY`]. Остановка супервизором пишется как
//! `stop reason=restart` (это не `hds stop`, общий файл не трогается).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use hds_agent::mirror::{LazyAnonymizer, LazyExtractor, Mirror, SyncReport};
use hds_agent::registry_db::ArtifactRegistry;
use hds_agent::{projects, settings};
use hds_core::config::{self, dig, Config};
use hds_core::timefmt::{fmt_local_datetime, now_epoch};
use hds_index::watch::WatchLock;

/// Размер журнала, после которого он уезжает в `sync.log.1` (A4.6).
pub const LOG_ROTATE_BYTES: u64 = 1024 * 1024;
/// Шаг сна в цикле демона: `index.stop` замечается не позже этого (A4.6).
pub const SLEEP_STEP: Duration = Duration::from_millis(500);
/// Сколько проблем прохода попадает в журнал (остальные — только числом).
pub const LOG_PROBLEMS: usize = 10;
/// Как часто супервизор `--all` перечитывает `projects.json` (AG6-15(а)).
///
/// Меньше интервала прохода смысла нет: новый проект подхватывается между
/// тиками, а ручная правка файла — редкое событие.
pub const SUPERVISOR_TICK: Duration = Duration::from_secs(5);
/// Кулдаун проекта, который не удалось вести: занятый `sync.lock` (его держит
/// другой демон) или отказ запуска (AG6-15(а)). Без него супервизор поднимал бы
/// поток каждые [`SUPERVISOR_TICK`] и засорял журнал.
pub const BUSY_RETRY: Duration = Duration::from_secs(60);

/// Сигнал остановки `<root>\index.stop` (общий файл с `hds watch`, AG-27).
///
/// `hds stop` пишет его для **всех** демонов сразу. Демон зеркала не должен:
/// * удалять файл — иначе `hds watch` (второй адресат) сигнала уже не увидит;
/// * считать сигналом файл, который лежал на диске до его старта — иначе
///   демон, запущенный после неудачного `hds stop` (индексатор при этом не
///   работал и файл не снял), останавливался бы сразу после старта.
///
/// Поэтому сигнал — это файл, изменённый **после** запуска демона.
#[derive(Debug, Clone)]
pub struct StopSignal {
    path: PathBuf,
    since: SystemTime,
}

impl StopSignal {
    /// Запомнить момент старта: файл, появившийся раньше, — устаревший.
    pub fn new(path: PathBuf, since: SystemTime) -> StopSignal {
        StopSignal { path, since }
    }

    /// Путь сигнала (для тестов и сообщений).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Пришёл ли сигнал после старта демона.
    pub fn triggered(&self) -> bool {
        std::fs::metadata(&self.path)
            .and_then(|meta| meta.modified())
            .map(|mtime| mtime >= self.since)
            .unwrap_or(false)
    }
}

/// Снимок `config.yaml` для горячего перечитывания (A6.3).
///
/// Демон сверяет `mtime` и размер файла между проходами; если они не
/// изменились, файл не читается вовсе. Ошибка разбора не валит демон: строка
/// `error` в журнал — и работа на прежних настройках.
#[derive(Debug)]
pub struct LiveConfig {
    path: PathBuf,
    stamp: Option<(SystemTime, u64)>,
}

impl LiveConfig {
    /// Запомнить текущее состояние файла (путь — [`config::config_path`]).
    pub fn new(path: PathBuf) -> LiveConfig {
        let stamp = fingerprint(&path);
        LiveConfig { path, stamp }
    }

    /// Перечитать, если файл изменился. `None` — изменений нет.
    ///
    /// Пустой (или не-словарь) файл — ошибка, а не «агент выключен»: так
    /// выглядит недописанный `config.yaml` (`fs::write` сначала усекает файл),
    /// и демон не должен из-за этого останавливаться.
    pub fn reload(&mut self) -> Option<Result<Config, String>> {
        let now = fingerprint(&self.path);
        if now == self.stamp {
            return None;
        }
        self.stamp = now;
        let path = self.path.display().to_string();
        Some(
            config::load_from(&self.path)
                .map_err(|e| format!("{path}: {e}"))
                .and_then(|fresh| {
                    if fresh.is_mapping() {
                        Ok(fresh)
                    } else {
                        Err(format!("{path}: пустой config.yaml (правка не завершена?)"))
                    }
                }),
        )
    }
}

/// Отпечаток файла: `mtime` и размер; `None` — файла нет.
fn fingerprint(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// `index.max_file_mb` (В-3) — предел размера файла для зеркала.
fn index_max_file_mb(cfg: &Config) -> u64 {
    dig(cfg, "index.max_file_mb")
        .and_then(|v| v.as_u64())
        .unwrap_or(200)
}

/// Точка входа `hds agent sync ...`.
///
/// `--project <P>` — один проект (A4.1…A4.6), `--all` — все проекты
/// `projects.json` в одном процессе (A6.2, В-A6-2).
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
    if parsed.all {
        return sync_all(cfg, &parsed);
    }
    sync_one(cfg, &parsed)
}

/// `--project <P>`: прежний путь A4.1…A4.6 — коды и сообщения не меняются.
fn sync_one(cfg: &Config, parsed: &SyncArgs) -> i32 {
    let id = parsed.project.clone().unwrap_or_default();
    // Не идентификатор проекта — отказ до построения пути (D21).
    if settings::registry_db(cfg, &id).is_none() {
        eprintln!("agent sync: '{id}' не похож на идентификатор проекта (P + 4 цифры, D21)");
        return 2;
    }
    let project = match projects::get(cfg, &id) {
        Some(project) => {
            println!("[i] проект {} — папка {}", project.id, project.path);
            project
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
    let project_path = PathBuf::from(&project.path);
    if !project_path.is_dir() {
        eprintln!(
            "agent sync: папка проекта не найдена: {}",
            project_path.display()
        );
        return 1;
    }
    let sync = settings::sync_settings(cfg);
    let interval_sec = parsed.interval.unwrap_or(sync.interval_sec);
    let root = hds_core::config::project_root();
    let stop = StopSignal::new(root.join("index.stop"), SystemTime::now());
    run_project(
        cfg,
        &project,
        &project_path,
        parsed.daemon(),
        interval_sec,
        &stop,
        None,
    )
    .code
}

/// Что вышло с одним проектом (`--all` продолжает остальные).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProjectOutcome {
    /// Код по проекту (`0/1/2` — как у одиночной команды).
    code: i32,
    /// Проходы/цикл действительно шли (не «уже запущен» и не отказ).
    started: bool,
    /// Почему кончился цикл демона (AG6-15(а): супервизор не перезапускает
    /// проект, у которого `agent.enabled: false`).
    reason: Option<StopReason>,
}

impl ProjectOutcome {
    fn failed(code: i32) -> ProjectOutcome {
        ProjectOutcome {
            code,
            started: false,
            reason: None,
        }
    }

    /// Lock занят: проект ведёт другой демон — это норма, не ошибка (A4.6).
    fn busy() -> ProjectOutcome {
        ProjectOutcome {
            code: 0,
            started: false,
            reason: None,
        }
    }

    fn done(code: i32, reason: Option<StopReason>) -> ProjectOutcome {
        ProjectOutcome {
            code,
            started: true,
            reason,
        }
    }
}

/// Один проект: lock → журнал → реестр → зеркало → проходы → остановка.
///
/// Одна реализация для `--project` и `--all`: иначе `sync.log` и коды двух
/// режимов со временем разъедутся.
///
/// `local_stop` (AG6-15(а)) — свой флаг проекта в супервизоре `--all`:
/// супервизор поднимает его, когда проект исчез из `projects.json` или у него
/// сменился путь, и остальные проекты при этом продолжают работу. У `--project`
/// и `--once` флага нет (`None`) — там достаточно общего `index.stop`.
fn run_project(
    cfg: &Config,
    project: &projects::Project,
    project_path: &Path,
    daemon: bool,
    interval_sec: u64,
    stop: &StopSignal,
    local_stop: Option<&AtomicBool>,
) -> ProjectOutcome {
    let id = project.id.as_str();
    let Some(db_path) = settings::registry_db(cfg, id) else {
        eprintln!("agent sync: '{id}' не похож на идентификатор проекта (P + 4 цифры, D21)");
        return ProjectOutcome::failed(2);
    };
    let Some(state_dir) = settings::state_dir_for(cfg, id) else {
        eprintln!("agent sync: не задан agent.root — нет каталога agent-state для журнала");
        return ProjectOutcome::failed(1);
    };
    // Каталог нужен до lock: `WatchLock` занимает файл через `create_new`, а
    // на отсутствующем каталоге это ошибка, которую он трактует как «занято».
    if let Err(e) = std::fs::create_dir_all(&state_dir) {
        eprintln!("agent sync: не создать {}: {e}", state_dir.display());
        return ProjectOutcome::failed(1);
    }
    // Lock демона — свой файл: боевой `watch.lock` индексатора не мешает, а
    // второй проход по тому же проекту не нужен. Занят — «уже запущен» и код 0,
    // как у `hds watch` (A4.6). С A6.2 lock занимает и `--once` (AG6-06): иначе
    // ручной `--all --once` шёл бы параллельно с автозапуском — два зеркала на
    // один проект, гонка `pending` и образов.
    let lock = match WatchLock::acquire_named(&state_dir, "sync.lock", 3) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            println!("[i] зеркало уже запущено для {id} (sync.lock) — выхожу");
            return ProjectOutcome::busy();
        }
        Err(e) => {
            eprintln!("agent sync: не занять sync.lock: {}", e.message());
            return ProjectOutcome::failed(1);
        }
    };
    let log_path = state_dir.join("sync.log");
    let mut log = match SyncLog::open(&log_path) {
        Ok(log) => log,
        Err(e) => {
            eprintln!("agent sync: журнал {}: {e}", log_path.display());
            return ProjectOutcome::failed(1);
        }
    };

    let registry = match ArtifactRegistry::open(&db_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("agent sync: {e}");
            return ProjectOutcome::failed(1);
        }
    };

    let (count, pending) = match (registry.count(), registry.list_pending()) {
        (Ok(count), Ok(pending)) => (count, pending.len()),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("agent sync: {e}");
            return ProjectOutcome::failed(1);
        }
    };
    println!("[ok] реестр: {}", registry.path().display());
    println!("[ok] записей: {count}, ждут решения: {pending}");
    println!("[ok] журнал: {}", log.path().display());

    let root = hds_core::config::project_root();
    let workspace = settings::workspace_dir(cfg, id).unwrap_or_default();
    let mut anonymizer = LazyAnonymizer::new(anonymizer_factory(cfg, &root));
    let mut extractor = LazyExtractor::new(extractor_factory(&root));
    let mut mirror = match Mirror::from_config(
        cfg,
        id,
        project_path,
        &registry,
        &mut anonymizer,
        &mut extractor,
    ) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("agent sync: {e}");
            return ProjectOutcome::failed(1);
        }
    };

    // Один экземпляр зеркала и воркеров на все проходы: поднимать Python и
    // перечитывать настройки каждые пять секунд незачем (A4.6).
    //
    // AG-27: `index.stop` — общий файл с `hds watch`/`hds stop`. Демон его
    // **не снимает** (иначе второй демон никогда не увидит сигнал) и не
    // считает сигналом файл, который лежал до старта. Сигнал передаётся
    // снаружи: в `--all` он общий для всех проектов.
    //
    // AG6-15(а): кроме общего сигнала у проекта может быть свой флаг —
    // супервизор `--all` снимает один проект, не трогая остальные.
    let stopped_locally = || {
        local_stop
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    };
    let (passes, changes, reason, code) = if daemon {
        let interval = Duration::from_secs(interval_sec);
        log.start("daemon", id, project_path, &workspace, Some(interval));
        println!(
            "[..] демон зеркала: проект {id}, интервал {interval_sec} с; остановка — \
             `hds stop` (index.stop), Ctrl+C или agent.enabled: false в config.yaml"
        );
        // A6.3: конфиг перечитывается между проходами. Интервал живёт в `Cell`:
        // его меняет проход (перечитанные `agent.sync.*`), читает сон цикла.
        let interval_sec = std::cell::Cell::new(interval_sec);
        let mut live = LiveConfig::new(config::config_path());
        let mut sync_now = settings::sync_settings(cfg);
        let mut max_mb_now = index_max_file_mb(cfg);
        let root_now = settings::settings(cfg).root;
        let summary = run_loop(
            || Duration::from_secs(interval_sec.get()),
            None,
            || stop.triggered() || stopped_locally(),
            |log| {
                // A6.3: правка `config.yaml` подхватывается без перезапуска.
                // Ошибка разбора не валит демон — строка `error` в журнал.
                if let Some(reloaded) = live.reload() {
                    match reloaded {
                        Err(e) => log.error(&format!("config: {e}")),
                        Ok(fresh) => {
                            let agent = settings::settings(&fresh);
                            if !agent.enabled {
                                log.config("disabled agent.enabled=false");
                                return PassResult::Stop(StopReason::Disabled);
                            }
                            // `agent.root` на лету не меняется: workspace и
                            // реестр уже открыты (TR-8.1, критерий A6.3).
                            if agent.root != root_now {
                                let new = agent
                                    .root
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_else(|| "не задан".to_string());
                                let old = root_now
                                    .as_ref()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default();
                                log.config(&format!("root ignored new={new} keep={old}"));
                                eprintln!(
                                    "[!] agent.root изменён: {new} — демон продолжает на прежнем \
                                     {old} (нужен перезапуск)"
                                );
                            }
                            let fresh_sync = settings::sync_settings(&fresh);
                            if fresh_sync != sync_now {
                                interval_sec.set(fresh_sync.interval_sec);
                                log.config(&format!(
                                    "reload interval={}s wait_stable={}s wait_poll={}ms \
                                     wait_max={}s settle_age={}s",
                                    fresh_sync.interval_sec,
                                    fresh_sync.wait_stable_sec,
                                    fresh_sync.wait_poll_ms,
                                    fresh_sync.wait_max_sec,
                                    fresh_sync.settle_age_sec
                                ));
                                mirror.set_sync_settings(fresh_sync);
                                sync_now = fresh_sync;
                            }
                            let fresh_max = index_max_file_mb(&fresh);
                            if fresh_max != max_mb_now {
                                log.config(&format!("reload index.max_file_mb={fresh_max}"));
                                mirror.set_max_file_mb(fresh_max);
                                max_mb_now = fresh_max;
                            }
                        }
                    }
                }
                match mirror.sync_once_cancellable(&|| stop.triggered() || stopped_locally()) {
                    Ok(report) => PassResult::Report(report),
                    Err(e) => PassResult::Failed(e.to_string()),
                }
            },
            &mut log,
        );
        // AG6-15(а): остановка своим флагом — это не `hds stop`: `index.stop`
        // тут ни при чём, и в журнале честнее `reason=restart`.
        let mut reason = summary.reason;
        if reason == StopReason::StopFile && !stop.triggered() && stopped_locally() {
            reason = StopReason::Restart;
        }
        (
            summary.passes,
            summary.changes,
            reason,
            // Демон не умирает от ошибок прохода: они уже в журнале.
            0,
        )
    } else {
        log.start("once", id, project_path, &workspace, None);
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
    // строка stop. Воркеры бросаем явно, до снятия lock: иначе следующий демон
    // поднимется, пока Python ещё пишет во временные файлы.
    drop(mirror);
    drop(anonymizer);
    drop(extractor);
    drop(lock);
    log.stop(reason.as_str(), passes, changes);
    println!(
        "[ok] остановлено: проект {id}, проходов {passes}, изменений {changes}, причина {}; журнал {}",
        reason.as_str(),
        log.path().display()
    );
    ProjectOutcome::done(code, Some(reason))
}

/// `--all` (A6.2, В-A6-2): все проекты `projects.json` в одном процессе.
///
/// * демон — супервизор (`Supervisor`, AG6-15(а)): по потоку на проект, у
///   каждого свой `sync.lock`, журнал, реестр и воркеры, сон интервала свой, а
///   остановка общая (`index.stop`); список проектов перечитывается раз в
///   [`SUPERVISOR_TICK`] — новый проект подхватывается без перезапуска демона;
/// * `--once` — по одному проходу на проект подряд (как у `--project <P> --once`):
///   lock тот же, занятый — «уже запущен» и код 0, поэтому параллельный ручной
///   проход не сталкивается с автозапуском (AG6-06);
/// * папка проекта недоступна — `problem` в журнале проекта и пропуск:
///   остальные проекты обрабатываются (критерий A6.2). В демоне пропуск не
///   мешает проекту подняться позже — `sync.lock` не занимается;
/// * проект уже под наблюдением другого демона (`sync.lock` занят) — это
///   норма: задача автозапуска и ручной запуск не конфликтуют. Если так у всех
///   проектов на первом тике — «уже запущен» и код 0 (AG6-15(б)).
fn sync_all(cfg: &Config, parsed: &SyncArgs) -> i32 {
    if !settings::settings(cfg).enabled {
        eprintln!("agent sync: анонимная зона выключена (agent.enabled: false)");
        return 1;
    }
    let registry = match projects::load(cfg) {
        Ok(registry) => registry,
        Err(e) => {
            eprintln!("agent sync: projects.json: {e}");
            return 1;
        }
    };
    if registry.projects.is_empty() {
        eprintln!(
            "agent sync: в projects.json нет проектов (hds agent project add P0001 --path DIR)"
        );
        return 1;
    }
    let sync = settings::sync_settings(cfg);
    let interval_sec = parsed.interval.unwrap_or(sync.interval_sec);
    let root = hds_core::config::project_root();
    let stop = StopSignal::new(root.join("index.stop"), SystemTime::now());
    println!(
        "[i] проектов в реестре: {}; режим: {}",
        registry.projects.len(),
        if parsed.daemon() {
            "демон"
        } else {
            "один проход"
        }
    );

    if parsed.daemon() {
        // AG6-15(а): супервизор. Проекты живут своими потоками (свой
        // `sync.lock`, журнал, реестр, воркеры), а супервизор раз в тик
        // перечитывает `projects.json`: новые проекты добирает, при смене пути
        // перезапускает поток, исчезнувшие — останавливает своим флагом.
        return Supervisor::new(cfg, registry, interval_sec, &stop).run();
    }

    // `--once`: один проход по каждому проекту подряд.
    let mut ok = true;
    let mut processed = 0usize;
    for project in &registry.projects {
        let path = PathBuf::from(&project.path);
        if !path.is_dir() {
            note_missing_project(cfg, project, &path);
            continue;
        }
        println!("[i] проект {} — папка {}", project.id, project.path);
        let outcome = run_project(cfg, project, &path, false, interval_sec, &stop, None);
        processed += 1;
        if outcome.code != 0 {
            ok = false;
        }
    }
    println!(
        "[ok] --all: проектов {}, прошли проход {processed}",
        registry.projects.len()
    );
    if ok && processed > 0 {
        0
    } else {
        1
    }
}

/// Проект под наблюдением супервизора `--all` (AG6-15(а)).
struct Worker {
    /// Свой флаг остановки: его поднимает супервизор, остальные проекты живут.
    flag: Arc<AtomicBool>,
    /// Поток проекта; `None` — поток завершился или ещё не запускался.
    handle: Option<JoinHandle<ProjectOutcome>>,
    /// Папка, с которой запущен поток: другая в `projects.json` — перезапуск.
    path: String,
    /// До какого момента проект не трогаем (занятый `sync.lock`, отказ запуска).
    retry_at: Option<Instant>,
}

/// Живой `sync.lock` проекта: его держит другой демон (A4.6).
///
/// Проверяется до `spawn`: так повторный запуск `hds agent sync --all` видит
/// «уже запущен» сразу, не создавая поток на каждый проект.
fn project_daemon_alive(cfg: &Config, id: &str) -> bool {
    let Some(state) = settings::state_dir_for(cfg, id) else {
        return false;
    };
    let lock = state.join("sync.lock");
    lock.is_file() && !hds_index::watch::lock_is_stale(&lock)
}

/// Супервизор `hds agent sync --all` (AG6-15(а), вариант B).
///
/// Раньше список проектов читался один раз, и новый проект подхватывался только
/// перезапуском демона. Теперь раз в [`SUPERVISOR_TICK`]:
///
/// 1. `projects.json` перечитывается, если изменились `mtime`/размер файла
///    (недописанный или битый файл — предупреждение и работа на прежнем списке,
///    повтор на следующем тике);
/// 2. новый проект — запуск потока (если не действует кулдаун и `sync.lock`
///    не занят чужим демоном);
/// 3. сменился `path` — флаг, ожидание потока, запуск заново с новой папкой;
/// 4. проект исчез из файла — флаг и ожидание потока (удаление через
///    `hds agent project rm` при живом демоне и так запрещено, это сценарий
///    ручной правки файла);
/// 5. поток завершился сам: занятый `sync.lock` или отказ запуска — кулдаун
///    [`BUSY_RETRY`]; `agent.enabled: false` — до перезапуска демона проект не
///    поднимаем (иначе был бы цикл «старт → disabled → старт»).
///
/// `hds stop` (`index.stop`) по-прежнему останавливает всё: супервизор поднимает
/// все флаги, дожидается потоков и печатает сводку. Формат `sync.log` и коды
/// возврата не меняются: остановка проекта — обычная строка `stop`, только у
/// снятого супервизором проекта причина `restart`.
struct Supervisor<'a> {
    cfg: &'a Config,
    interval_sec: u64,
    stop: &'a StopSignal,
    /// `projects.json` — перечитывается по отпечатку `mtime`+размер.
    projects_file: PathBuf,
    stamp: Option<(SystemTime, u64)>,
    registry: projects::Projects,
    workers: BTreeMap<String, Worker>,
    /// Проекты, чьей папки нет: `problem` пишется один раз на переход.
    missing: BTreeSet<String>,
    /// Проекты с `agent.enabled: false` — до перезапуска демона не поднимаем.
    disabled: BTreeSet<String>,
    /// Проекты, чей `sync.lock` держал другой демон (для сводки).
    busy: BTreeSet<String>,
    /// Последняя ошибка разбора `projects.json` — чтобы не печатать каждые 5 с.
    last_error: Option<String>,
    /// Сколько раз запускали потоки (в сводке — «запущено»).
    runs: usize,
    /// Сколько потоков завершилось отказами.
    failed: usize,
    first_tick: bool,
}

impl<'a> Supervisor<'a> {
    fn new(
        cfg: &'a Config,
        registry: projects::Projects,
        interval_sec: u64,
        stop: &'a StopSignal,
    ) -> Supervisor<'a> {
        Supervisor {
            cfg,
            interval_sec,
            stop,
            projects_file: settings::projects_file(cfg),
            stamp: fingerprint(&settings::projects_file(cfg)),
            registry,
            workers: BTreeMap::new(),
            missing: BTreeSet::new(),
            disabled: BTreeSet::new(),
            busy: BTreeSet::new(),
            last_error: None,
            runs: 0,
            failed: 0,
            first_tick: true,
        }
    }

    /// Главный цикл: перечитать список → сверить проекты → сон шагами ≤ 500 мс.
    fn run(mut self) -> i32 {
        loop {
            self.reload();
            self.tick();
            self.reap();
            if self.stop.triggered() {
                break;
            }
            if self.first_tick && self.runs == 0 && self.failed == 0 {
                // Первый тик не поднял ни одного потока: все проекты уже под
                // наблюдением (повторный запуск задачи автозапуска) или папок
                // нет. Ждать сигнала незачем — сообщения и коды прежние
                // (AG6-15(б): недоступные папки не уводят задачу в перезапуски).
                println!("[ok] --all: {}", self.summary());
                if !self.busy.is_empty() {
                    println!(
                        "[i] демон зеркала уже запущен для всех проектов (sync.lock) — выхожу"
                    );
                } else {
                    println!("[i] все проекты пропущены: папок нет — выхожу без ошибки");
                }
                return 0;
            }
            self.first_tick = false;
            if !sleep_interval(SUPERVISOR_TICK, &|| self.stop.triggered()) {
                break;
            }
        }
        // `hds stop`: поднимаем все флаги и ждём каждый поток (AG6-02: поток
        // дорабатывает текущий файл и выходит сам).
        self.stop_all();
        println!("[ok] --all: {}", self.summary());
        if self.runs > 0 || self.failed == 0 {
            0
        } else {
            1
        }
    }

    /// Перечитать `projects.json`, если файл изменился.
    fn reload(&mut self) {
        let now = fingerprint(&self.projects_file);
        if now == self.stamp {
            return;
        }
        match projects::load(self.cfg) {
            Ok(fresh) => {
                let before = self.registry.projects.len();
                let after = fresh.projects.len();
                self.registry = fresh;
                self.stamp = now;
                self.last_error = None;
                if before != after {
                    println!("[i] projects.json перечитан: проектов {after} (было {before})");
                }
            }
            Err(e) => {
                // Недописанный или битый файл: работаем на прежнем списке и
                // повторяем попытку на следующем тике (отпечаток не сдвигаем).
                if self.last_error.as_deref() != Some(e.as_str()) {
                    eprintln!("[!] projects.json не перечитан: {e}");
                    self.last_error = Some(e);
                }
            }
        }
    }

    /// Сводка в прежнем формате (запущено / уже работали / пропущено / отказов).
    fn summary(&self) -> String {
        format!(
            "запущено {}, уже работали {}, пропущено (папка) {}, отказов {}",
            self.runs,
            self.busy.len(),
            self.missing.len(),
            self.failed
        )
    }

    /// Один тик супервизора: сверить список проектов с потоками.
    fn tick(&mut self) {
        let now = Instant::now();
        let known: BTreeSet<String> = self
            .registry
            .projects
            .iter()
            .map(|p| p.id.clone())
            .collect();

        for project in self.registry.projects.clone() {
            let id = project.id.clone();
            let path = PathBuf::from(&project.path);
            if !path.is_dir() {
                // Поток по проекту без папки не держим: проход по
                // несуществующей папке — только строки `error` в журнале.
                self.stop_worker(&id);
                if self.missing.insert(id.clone()) {
                    note_missing_project(self.cfg, &project, &path);
                }
                continue;
            }
            if self.missing.remove(&id) {
                println!("[i] проект {id}: папка снова доступна — запускаю");
            }
            if self.disabled.contains(&id) {
                continue;
            }

            let (running, same_path, ready) = match self.workers.get(&id) {
                Some(w) => (
                    w.handle.is_some(),
                    w.path == project.path,
                    w.retry_at.map(|t| t <= now).unwrap_or(true),
                ),
                // Проекта в карте нет: запускать можно сразу.
                None => (false, false, true),
            };
            if running {
                if same_path {
                    continue;
                }
                // Папка проекта сменилась: поднимаем флаг, ждём поток (он
                // доработает текущий файл) и стартуем заново.
                self.stop_worker(&id);
            } else if same_path && !ready {
                // Кулдаун после занятого `sync.lock` или отказа запуска.
                continue;
            }

            if project_daemon_alive(self.cfg, &id) {
                // Проект ведёт другой демон (обычно автозапуск): своих потоков
                // не плодим, вернёмся к нему после кулдауна.
                if self.busy.insert(id.clone()) {
                    println!("[i] зеркало уже запущено для {id} (sync.lock) — слежу");
                }
                self.workers.insert(
                    id,
                    Worker {
                        flag: Arc::new(AtomicBool::new(false)),
                        handle: None,
                        path: project.path.clone(),
                        retry_at: Some(now + BUSY_RETRY),
                    },
                );
                continue;
            }
            self.start_worker(&project);
        }

        // Проекты, которых больше нет в файле: останавливаем своим флагом.
        let vanished: Vec<String> = self
            .workers
            .keys()
            .filter(|id| !known.contains(*id))
            .cloned()
            .collect();
        for id in vanished {
            self.stop_worker(&id);
            self.disabled.remove(&id);
            println!("[i] проект {id} исчез из projects.json — поток остановлен");
        }
    }

    /// Забрать завершившиеся потоки: учесть итог и поставить кулдаун, если надо.
    fn reap(&mut self) {
        let now = Instant::now();
        let finished: Vec<String> = self
            .workers
            .iter()
            .filter(|(_, w)| w.handle.as_ref().map(|h| h.is_finished()).unwrap_or(false))
            .map(|(id, _)| id.clone())
            .collect();
        for id in finished {
            let Some(handle) = self.workers.get_mut(&id).and_then(|w| w.handle.take()) else {
                continue;
            };
            match handle.join() {
                Ok(outcome) => {
                    if outcome.code != 0 {
                        self.failed += 1;
                    }
                    if !outcome.started && outcome.code == 0 {
                        // Занятый `sync.lock`: проект ведёт другой демон.
                        self.busy.insert(id.clone());
                    }
                    if outcome.reason == Some(StopReason::Disabled) {
                        // Зону выключили в config.yaml: поднимать проект заново
                        // каждые 5 с — значит крутить «старт → disabled → старт».
                        self.disabled.insert(id.clone());
                        println!(
                            "[i] проект {id}: agent.enabled=false — до перезапуска демона не веду"
                        );
                    }
                    if outcome.code != 0 || !outcome.started {
                        if let Some(worker) = self.workers.get_mut(&id) {
                            worker.retry_at = Some(now + BUSY_RETRY);
                        }
                    }
                }
                Err(_) => {
                    self.failed += 1;
                    if let Some(worker) = self.workers.get_mut(&id) {
                        worker.retry_at = Some(now + BUSY_RETRY);
                    }
                }
            }
        }
    }

    /// Поднять флаг проекта и дождаться его потока (AG6-02: поток дорабатывает
    /// текущий файл и выходит сам).
    fn stop_worker(&mut self, id: &str) -> Option<ProjectOutcome> {
        let worker = self.workers.remove(id)?;
        worker.flag.store(true, Ordering::Relaxed);
        let outcome = worker.handle.and_then(|handle| handle.join().ok());
        if let Some(outcome) = &outcome {
            if outcome.code != 0 {
                self.failed += 1;
            }
        }
        outcome
    }

    /// Запустить поток проекта (флаг остановки — свой у каждого проекта).
    fn start_worker(&mut self, project: &projects::Project) {
        let id = project.id.clone();
        let path = PathBuf::from(&project.path);
        let flag = Arc::new(AtomicBool::new(false));
        // Запись в карту — до `spawn`: иначе следующий тик увидит проект без
        // потока и запустит второй (дубли потоков запрещены).
        self.workers.insert(
            id.clone(),
            Worker {
                flag: Arc::clone(&flag),
                handle: None,
                path: project.path.clone(),
                retry_at: None,
            },
        );
        println!("[i] проект {} — папка {}", project.id, project.path);
        let cfg = self.cfg.clone();
        let project = project.clone();
        let stop = self.stop.clone();
        let interval_sec = self.interval_sec;
        let handle = std::thread::spawn(move || {
            run_project(
                &cfg,
                &project,
                &path,
                true,
                interval_sec,
                &stop,
                Some(&flag),
            )
        });
        if let Some(worker) = self.workers.get_mut(&id) {
            worker.handle = Some(handle);
        }
        self.runs += 1;
    }

    /// Остановить все проекты (`hds stop`, Ctrl+C, конец работы).
    fn stop_all(&mut self) {
        let ids: Vec<String> = self.workers.keys().cloned().collect();
        for id in ids {
            self.stop_worker(&id);
        }
    }
}

/// Проект без папки: `problem` в его журнале и пропуск (критерий A6.2).
///
/// `sync.lock` не занимается: как только папка вернётся, проект поднимется
/// обычным порядком — следующим проходом демона или ручным запуском.
fn note_missing_project(cfg: &Config, project: &projects::Project, path: &Path) {
    eprintln!(
        "[!] проект {}: папка проекта не найдена — пропуск ({})",
        project.id,
        path.display()
    );
    let Some(state_dir) = settings::state_dir_for(cfg, &project.id) else {
        return;
    };
    if std::fs::create_dir_all(&state_dir).is_err() {
        return;
    }
    if let Ok(mut log) = SyncLog::open(&state_dir.join("sync.log")) {
        log.problem(&format!(
            "project={} missing source={}",
            project.id,
            path.display()
        ));
    }
}

/// Фабрики воркеров живут в `hds-agent` (A5.1): тем же способом поднимает
/// воркеры вкладка «Агент». Здесь — реэкспорт, чтобы не менять вызовы:
/// `hds agent result` берёт их отсюда с A4.4, демон — тоже.
pub(crate) use hds_agent::factory::{anonymizer_factory, extractor_factory};

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
    /// `agent.enabled: false` в перечитанном конфиге (A6.3): зону выключили,
    /// демон останавливается без `index.stop` — общий файл не трогаем (AG-27).
    Disabled,
    /// Проект снят супервизором `--all` (AG6-15(а)): сменилась папка проекта
    /// в `projects.json` (поток перезапускается) или проект из файла исчез.
    /// `index.stop` при этом не создаётся и не трогается.
    Restart,
}

impl StopReason {
    /// Метка для строки `stop` в журнале.
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::MaxPasses => "max_passes",
            StopReason::StopFile => "index.stop",
            StopReason::Once => "once",
            StopReason::Error => "error",
            StopReason::Disabled => "disabled",
            StopReason::Restart => "restart",
        }
    }
}

/// Результат одного прохода для цикла: отчёт, текст ошибки (без значений) или
/// требование остановиться (`agent.enabled: false`, A6.3).
pub enum PassResult {
    /// Проход удался — счётчики [`SyncReport`].
    Report(SyncReport),
    /// Проход не удался; текст пишется в журнал строкой `error`.
    Failed(String),
    /// Демон должен остановиться с этой причиной (`disabled` — A6.3).
    Stop(StopReason),
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
///
/// * интервал — функция: A6.3 перечитывает `agent.sync.interval_sec` между
///   проходами, и следующий сон берёт уже новое значение;
/// * проход получает журнал: проверка конфига пишет строку `config` до обхода;
/// * `PassResult::Stop` — проход сам просит остановиться (например
///   `agent.enabled: false` → `disabled`).
pub fn run_loop<F, I, P>(
    interval: I,
    max_passes: Option<usize>,
    should_stop: F,
    mut one_pass: P,
    log: &mut SyncLog,
) -> LoopSummary
where
    F: Fn() -> bool,
    I: Fn() -> Duration,
    P: FnMut(&mut SyncLog) -> PassResult,
{
    let mut passes = 0usize;
    let mut changes = 0usize;
    let reason;
    loop {
        if should_stop() {
            reason = StopReason::StopFile;
            break;
        }
        match one_pass(log) {
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
            PassResult::Stop(why) => {
                // Проход не состоялся: счётчики не растут, причина — от прохода.
                reason = why;
                break;
            }
        }
        passes += 1;
        if let Some(max) = max_passes {
            if passes >= max {
                reason = StopReason::MaxPasses;
                break;
            }
        }
        if !sleep_interval(interval(), &should_stop) {
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
/// [`LOG_PROBLEMS`] штук за проход и пропуски проекта в `--all`), `config`
/// (перечитывание конфига, A6.3), `error` (проход не удался), `stop`
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

    /// `config`: горячее перечитывание `config.yaml` (A6.3) — только имена
    /// настроек и числа (`interval=2s`, `index.max_file_mb=50`, `disabled`).
    pub fn config(&mut self, note: &str) {
        self.line(&format!("config {note}"));
    }

    /// `problem`: одна строка о проблеме (пропуск проекта в `--all`, A6.2).
    pub fn problem(&mut self, problem: &str) {
        self.line(&format!("problem {problem}"));
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

/// Разобранные аргументы `hds agent sync` (A4.6: `--once | --interval SEC`;
/// A6.2: `--all` — все проекты `projects.json` в одном процессе).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncArgs {
    /// Идентификатор проекта (`P` + 4 цифры, D21); `None` при `--all`.
    pub project: Option<String>,
    /// `--all` — все проекты реестра в одном процессе (A6.2, В-A6-2).
    pub all: bool,
    /// `--once` — один проход (тот же режим, что и без флагов).
    pub once: bool,
    /// `--interval SEC` — демон; `None` — разовый проход.
    pub interval: Option<u64>,
}

impl SyncArgs {
    /// Демон ли это (`--interval`).
    ///
    /// `--all` без `--once` — тоже демон: задачу автозапуска запускают как
    /// `hds agent sync --all`, и разовый проход там не имел бы смысла (A6.2).
    pub fn daemon(&self) -> bool {
        self.interval.is_some() || (self.all && !self.once)
    }
}

/// Разбор аргументов: `--project <P>`/`--project=<P>`, `--all`, `--once`,
/// `--interval SEC`/`--interval=SEC`.
///
/// Отказы (код 2) — те же, что были в A4.1, плюс правила A4.6: `--interval`
/// без значения, `0`, отрицательное или не число, и `--once` вместе с
/// `--interval` (один проход и демон — разные режимы, угадывать нечего).
/// A6.2: `--all` и `--project` вместе — тоже отказ (один режим на запуск).
pub fn parse_args(args: &[String]) -> Result<SyncArgs, String> {
    let mut project: Option<String> = None;
    let mut all = false;
    let mut once = false;
    let mut interval: Option<u64> = None;
    let mut i = 0usize;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if arg == "--all" {
            all = true;
            continue;
        }
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
    if all && project.is_some() {
        return Err("--all и --project вместе: выберите один режим".to_string());
    }
    if !all && project.is_none() {
        return Err("нужен --project <P0001> или --all".to_string());
    }
    Ok(SyncArgs {
        project,
        all,
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
    "использование: hds agent sync (--project <P> | --all) [--once | --interval SEC]\n\
     \x20 (один проход: обход папки проекта, wait_stable, образы docx/xlsx/txt/md/xml\n\
     \x20  и текстовые образы pdf/pptx/mpp, unsupported/too_big, удаление и\n\
     \x20  переименование, обратный путь: agent_new → папка проекта, agent_changed →\n\
     \x20  .hds-agent\\pending; TR-7.1…7.7)\n\
     \x20 без флагов и с --once — один проход (совместимость);\n\
     \x20 --interval SEC — демон: проход, сон SEC (дефолт agent.sync.interval_sec = 5),\n\
     \x20  остановка — `hds stop` (index.stop) или Ctrl+C; lock — <P>\\agent-state\\sync.lock;\n\
     \x20  журнал — <agent.root>\\P<NNNN>\\agent-state\\sync.log (без содержимого, I8);\n\
     \x20 --all — все проекты data\\agent\\projects.json в одном процессе (A6.2):\n\
     \x20  у каждого свой sync.lock, журнал, реестр и воркеры; папка проекта\n\
     \x20  недоступна — пропуск со строкой problem в его журнале; без --once — демон\n\
     \x20  с интервалом agent.sync.interval_sec (автозапуск:\n\
     \x20  installers\\install_agent_sync_task.ps1); projects.json перечитывается\n\
     \x20  раз в 5 с: новый проект подхватывается, смена папки перезапускает поток,\n\
     \x20  исчезнувший проект останавливается (stop reason=restart; AG6-15);\n\
     \x20 между проходами перечитывается config.yaml (A6.3): agent.sync.*,\n\
     \x20  index.max_file_mb, agent.enabled (false — остановка `stop reason=disabled`);\n\
     \x20  agent.root на лету не меняется (`[!]`, работа на прежнем);\n\
     \x20 путь реестра: <state_dir>\\data\\agent\\registry\\<P>.db, TR-7.2)"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// AG-27: устаревший `index.stop` (лежал до старта) демон не останавливает,
    /// а новый сигнал — да; сам файл остаётся на месте (его снимает индексатор).
    #[test]
    fn stale_stop_signal_is_ignored_and_file_is_kept() {
        let dir = temp_dir("stopsignal");
        let path = dir.join("index.stop");
        std::fs::write(&path, b"").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        // Старт «после» файла — сигнал устарел.
        let stale = StopSignal::new(path.clone(), SystemTime::now());
        assert!(!stale.triggered(), "старый файл — не сигнал");
        // Старт «до» файла — сигнал свежий.
        let fresh = StopSignal::new(path.clone(), SystemTime::now() - Duration::from_secs(1));
        assert!(fresh.triggered(), "новый файл — сигнал");
        assert!(path.exists(), "файл не снимается: его ждёт hds watch");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        assert_eq!(daemon.project.as_deref(), Some("P0001"));
        assert!(!daemon.all);
        assert!(daemon.daemon());
        assert_eq!(daemon.interval, Some(7));
        assert!(parse_args(&args(&["--project", "P0001", "--interval=3"]))
            .unwrap()
            .daemon());
        let once = parse_args(&args(&["--project=P0001", "--once"])).unwrap();
        assert!(once.once && !once.daemon());

        // A6.2: `--all` — все проекты реестра в одном процессе.
        let all = parse_args(&args(&["--all"])).unwrap();
        assert!(all.all && all.project.is_none());
        assert!(
            all.daemon(),
            "--all без --once — демон (задача автозапуска)"
        );
        assert!(!parse_args(&args(&["--all", "--once"])).unwrap().daemon());
        assert!(parse_args(&args(&["--all", "--interval", "9"]))
            .unwrap()
            .daemon());

        for bad in [
            vec!["--project", "P0001", "--interval", "0"],
            vec!["--project", "P0001", "--interval", "-5"],
            vec!["--project", "P0001", "--interval", "abc"],
            vec!["--project", "P0001", "--interval"],
            vec!["--project", "P0001", "--once", "--interval", "5"],
            vec!["--project", "P0001", "--bogus"],
            vec!["--once"],
            vec!["--all", "--project", "P0001"],
            vec!["--all", "--once", "--interval", "5"],
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
        log.problem("project=P0002 missing source=D:/нет");
        log.config("reload interval=2s");
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
            text.contains("problem project=P0002 missing source=D:/нет"),
            "пропуск проекта в --all пишется строкой problem (A6.2): {text}"
        );
        assert!(
            text.contains("config reload interval=2s"),
            "перечитывание конфига пишется строкой config (A6.3): {text}"
        );
        assert!(
            text.contains("stop reason=once passes=1 changes=3"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 7, "одна запись — одна строка: {text}");
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

    /// A6.3: конфиг перечитывается только после правки файла (`mtime`/размер),
    /// битый YAML — ошибка, а не паника: демон остаётся на прежних настройках.
    #[test]
    fn live_config_reloads_only_after_edit() {
        let dir = temp_dir("live-config");
        let path = dir.join("config.yaml");
        std::fs::write(&path, "agent:\n  enabled: true\n").unwrap();
        let mut live = LiveConfig::new(path.clone());
        assert!(live.reload().is_none(), "без правки файл не перечитывается");

        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(
            &path,
            "agent:\n  enabled: true\n  sync:\n    interval_sec: 9\n",
        )
        .unwrap();
        let fresh = live
            .reload()
            .expect("правка замечена")
            .expect("конфиг разобран");
        assert_eq!(settings::sync_settings(&fresh).interval_sec, 9);
        assert!(settings::settings(&fresh).enabled);
        assert!(live.reload().is_none(), "повторно не перечитывается");

        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&path, "agent: [broken").unwrap();
        assert!(
            live.reload().expect("правка замечена").is_err(),
            "битый YAML — ошибка, а не паника"
        );
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
            || Duration::from_millis(1),
            Some(2),
            || false,
            |_log| {
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
            || Duration::from_secs(60),
            None,
            move || checked.load(Ordering::Relaxed),
            move |_log| {
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
            || Duration::from_millis(1),
            Some(2),
            || false,
            |_log| PassResult::Failed("обход: отказано в доступе".to_string()),
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

    /// A6.3: проход сам просит остановиться (`agent.enabled: false`) — причина
    /// `disabled` в итоге цикла, проход не считается, сон не наступает.
    #[test]
    fn loop_stops_when_pass_asks() {
        let dir = temp_dir("pass-stop");
        let path = dir.join("sync.log");
        let mut log = SyncLog::open(&path).unwrap();
        let started = std::time::Instant::now();
        let summary = run_loop(
            || Duration::from_secs(60),
            None,
            || false,
            |log| {
                log.config("disabled agent.enabled=false");
                PassResult::Stop(StopReason::Disabled)
            },
            &mut log,
        );
        assert_eq!(summary.reason, StopReason::Disabled);
        assert_eq!(summary.passes, 0, "прохода не было");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "остановка без выжидания интервала"
        );
        assert_eq!(summary.reason.as_str(), "disabled");
        cleanup(&dir);
    }
}
