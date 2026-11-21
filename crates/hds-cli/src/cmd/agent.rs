//! `hds agent` — служебные команды анонимной зоны (фаза A2).
//!
//! * `hds agent init [--yes] [--root DIR] [--no-index]` — сброс и первичное
//!   построение индекса анонимной зоны (TR-4.2, D9): останавливает `hds watch`,
//!   удаляет `index.db` (+`-wal`/`-shm`) и выход прежней карточки анонимизации
//!   (`anonymize.output_dir`), создаёт пустую базу и запускает индексацию
//!   `<agent.root>\P<NNNN>\workspace`. Команда идемпотентна;
//! * `hds agent token <P> [--ttl-days N]` — выдать токен проекта для маршрута
//!   `/mcp/agent/<P>` (в файл — только хэш, plain печатается один раз).
//!
//! Всё, что удаляется, перечисляется до удаления; без `--yes` команда спрашивает
//! подтверждение и в неинтерактивном запуске (установщик, служба) отказывается
//! работать — молча сносить индекс нельзя.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hds_core::config::{self, project_root, Config};
use hds_core::roots::is_agent_project;

use crate::support::{build_embedder, build_sidecar, open_conn};

/// Точка входа `hds agent ...` (конфиг читается как обычно — `HDS_CONFIG`).
pub fn cmd_agent(args: &[String]) -> i32 {
    let cfg = match hds_core::config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    cmd_agent_cfg(&cfg, args)
}

/// То же с явным конфигом (тесты не трогают боевой `config.yaml`).
pub fn cmd_agent_cfg(cfg: &Config, args: &[String]) -> i32 {
    let (sub, rest) = match args.split_first() {
        Some((s, r)) => (s.as_str(), r),
        None => {
            eprintln!("{}", usage());
            return 2;
        }
    };
    match sub {
        "init" => cmd_init(cfg, rest),
        "check" => cmd_check(cfg, rest),
        "token" => cmd_token(cfg, rest),
        "project" => crate::cmd::agent_run::cmd_project(cfg, rest),
        "run" => crate::cmd::agent_run::cmd_run(cfg, rest),
        "history" => crate::cmd::agent_run::cmd_history(cfg, rest),
        "sync" => crate::cmd::agent_sync::cmd_sync(cfg, rest),
        "result" => crate::cmd::agent_result::cmd_result(cfg, rest),
        "--help" | "-h" | "help" => {
            println!("{}", usage());
            0
        }
        other => {
            eprintln!("agent: неизвестная подкоманда '{other}'");
            eprintln!("{}", usage());
            2
        }
    }
}

