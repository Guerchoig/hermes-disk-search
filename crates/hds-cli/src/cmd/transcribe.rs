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
