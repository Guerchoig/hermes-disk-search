//! `hds transcribe-watch` / `hds transcribe-once` / `hds transcribe-stop` —
//! конвейер автотранскрибации (`PLAN_AUTO_TRANSCRIBE` §4, вариант A: демон —
//! отдельная подкоманда со своим `transcribe.lock`).
//!
//! Владельца GPU не поднимаем и движок не грузим: работаем только по HTTP
//! (`POST /internal/transcribe`), как `hds watch` ходит в медиа-ветку (§8.6.2 —
//! второй владелец GPU недопустим).

use std::path::Path;

use hds_core::config::{load, project_root};
use hds_index::{run_transcribe_watch, transcribe_once, AutoTranscribeConfig};

/// Конфиг конвейера из `config.yaml` (или сообщение об ошибке загрузки).
fn load_cfg() -> Result<AutoTranscribeConfig, String> {
    let cfg = load().map_err(|e| format!("конфиг: {}", e.message()))?;
    Ok(AutoTranscribeConfig::from_config(&cfg))
}

/// Демон `hds transcribe-watch`: `inbox_dir` → `out_dir` (§4).
pub fn cmd_transcribe_watch() -> i32 {
    let at = match load_cfg() {
        Ok(at) => at,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if !at.enabled {
        println!("auto_transcribe.enabled: false — конвейер выключен.");
        println!(
            "Включите в config.yaml: auto_transcribe.enabled: true, inbox_dir, out_dir \
             (см. config.example.yaml)."
        );
        return 0;
    }
    match run_transcribe_watch(&at) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Ошибка: {}", e.message());
            1
        }
    }
}

/// Обёртка для CLI: `--help/-h` → справка без запуска; `--dry-run` → проверка конфига;
/// без флагов — обычный запуск демона (R3.3, PLAN_TRANSCRIBE_SERVICE).
pub fn cmd_transcribe_watch_cli(args: &[String]) -> i32 {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--help") || has("-h") {
        print_watch_usage();
        return 0;
    }
    if has("--dry-run") {
        return cmd_transcribe_watch_dry_run();
    }
    cmd_transcribe_watch()
}

/// Справка подкоманды `hds transcribe-watch`.
fn print_watch_usage() {
    println!(
        "hds transcribe-watch — демон автотранскрибации+диаризации (inbox_dir → out_dir)\n\
         \n\
         Использование:\n\
           hds transcribe-watch             запустить демон (блокирующе)\n\
           hds transcribe-watch --dry-run   проверить конфиг/каталоги/модели, ничего не делать\n\
           hds transcribe-watch --help      эта справка\n\
         \n\
         Настройки — секция `auto_transcribe` в config.yaml (см. config.example.yaml).\n\
         Владельца GPU (`llm-host`) демон не поднимает: ходит по HTTP /internal/transcribe."
    );
}

/// `--dry-run`: проверка конфига/каталогов/моделей без запуска (R3.3).
fn cmd_transcribe_watch_dry_run() -> i32 {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let dirs = hds_index::diag::check_auto_transcribe_dirs(&cfg);
    let dia = hds_index::diag::check_diarization(&cfg);
    let mut fail = false;
    for c in [&dirs, &dia] {
        println!("[{}] {}: {}", c.status, c.title, c.msg);
        if c.status == "fail" {
            fail = true;
        }
    }
    let at = AutoTranscribeConfig::from_config(&cfg);
    println!(
        "enabled={} inbox={:?} out={:?} source_disposal={:?} diarization_required={}",
        at.enabled,
        at.inbox_dir,
        at.out_dir,
        at.source_disposal,
        at.transcribe.diarization.required
    );
    if !at.enabled {
        println!("auto_transcribe.enabled: false — демон не будет обрабатывать файлы.");
    }
    if fail {
        1
    } else {
        0
    }
}

/// Один файл «здесь и сейчас»: `hds transcribe-once <медиафайл>`.
///
/// Требует заданного `auto_transcribe.out_dir` (куда писать) и владельца GPU —
/// иначе вернёт понятную ошибку. Очередь и `source_disposal` не участвуют.
pub fn cmd_transcribe_once(file: &str) -> i32 {
    let at = match load_cfg() {
        Ok(at) => at,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let path = Path::new(file);
    if !path.is_file() {
        eprintln!("файл не найден: {file}");
        return 1;
    }
    match transcribe_once(&at, path) {
        Ok(out) => {
            println!("готово: {}", out.out_path.display());
            println!("  исходный текст (до имён): {}", out.orig_path.display());
            println!("  карта спикеров: {}", out.speakers_path.display());
            println!(
                "  метки: {}",
                if out.speakers.is_empty() {
                    "—".to_string()
                } else {
                    out.speakers.join(", ")
                }
            );
            println!("  символов: {}", out.chars);
            0
        }
        Err(e) => {
            eprintln!("Ошибка: {}", e.message());
            println!(
                "Проверьте: владелец GPU поднят (`hds status`), `auto_transcribe.out_dir` \
                 задан, sortformer-модель установлена (диаризация обязательна)."
            );
            1
        }
    }
}

/// Мягкая остановка демона: файл `transcribe.stop` (§4) — аналог `index.stop`.
pub fn cmd_transcribe_stop() -> i32 {
    let p = project_root().join("transcribe.stop");
    match std::fs::write(&p, b"") {
        Ok(()) => {
            println!("Запрошена остановка: {}", p.display());
            println!("Демон завершится в течение ~1 с (снимает transcribe.lock сам).");
            0
        }
        Err(e) => {
            eprintln!("не создать {}: {e}", p.display());
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::cmd_transcribe_watch_cli;

    /// R3.3 (PLAN_TRANSCRIBE_SERVICE): `--help`/`-h` у `transcribe-watch` печатают
    /// справку и выходят с кодом 0, **не запуская** демон (баг из отчёта).
    #[test]
    fn watch_cli_help_exits_without_running() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(cmd_transcribe_watch_cli(&a(&["--help"])), 0);
        assert_eq!(cmd_transcribe_watch_cli(&a(&["-h"])), 0);
    }
}