fn usage() -> String {
    "использование: hds agent <подкоманда>\n\
     \x20 init  [--yes] [--root DIR] [--no-index] [--force]\n\
     \x20       (сброс индекса: стоп hds watch, удаление index.db и выхода\n\
     \x20        анонимизации, пустая база, индексация <agent.root>\\P*\\workspace; TR-4.2;\n\
     \x20        выход анонимизации удаляется, только если он внутри <state_dir>\\data\n\
     \x20        или <agent.root> — иначе нужен --force)\n\
     \x20 token <P> [--ttl-days N]\n\
     \x20       (токен проекта для /mcp/agent/<P>: в файл — хэш, plain печатается один раз;\n\
     \x20        без --ttl-days токен бессрочный)\n\
     \x20 check [--project <P>] [--json]\n\
     \x20       (пункты agent-* из `hds check`: зона, Node-воркер, Python, база меток,\n\
     \x20        реестр проектов, ключи и токены, маршрут /mcp/agent/<P>, индекс,\n\
     \x20        демон зеркала; значения секретов не печатаются; A6.1)\n\
     \x20 project add <P> --name NAME --path DIR [--provider NAME] [--model M]\n\
     \x20       (реестр проектов data\\agent\\projects.json; провайдер по умолчанию\n\
     \x20        openrouter, модель — из В-12; имя — имя папки, если не задано)\n\
     \x20 project list [--json]\n\
     \x20       (список проектов зоны)\n\
     \x20 run --project <P> [--prompt TEXT] [--session ID] [--max-iterations N]\n\
     \x20     [--subagents] [--json] [--quiet] [--dry-run]\n\
     \x20       (сессия агента: снимок vault, env, MCP, правила, Gateway; без --prompt\n\
     \x20        реплика читается со stdin; --session возобновляет сессию вместе с её\n\
     \x20        историей; --dry-run печатает план и не запускает воркер)\n\
     \x20 history clear --project <P> [--session ID]\n\
     \x20       (Очистить историю: значения в файлах сессии заменяются метками,\n\
     \x20        рядом остаётся бэкап .json.bak; без --session — последняя сессия)\n\
     \x20 sync (--project <P> | --all) [--once | --interval SEC]\n\
     \x20       (зеркало папки проекта и <P>\\workspace; реестр\n\
     \x20        data\\agent\\registry\\<P>.db, прямой и обратный путь; --interval —\n\
     \x20        демон по таймеру, остановка `hds stop`/Ctrl+C, журнал\n\
     \x20        <P>\\agent-state\\sync.log; A4.6; --all — все проекты projects.json\n\
     \x20        в одном процессе, без --once — демон; A6.2)\n\
     \x20 result accept|reject|conflict --project <P> --path <отн. путь>\n\
     \x20       [--choice agent|new|delegate] [--json]\n\
     \x20       (решения по варианту агента: backup и замена оригинала, отклонение\n\
     \x20        с сообщением агенту, три исхода конфликта; TR-7.6, A4.4)\n\
     \x20 result restore|rebuild|relink --project <P> --path <отн. путь>\n\
     \x20       [--stamp <штамп>] [--json]\n\
     \x20       («Вернуть» из .hds-agent\\backup с выбором штампа, «Пересоздать образ»\n\
     \x20        для orphan, relink по маркеру образа — только по команде; журнал\n\
     \x20        решений decisions.jsonl; A6.4)"
        .to_string()
}

