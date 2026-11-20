//! `hds anon` — отладочные команды анонимизации без UI
//! (PLAN_ANONYMIZER_SIDECAR.md §2.4).
//!
//! Подкоманды: `anonymize <path>`, `values <path> <значения…>`,
//! `deanon <path>`, `deanon-values <path> <токены…>`, `inspect <path>`,
//! `warmup`, `check`. Папку перечисляет Rust (форматы — из `hello.formats`,
//! верхний уровень) и вызывает RPC по одному файлу — прогресс и ошибки
//! пофайлово. Значения PII в вывод НЕ печатаются: только имена файлов,
//! токены и счётчики.
//!
//! Флаги: `--out-dir DIR` (иначе `anonymize.output_dir` из конфига).

use std::path::{Path, PathBuf};

use hds_anonymize::{discover_python, list_files, Client, ClientConfig, Settings};
use hds_core::config::{load, project_root, Config};

pub fn cmd_anon(args: &[String]) -> i32 {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    cmd_anon_cfg(&cfg, args)
}

/// То же с явным конфигом (тесты не трогают боевой `config.yaml`).
pub fn cmd_anon_cfg(cfg: &Config, args: &[String]) -> i32 {
    let (sub, rest) = match args.split_first() {
        Some((s, r)) => (s.as_str(), r),
        None => {
            eprintln!("{}", usage());
            return 2;
        }
    };
    match sub {
        "anonymize" => run_act(cfg, rest, Act::Anonymize),
        "values" => run_act(cfg, rest, Act::Values),
        "deanon" => run_act(cfg, rest, Act::Deanon),
        "deanon-values" => run_act(cfg, rest, Act::DeanonValues),
        "inspect" => run_act(cfg, rest, Act::Inspect),
        "agent" => run_act(cfg, rest, Act::Agent),
        "text-image" => run_text_image(cfg, rest),
        "snapshot" => cmd_snapshot(cfg, rest),
        "keep" => cmd_keep(cfg, rest),
        "merge" => cmd_merge(cfg, rest),
        "warmup" => cmd_warmup(cfg),
        "check" => cmd_check(cfg),
        "--help" | "-h" | "help" => {
            println!("{}", usage());
            0
        }
        other => {
            eprintln!("anon: неизвестная подкоманда '{other}'");
            eprintln!("{}", usage());
            2
        }
    }
}

fn usage() -> String {
    "использование: hds anon <подкоманда>\n\
     \x20 anonymize <путь> [--out-dir DIR]      (скрыть: NER по документу)\n\
     \x20 values <путь> <значение…> [--out-dir DIR]  (скрыть перечисленное, без NER)\n\
     \x20 deanon <путь> [--project P] [--out-dir DIR] (показать: маппинги БД/проекта)\n\
     \x20 deanon-values <путь> <токен…>         (показать перечисленные токены)\n\
     \x20 agent <путь> --project P [--artifact ID] [--out-dir DIR]\n\
     \x20                                       (образ анонимной зоны: метки проекта,\n\
     \x20                                        [IMAGE_n], маркер hds-agent/P/ID)\n\
     \x20 text-image <файл> --project P [--artifact ID] [--out-dir DIR]\n\
     \x20                                       (pdf/pptx/mpp → <имя>.<ext>.md, D18)\n\
     \x20 inspect <путь>                        (плейсхолдеры/маркер БЕЗ значений)\n\
     \x20 snapshot <проект>                     (метки проекта: только счётчики)\n\
     \x20 keep [list|add <значение>]            (список «не анонимизировать», D19)\n\
     \x20 merge <проект> <метка> <метка…>      («Объединить метки», TR-2.6)\n\
     \x20 warmup                                (прогрев моделей: backend/providers)\n\
     \x20 check                                 (venv/модели/БД)\n\
       путь — файл или папка (папка: верхний уровень, форматы из hello.formats)"
        .to_string()
}

