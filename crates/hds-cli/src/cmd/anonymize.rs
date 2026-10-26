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
     \x20 deanon <путь> [--out-dir DIR]         (показать: все маппинги БД)\n\
     \x20 deanon-values <путь> <токен…>         (показать перечисленные токены)\n\
     \x20 inspect <путь>                        (плейсхолдеры/маркер БЕЗ значений)\n\
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
        if a == "--out-dir" {
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
            Act::Deanon => client
                .deanonymize(file, &out_dir, None)
                .map(|r| format!("-> {} (восстановлено: {})", r.out_path, r.restored.len())),
            Act::DeanonValues => client.deanonymize(file, &out_dir, Some(&items)).map(|r| {
                format!(
                    "-> {} (восстановлено: {}, нет в БД: {})",
                    r.out_path,
                    r.restored.len(),
                    r.unmatched.len()
                )
            }),
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

/// `hds anon check` — venv/модели/БД (lease — P3, пока печатается как off).
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
        "[..] gpu: {} (lease у llm_host — фаза P3; сейчас CPU)",
        settings.gpu
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
            } else if p.file_name().and_then(|n| n.to_str()) == Some("model.onnx") {
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
}