/// Есть ли флаг `name` среди аргументов (те же правила, что у `hds agent init`).
pub(crate) fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Значение флага в виде строки (`--name value` или `--name=value`).
pub(crate) fn flag_val(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(name) {
            if let Some(v) = rest.strip_prefix('=') {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Остановить `hds watch` сигналом `index.stop` и дождаться снятия `watch.lock`.
///
/// Удалять `index.db` под живым watcher'ом нельзя: он держит соединение и
/// продолжает писать. Таймаут — честный отказ, а не «продолжим на удачу».
fn stop_watch(root: &Path, lock: &Path, wait_secs: u64) -> Result<(), String> {
    if !lock.exists() || hds_index::watch::lock_is_stale(lock) {
        return Ok(());
    }
    std::fs::write(root.join("index.stop"), b"")
        .map_err(|e| format!("не создать index.stop: {e}"))?;
    println!("[..] остановка демона индексации (index.stop)…");
    let deadline = Instant::now() + Duration::from_secs(wait_secs);
    while Instant::now() < deadline {
        if !lock.exists() {
            println!("[ok] демон индексации остановлен");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!(
        "демон индексации не остановился за {wait_secs} с: остановите `hds watch` \
         (или службу) вручную и повторите `hds agent init`"
    ))
}

/// Лежит ли `child` строго внутри `parent` (по компонентам, без регистра).
///
/// `std::path::absolute` — лексическая нормализация (`.`/`..`) без обращений к
/// ФС: для предохранителя от «чужого каталога» этого достаточно, а сбой
/// канонизации (нет каталога) не должен разрешать удаление.
fn is_within(child: &Path, parent: &Path) -> bool {
    fn comps(p: &Path) -> Vec<String> {
        let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        abs.components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    }
    let c = comps(child);
    let p = comps(parent);
    !p.is_empty() && c.len() > p.len() && c[..p.len()] == p[..]
}

/// Разрешить путь: существующие ссылки/junction раскрываются (`canonicalize`),
/// иначе — лексическая нормализация. Нужно предохранителю В-10: проверка
/// вложенности должна видеть реальный каталог, а не ссылку на него.
fn resolve_path(p: &Path) -> PathBuf {
    std::fs::canonicalize(p)
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf())
}

/// Удалить файл или каталог, если он есть.
fn remove_path(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Ok(());
    }
    let res = if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    res.map_err(|e| format!("не удалить {}: {e}", p.display()))?;
    println!("[ok] удалено: {}", p.display());
    Ok(())
}

/// `hds agent token <P>` — выдать токен проекта (в файл — только хэш).
///
/// Plain-токен печатается в stdout **одной строкой** (его читает установщик и
/// подставляет в настройки MCP агента); пояснения идут в stderr.
fn cmd_token(cfg: &Config, rest: &[String]) -> i32 {
    let Some(project) = rest.iter().find(|a| !a.starts_with('-')) else {
        eprintln!("agent token: нужен id проекта (P0001)");
        eprintln!("{}", usage());
        return 2;
    };
    if !is_agent_project(project) {
        eprintln!("неверный id проекта '{project}': ожидается P + 4 цифры (например P0001)");
        return 2;
    }
    let ttl_days = flag_val(rest, "--ttl-days").and_then(|v| v.trim().parse::<u64>().ok());
    let path = hds_mcp::agent::secrets_path_from(cfg);
    let mut secrets = match hds_mcp::agent::load_at(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("хранилище токенов: {e}");
            return 1;
        }
    };
    // `expires: 0` — бессрочный: MVP-агент проекта живёт, пока жив проект.
    let (plain, token) = match hds_core::access::new_token(
        project,
        ttl_days.unwrap_or(36_500),
        hds_mcp::auth::now_unix(),
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("не создать токен: {}", e.message());
            return 1;
        }
    };
    let expires = if ttl_days.is_some() { token.expires } else { 0 };
    secrets.projects.insert(
        project.clone(),
        hds_mcp::agent::AgentToken {
            token_sha256: token.token_sha256,
            expires,
            // Plain-токен нужен агенту для `Authorization` в
            // `cline_mcp_settings.json` (TR-3.6); файл под ACL (В-1).
            token: Some(plain.clone()),
        },
    );
    if let Err(e) = hds_mcp::agent::save_at(&path, &secrets) {
        eprintln!("не сохранить токен: {e}");
        return 1;
    }
    lock_down(&path);
    let term = if expires == 0 {
        "бессрочный".to_string()
    } else {
        format!("истекает {expires} (unix)")
    };
    eprintln!(
        "токен проекта {project} сохранён (хэш sha256): {}; {term}",
        path.display()
    );
    if !hds_core::roots::agent_roots(cfg)
        .iter()
        .any(|r| r.project == *project)
    {
        eprintln!(
            "[warn] проект {project} не найден под agent.root — токен выдан «наперёд»; \
             каталог {project}\\workspace должен появиться до первого поиска"
        );
    }
    // Plain-токен — единственная строка stdout: его читают скрипты.
    println!("{plain}");
    0
}

/// Best-effort ACL «только владелец» на файл токенов (В-1).
///
/// Полноценная настройка ACL — A6 (`TR-8.3`); здесь важно, чтобы файл не
/// остался доступен «всем» на общей машине. Ошибка `icacls` — предупреждение,
/// а не отказ: на некоторых ФС (FAT, сетевые шары) ACL не поддерживаются.
#[cfg(windows)]
fn lock_down(path: &Path) {
    let Ok(user) = std::env::var("USERNAME") else {
        return;
    };
    if user.trim().is_empty() {
        return;
    }
    let status = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:F"))
        // Вывод icacls не нужен: успех проверяем по коду возврата.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        eprintln!(
            "[warn] не удалось выставить ACL «только владелец» на {} — проверьте права вручную",
            path.display()
        );
    }
}

/// Вне Windows файл токенов защищается правами каталога (0600 ставит установщик).
#[cfg(not(windows))]
fn lock_down(_path: &Path) {}