#[derive(Clone, Copy, PartialEq)]
enum Act {
    Anonymize,
    Values,
    Deanon,
    DeanonValues,
    Inspect,
    Agent,
}

/// Позиционный аргумент (первый, не начинающийся с `--`).
fn positional(args: &[String]) -> Option<String> {
    args.iter().find(|a| !a.starts_with("--")).cloned()
}

fn flag_val(args: &[String], name: &str) -> Option<String> {
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

fn run_act(cfg: &Config, args: &[String], act: Act) -> i32 {
    let path_arg = match positional(args) {
        Some(p) => p,
        None => {
            eprintln!("{}", usage());
            return 2;
        }
    };
    let root = project_root();
    let settings = Settings::from_config(cfg);
    let out_dir = match flag_val(args, "--out-dir") {
        Some(d) => PathBuf::from(d),
        None => settings.output_dir_abs(&root),
    };
    // Уровень 1 (TR-1.5, TR-7.4): проект и id артефакта — необязательные
    let project = flag_val(args, "--project");
    let artifact = flag_val(args, "--artifact");

    // P3 (§2.2): lease у llm_host (`anonymize.gpu: auto`); `off` — всегда CPU.
    // llm_host недоступен → fallback CPU с предупреждением; gpu_busy (бюджет
    // ожидания chat истёк) → акт отклоняется, повтор решает пользователь.
    let mut device = "cpu";
    let mut gpu_limit = 0u64;
    let mut _lease_guard: Option<hds_anonymize::lease::LeaseGrant> = None;
    if settings.gpu != "off" {
        match hds_anonymize::lease::acquire(
            &settings.llm_host_url,
            settings.lease_timeout_sec,
            settings.gpu_reserve_mb,
            std::time::Duration::from_secs(10),
        ) {
            Ok(grant) => {
                println!(
                    "[..] lease 'anon' выдан (лимит ORT {} МиБ) — GLiNER на GPU",
                    grant.gpu_mem_limit_mb
                );
                device = "cuda";
                gpu_limit = grant.gpu_mem_limit_mb;
                _lease_guard = Some(grant); // Drop → /internal/anon/exit
            }
            Err(hds_anonymize::lease::LeaseError::Unavailable(m)) => {
                println!("[warn] lease недоступен ({m}) — GLiNER на CPU (fallback)");
            }
            Err(busy) => {
                let err = hds_anonymize::lease::to_anonymize_error(&busy);
                eprintln!("[!!] lease: {} ({})", err, err.kind.as_str());
                return 1;
            }
        }
    }
    // значения/токены — позиционные аргументы после пути (значения флагов
    // вида `--out-dir DIR` не попадают в список)
    let mut items: Vec<String> = Vec::new();
    let mut skip_next = false;
    let mut path_seen = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--out-dir" || a == "--project" || a == "--artifact" {
            skip_next = true;
            continue;
        }
        if a.starts_with("--") {
            continue;
        }
        if !path_seen {
            path_seen = true; // первый позиционный — путь
            continue;
        }
        items.push(a.clone());
    }
    match act {
        Act::Values | Act::DeanonValues if items.is_empty() => {
            eprintln!("нужен хотя бы один элемент списка");
            return 2;
        }
        _ => {}
    }

    let python = match discover_python(&root) {
        Some(p) => p,
        None => {
            eprintln!(
                "[!!] не найден интерпретатор воркера анонимизации \
                 (HDS_ANONYMIZE_PYTHON / sidecar\\venv-anonymize / sidecar\\python)"
            );
            return 1;
        }
    };
    let client_cfg = {
        let mut c = ClientConfig::from_settings(&python, &root, &settings);
        c.extra_args = settings.worker_args_for(&root, device, gpu_limit);
        c
    };
    let mut client = match Client::spawn(client_cfg) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[!!] воркер: {}", e);
            return 1;
        }
    };

    // Папку перечисляем сами (§3); форматы — из hello воркера.
    let path = PathBuf::from(&path_arg);
    let files: Vec<PathBuf> = if path.is_dir() {
        let formats = match client.hello() {
            Ok(h) => h.formats,
            Err(e) => {
                eprintln!("[!!] hello: {}", e);
                return 1;
            }
        };
        let files = list_files(&path, &formats);
        if files.is_empty() {
            eprintln!(
                "[!!] в папке нет файлов поддерживаемых форматов ({:?})",
                formats
            );
            return 1;
        }
        println!("[..] файлов в папке: {}", files.len());
        files
    } else {
        vec![path]
    };

    let mut failures = 0usize;
    for file in &files {
        let res = match act {
            Act::Anonymize => client.anonymize(file, &out_dir, None).map(|r| {
                format!(
                    "-> {} (токенов: {})",
                    r.out_path,
                    r.tokens.iter().map(|t| t.count).sum::<i64>()
                )
            }),
            Act::Values => client.anonymize(file, &out_dir, Some(&items)).map(|r| {
                format!(
                    "-> {} (токенов: {}, ненайдено: {})",
                    r.out_path,
                    r.tokens.iter().map(|t| t.count).sum::<i64>(),
                    r.unmatched.len()
                )
            }),
            Act::Deanon => match &project {
                Some(project) => client
                    .deanonymize_project(file, &out_dir, project, None)
                    .map(|r| {
                        format!(
                            "-> {} (восстановлено: {}, нет в проекте: {})",
                            r.out_path,
                            r.restored.len(),
                            r.unmatched.len()
                        )
                    }),
                None => client
                    .deanonymize(file, &out_dir, None)
                    .map(|r| format!("-> {} (восстановлено: {})", r.out_path, r.restored.len())),
            },
            Act::DeanonValues => client.deanonymize(file, &out_dir, Some(&items)).map(|r| {
                format!(
                    "-> {} (восстановлено: {}, нет в БД: {})",
                    r.out_path,
                    r.restored.len(),
                    r.unmatched.len()
                )
            }),
            Act::Agent => {
                let project = match project.clone() {
                    Some(p) if !p.is_empty() => p,
                    _ => {
                        eprintln!("[!!] agent: нужен --project P0001");
                        return 2;
                    }
                };
                client
                    .anonymize_agent(file, &out_dir, &project, artifact.as_deref())
                    .map(|r| {
                        format!(
                            "-> {} (режим {}, меток: {}, картинок: {}, предупр.: {})",
                            r.out_path,
                            r.mode,
                            r.tokens.len(),
                            r.images,
                            r.warnings.len()
                        )
                    })
            }
            Act::Inspect => client.inspect(file).map(|info| {
                let mut lines = vec![format!(
                    "supported={} marker={:?} плейсхолдеров: {}",
                    info.supported,
                    info.marker_version,
                    info.placeholders.len()
                )];
                for p in &info.placeholders {
                    let in_db = info
                        .resolved
                        .iter()
                        .find(|r| r.token == p.token)
                        .map(|r| r.in_db)
                        .unwrap_or(false);
                    lines.push(format!(
                        "  {} type={} count={} in_db={}",
                        p.token, p.entity_type, p.count, in_db
                    ));
                }
                lines.join("\n")
            }),
        };
        match res {
            Ok(text) => println!("[ok] {} {}", file.display(), text),
            Err(e) => {
                failures += 1;
                eprintln!("[!!] {}: {} ({})", file.display(), e, e.kind.as_str());
            }
        }
    }
    client.shutdown();
    if failures > 0 {
        eprintln!("[..] неудач: {} из {}", failures, files.len());
        return 1;
    }
    0
}

/// Позиционные аргументы без значений флагов (для новых подкоманд).
fn positionals(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg == "--out-dir" || arg == "--project" || arg == "--artifact" {
            skip = true;
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

/// Клиент воркера анонимизации (общий запуск для новых подкоманд).
fn spawn_client(settings: &Settings, root: &Path) -> Option<Client> {
    let python = match discover_python(root) {
        Some(p) => p,
        None => {
            eprintln!(
                "[!!] не найден интерпретатор воркера анонимизации \
                 (HDS_ANONYMIZE_PYTHON / sidecar\\venv-anonymize / sidecar\\python)"
            );
            return None;
        }
    };
    match Client::spawn(ClientConfig::from_settings(&python, root, settings)) {
        Ok(client) => Some(client),
        Err(e) => {
            eprintln!("[!!] воркер: {}", e);
            None
        }
    }
}

/// Текст pdf/pptx/mpp через `hds_extract` (mpxj/Java, PyMuPDF, python-pptx).
/// None — извлечение недоступно (воркер попробует сам или отдаст ошибку).
fn extract_text_by_worker(root: &Path, path: &Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !matches!(ext.as_str(), "pdf" | "pptx" | "mpp" | "mpt" | "mpx") {
        return None;
    }
    let python = hds_extract::discover_python(root)?;
    let mut config = hds_extract::WorkerConfig::new(&python, root);
    config.idle_timeout = std::time::Duration::from_secs(120);
    let mut worker = match hds_extract::Worker::spawn(config) {
        Ok(worker) => worker,
        Err(err) => {
            eprintln!(
                "[warn] hds_extract недоступен ({}): текст извлечёт воркер",
                err
            );
            return None;
        }
    };
    let result = worker.extract(path);
    worker.shutdown();
    match result {
        Ok(parsed) => {
            let text = parsed
                .segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
        Err(err) => {
            eprintln!(
                "[warn] извлечение текста не удалось ({}): пробует воркер",
                err
            );
            None
        }
    }
}

/// `hds anon text-image <файл> --project P` — текстовый образ (D18, TR-7.3a).
fn run_text_image(cfg: &Config, args: &[String]) -> i32 {
    let items = positionals(args);
    let path = match items.first() {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("{}", usage());
            return 2;
        }
    };
    let project = match flag_val(args, "--project") {
        Some(p) if !p.is_empty() => p,
        _ => {
            eprintln!("[!!] text-image: нужен --project P0001");
            return 2;
        }
    };
    let artifact = flag_val(args, "--artifact");
    let root = project_root();
    let settings = Settings::from_config(cfg);
    let out_dir = match flag_val(args, "--out-dir") {
        Some(dir) => PathBuf::from(dir),
        None => settings.output_dir_abs(&root),
    };
    let mut client = match spawn_client(&settings, &root) {
        Some(client) => client,
        None => return 1,
    };
    let text = extract_text_by_worker(&root, &path);
    let result = client.text_image(
        &path,
        &out_dir,
        &project,
        artifact.as_deref(),
        text.as_deref(),
    );
    client.shutdown();
    match result {
        Ok(image) => {
            println!(
                "[ok] {} -> {} (меток: {}, маркер: {})",
                path.display(),
                image.out_path,
                image.labels.len(),
                image.marker
            );
            0
        }
        Err(err) => {
            eprintln!("[!!] {}: {} ({})", path.display(), err, err.kind.as_str());
            1
        }
    }
}

/// `hds anon snapshot <проект>` — счётчики меток проекта, БЕЗ значений (TR-1.6).
fn cmd_snapshot(cfg: &Config, args: &[String]) -> i32 {
    let items = positionals(args);
    let project = match items.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("{}", usage());
            return 2;
        }
    };
    let root = project_root();
    let settings = Settings::from_config(cfg);
    let mut client = match spawn_client(&settings, &root) {
        Some(client) => client,
        None => return 1,
    };
    let result = client.snapshot(&project);
    client.shutdown();
    match result {
        Ok(snapshot) => {
            let forms: usize = snapshot.labels.iter().map(|l| l.forms.len()).sum();
            println!(
                "[ok] {}: rev={} меток={} форм={} прочих-форм={} keep={}",
                project,
                snapshot.rev,
                snapshot.labels.len(),
                forms,
                snapshot.others.len(),
                snapshot.keep.len()
            );
            0
        }
        Err(err) => {
            eprintln!("[!!] snapshot: {} ({})", err, err.kind.as_str());
            1
        }
    }
}