/// Конфиг с подменой `agent.root` (флаг `--root DIR`), без записи в файл.
///
/// Подмена нужна установщику и стендам: у них корень зоны задан аргументом, а
/// не в `config.yaml`, и индексация обязана идти по тому же каталогу.
fn cfg_with_root(cfg: &Config, root: Option<String>) -> Config {
    let mut cfg = cfg.clone();
    if let Some(root) = root {
        if let Some(agent) = cfg.get_mut("agent").and_then(|v| v.as_mapping_mut()) {
            agent.insert(
                serde_yaml::Value::String("root".into()),
                serde_yaml::Value::String(root),
            );
        }
    }
    cfg
}

/// `hds agent init` — сброс и первичное построение индекса (TR-4.2).
fn cmd_init(cfg: &Config, rest: &[String]) -> i32 {
    let cfg = cfg_with_root(cfg, flag_val(rest, "--root"));
    if !hds_core::roots::agent_enabled(&cfg) {
        eprintln!(
            "agent.enabled != true: режим анонимной зоны выключен.\n\
             Включите его в config.yaml (см. config.example.yaml, секция agent):\n\
             \x20 agent:\n\
             \x20   enabled: true\n\
             \x20   root: 'D:\\\\hds-anon'"
        );
        return 1;
    }
    let Some(agent_root) = hds_core::roots::agent_root(&cfg) else {
        eprintln!("agent.root не задан — укажите каталог анонимной зоны в config.yaml");
        return 1;
    };
    let projects = hds_core::roots::agent_roots(&cfg);
    if projects.is_empty() {
        eprintln!(
            "в {agent_root} нет проектов вида P0001\\workspace — создайте каталог проекта \
             (демон зеркалирования делает это в фазе A4) и повторите"
        );
        return 1;
    }

    let root = project_root();
    let db = config::db_abs_path(&cfg);
    let anonymized = hds_anonymize::Settings::from_config(&cfg).output_dir_abs(&root);
    let watch_lock = root.join("watch.lock");
    let watch_running = watch_lock.exists() && !hds_index::watch::lock_is_stale(&watch_lock);

    // Всё, что будет удалено, перечисляем ДО удаления (TR-4.2: подтверждение).
    let mut doomed: Vec<PathBuf> = vec![db.clone()];
    for suffix in ["-wal", "-shm"] {
        doomed.push(PathBuf::from(format!("{}{suffix}", db.display())));
    }
    doomed.push(anonymized.clone());
    // В-10: удаляем выход анонимизации только там, где он заведомо наш —
    // внутри `<state_dir>\data` или внутри `<agent.root>`. Иначе владелец мог
    // указать в `anonymize.output_dir` общий/сетевой каталог, и сброс индекса
    // снёс бы его целиком.
    let force = has(rest, "--force");
    let state_data = config::state_dir(&cfg).join("data");
    // В-10: вложенность проверяется по РАЗРЕШЁННЫМ путям — junction/симлинк
    // внутри разрешённого каталога не должен давать удалить чужое дерево.
    let anonymized_real = resolve_path(&anonymized);
    let state_data_real = resolve_path(&state_data);
    let agent_root_real = resolve_path(Path::new(&agent_root));
    let in_state_data = is_within(&anonymized_real, &state_data_real);
    let in_agent_root = is_within(&anonymized_real, &agent_root_real);
    if anonymized.exists() && !in_state_data && !in_agent_root && !force {
        eprintln!(
            "каталог выхода анонимизации {} лежит вне {} и вне {}.\n\
             Удалять его по умолчанию отказываюсь: в anonymize.output_dir мог оказаться \
             общий каталог (стенд, сетевая папка, папка рядом с оригиналами).\n\
             Если это действительно выход прежней карточки — повторите с --force.",
            anonymized.display(),
            state_data.display(),
            agent_root
        );
        return 1;
    }
    println!("Анонимная зона: {agent_root}");
    println!(
        "Проекты: {}",
        projects
            .iter()
            .map(|r| r.project.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("Индекс: {}", db.display());
    println!("Будет очищено:");
    for p in &doomed {
        println!(
            "  {} ({})",
            p.display(),
            if p.exists() { "есть" } else { "нет" }
        );
    }
    if watch_running {
        println!("Демон индексации: запущен — будет остановлен");
    }

    if !has(rest, "--yes") {
        if !std::io::stdin().is_terminal() {
            eprintln!("нет терминала для подтверждения — повторите с --yes");
            return 2;
        }
        print!("Продолжить? [y/N]: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            eprintln!("не удалось прочитать ответ — отменено");
            return 2;
        }
        let answer = line.trim().to_lowercase();
        if !matches!(answer.as_str(), "y" | "yes" | "д" | "да") {
            println!("Отменено, ничего не изменено.");
            return 1;
        }
    }

    // 1. Остановить watcher: без этого он держит базу и продолжит индексировать.
    if let Err(e) = stop_watch(&root, &watch_lock, 60) {
        eprintln!("{e}");
        return 1;
    }
    // 2. Снести базу и выход прежней карточки анонимизации.
    if let Err(e) = remove_path(&db) {
        eprintln!("{e}");
        return 1;
    }
    for suffix in ["-wal", "-shm"] {
        let _ = remove_path(&PathBuf::from(format!("{}{suffix}", db.display())));
    }
    if anonymized.exists() {
        if let Err(e) = std::fs::remove_dir_all(&anonymized) {
            eprintln!("не удалить {}: {e}", anonymized.display());
            return 1;
        }
        println!("[ok] удалён выход анонимизации: {}", anonymized.display());
    }
    // 3. Пустая база со схемой (идемпотентно: open_rw создаёт схему сам).
    match open_conn(&cfg) {
        Ok(conn) => drop(conn),
        Err(e) => {
            eprintln!("не создать пустую базу {}: {}", db.display(), e.message());
            return 1;
        }
    }
    println!("[ok] создана пустая база: {}", db.display());
    // Свежий heartbeat прошлой сессии сбил бы индексатор («уже идёт»).
    let _ = std::fs::remove_file(root.join("index.heartbeat.json"));
    let _ = std::fs::remove_file(root.join("index.stop"));

    if has(rest, "--no-index") {
        println!("[ok] индексация пропущена (--no-index): запустите `hds index` или `hds watch`.");
        return 0;
    }
    // 4. Первичное построение индекса.
    let conn = match open_conn(&cfg) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("не открыть базу {}: {}", db.display(), e.message());
            return 1;
        }
    };
    let emb = build_embedder(&cfg);
    let sidecar = match build_sidecar(&root) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("воркер извлечения: {}", e.message());
            return 1;
        }
    };
    println!("[..] индексация {} проект(ов)…", projects.len());
    let res = hds_index::pipeline::run_index(
        &conn,
        &cfg,
        &emb,
        &sidecar,
        &sidecar,
        &hds_index::RunIndexArgs::default(),
    );
    sidecar.shutdown();
    match res {
        Ok(counters) => {
            println!("[ok] индекс построен: {counters}");
            0
        }
        Err(e) => {
            eprintln!("индексация: {}", e.message());
            1
        }
    }
}