/// `hds anon keep [list|add <значение>]` — список «не анонимизировать» (D19).
fn cmd_keep(cfg: &Config, args: &[String]) -> i32 {
    let items = positionals(args);
    let action = items.first().map(|s| s.as_str()).unwrap_or("list");
    let root = project_root();
    let settings = Settings::from_config(cfg);
    let mut client = match spawn_client(&settings, &root) {
        Some(client) => client,
        None => return 1,
    };
    if action == "add" {
        let value = match items.get(1) {
            Some(value) => value.clone(),
            None => {
                eprintln!("[!!] keep add: нужно значение");
                client.shutdown();
                return 2;
            }
        };
        let result = client.add_keep_term(&value);
        client.shutdown();
        return match result {
            Ok(added) => {
                println!(
                    "[ok] keep_terms: {} ({})",
                    value,
                    if added {
                        "добавлено"
                    } else {
                        "уже был"
                    }
                );
                0
            }
            Err(err) => {
                eprintln!("[!!] keep_terms: {} ({})", err, err.kind.as_str());
                1
            }
        };
    }
    if action != "list" {
        eprintln!("[!!] keep: неизвестное действие {action:?} (list|add)");
        client.shutdown();
        return 2;
    }
    let result = client.keep_terms();
    client.shutdown();
    match result {
        Ok(terms) => {
            println!("[ok] keep_terms: {}", terms.len());
            for term in terms {
                println!("  {}", term);
            }
            0
        }
        Err(err) => {
            eprintln!("[!!] keep_terms: {} ({})", err, err.kind.as_str());
            1
        }
    }
}

/// `hds anon merge <проект> <метка> <метка…>` — «Объединить метки» (TR-2.6).
fn cmd_merge(cfg: &Config, args: &[String]) -> i32 {
    let items = positionals(args);
    if items.len() < 3 {
        eprintln!("[!!] merge: нужно <проект> <метка> <метка…>");
        eprintln!("{}", usage());
        return 2;
    }
    let project = items[0].clone();
    let keep = items[1].clone();
    let drop: Vec<String> = items[2..].to_vec();
    let root = project_root();
    let settings = Settings::from_config(cfg);
    let mut client = match spawn_client(&settings, &root) {
        Some(client) => client,
        None => return 1,
    };
    let result = client.merge_entities(&project, &keep, &drop);
    client.shutdown();
    match result {
        Ok(answer) => {
            let dropped = if answer.dropped.is_empty() {
                String::new()
            } else {
                answer.dropped.join(", ")
            };
            println!(
                "[ok] {}: {} (псевдонимы: {}{})",
                project,
                keep,
                dropped,
                match answer.merge_id {
                    Some(id) => format!("; склейка №{id}"),
                    None => String::new(),
                }
            );
            0
        }
        Err(err) => {
            eprintln!("[!!] merge: {} ({})", err, err.kind.as_str());
            1
        }
    }
}

/// `hds anon warmup` — прогрев моделей, печатает backend/providers.
fn cmd_warmup(cfg: &Config) -> i32 {
    let root = project_root();
    let settings = Settings::from_config(cfg);
    let python = match discover_python(&root) {
        Some(p) => p,
        None => {
            eprintln!("[!!] интерпретатор воркера не найден");
            return 1;
        }
    };
    let mut client = match Client::spawn(ClientConfig::from_settings(&python, &root, &settings)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[!!] воркер: {}", e);
            return 1;
        }
    };
    match client.warmup() {
        Ok(w) => {
            println!(
                "[ok] backend: {} providers: {:?} ({} мс)",
                w.backend, w.providers, w.load_ms
            );
            client.shutdown();
            0
        }
        Err(e) => {
            eprintln!("[!!] warmup: {}", e);
            client.shutdown();
            1
        }
    }
}