/// `hds agent check [--project <P>] [--json]` — пункты `agent-*` из `hds check`
/// (A6.1); с `--project` — только по одному проекту.
///
/// Коды: 0 — нет `fail`, 1 — есть, 2 — плохие аргументы. Значения секретов и
/// токенов не печатаются (I7, I8) — пункты говорят только «задан/нет».
fn cmd_check(cfg: &Config, args: &[String]) -> i32 {
    let json = has(args, "--json");
    let project = match flag_val(args, "--project") {
        Some(raw) => {
            let id = raw.trim();
            if !is_agent_project(id) {
                eprintln!("agent check: идентификатор проекта — P + 4 цифры (D21): '{id}'");
                return 2;
            }
            Some(id.to_string())
        }
        None => None,
    };
    let checks = hds_index::diag::check_agent_for(cfg, project.as_deref());
    let ok = !checks.iter().any(|c| c.status == "fail");
    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": ok,
                "project": project,
                "checks": crate::cmd::check::checks_json(&checks),
            })
        );
        return if ok { 0 } else { 1 };
    }
    println!("== hermes-disk-search: проверки зоны агента ==");
    if let Some(id) = &project {
        println!("проект: {id}");
    }
    crate::cmd::check::print_checks(&checks);
    println!(
        "Итог: {}",
        if ok {
            "зона готова"
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