/// `hds anon check` — venv/модели/БД + адрес lease (`anonymize.gpu`,
/// `anonymize.llm_host_url`).
fn cmd_check(cfg: &Config) -> i32 {
    let root = project_root();
    let settings = Settings::from_config(cfg);
    let mut failed = false;

    match discover_python(&root) {
        Some(p) => println!("[ok] python: {}", p.display()),
        None => {
            println!("[!!] python: не найден (HDS_ANONYMIZE_PYTHON / venv-anonymize)");
            failed = true;
        }
    }

    let models = hds_anonymize::settings::models_dir(&root);
    let navec = models.join("navec_news_v1_1B_250K_300d_100q.tar");
    let slovnet = models.join("slovnet_ner_news_v1.tar");
    for (name, path) in [("модели navec", &navec), ("модели slovnet", &slovnet)] {
        if path.is_file() {
            println!("[ok] {}: {}", name, path.display());
        } else {
            println!("[!!] {}: нет ({})", name, path.display());
            failed = true;
        }
    }
    let hf = models.join("hf-home");
    if has_gliner_weights(&hf) {
        println!("[ok] модели GLiNER: {}", hf.display());
    } else {
        println!("[!!] модели GLiNER: нет model.onnx в {}", hf.display());
        failed = true;
    }

    let db = settings.db_abs(&root);
    if db.is_file() {
        println!("[ok] БД токенов: {}", db.display());
    } else {
        println!(
            "[warn] БД токенов: не создана ({}) — будет создана первым актом",
            db.display()
        );
    }

    println!(
        "[..] gpu: {} (lease у llm_host: {}; недоступен — CPU fallback)",
        settings.gpu, settings.llm_host_url
    );

    if failed {
        1
    } else {
        0
    }
}

/// Рекурсивный поиск model.onnx в HF-кэше (GLiNER PII).
fn has_gliner_weights(dir: &Path) -> bool {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("onnx"))
                .unwrap_or(false)
            {
                // Имя не фиксировано: GLiNER PII публикует квантованный
                // onnx/model_quint8.onnx (и fp32 onnx/model.onnx).
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_and_unknown_subcommand() {
        assert_eq!(cmd_anon_cfg(&Config::Null, &[]), 2);
        assert_eq!(cmd_anon_cfg(&Config::Null, &["help".to_string()]), 0);
        assert_eq!(cmd_anon_cfg(&Config::Null, &["bogus".to_string()]), 2);
    }

    #[test]
    fn positional_skips_flags() {
        let args = vec![
            "a.txt".to_string(),
            "--out-dir".to_string(),
            "D:/out".to_string(),
            "Иван".to_string(),
        ];
        assert_eq!(positional(&args).as_deref(), Some("a.txt"));
        assert_eq!(flag_val(&args, "--out-dir").as_deref(), Some("D:/out"));
    }

    #[test]
    fn positionals_skip_flag_values() {
        let args = vec![
            "add".to_string(),
            "1С:ERP".to_string(),
            "--project".to_string(),
            "P0001".to_string(),
            "--artifact".to_string(),
            "42".to_string(),
        ];
        assert_eq!(
            positionals(&args),
            vec!["add".to_string(), "1С:ERP".to_string()]
        );
    }

    #[test]
    fn new_subcommands_in_usage_and_require_args() {
        let text = usage();
        for command in ["agent", "text-image", "snapshot", "keep", "merge"] {
            assert!(text.contains(command), "нет {command} в usage");
        }
        // без обязательных аргументов — код 2 и никаких процессов воркера
        assert_eq!(cmd_anon_cfg(&Config::Null, &["snapshot".to_string()]), 2);
        assert_eq!(cmd_anon_cfg(&Config::Null, &["merge".to_string()]), 2);
        assert_eq!(cmd_anon_cfg(&Config::Null, &["text-image".to_string()]), 2);
    }
}
