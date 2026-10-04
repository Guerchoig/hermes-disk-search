//! `hds whisper-check` — проверка медиа-ветки W3 (порт `hds/cli.py::cmd_whisper_check`
//! по смыслу): каталог движка, whisper-модель и — при `--file` — **живая**
//! транскрибация через владельца GPU (`llm-host`, `POST /internal/transcribe`).
//!
//! Отличие от Python: модель теперь GGML `.bin` движка (не папка faster-whisper),
//! поэтому «загрузка/проверка» = разрешение пути ([`resolve_whisper_model`]) плюс
//! живой прогон. Владельца портов не поднимаем — только по HTTP (`§8.6.2`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use hds_core::config::{dig, load, Config};
use hds_index::transcribe::{resolve_whisper_model, TranscribeClient, TranscribeConfig};

/// Каталог движка: `index.whisper_engine_dir` → `%APPDATA%\OpenResearchTools\
/// TranscribeOffline\Engine` (совместимо с `hds-llama::engine_dir`).
fn engine_dir(cfg: &Config) -> Option<PathBuf> {
    if let Some(s) = dig(cfg, "index.whisper_engine_dir").and_then(|v| v.as_str()) {
        let p = PathBuf::from(s.trim());
        if p.is_dir() {
            return Some(p);
        }
    }
    let appdata = std::env::var("APPDATA").ok()?;
    let p = PathBuf::from(appdata)
        .join("OpenResearchTools")
        .join("TranscribeOffline")
        .join("Engine");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// `cmd_whisper_check(file, json)`: 0 — движок и модель на месте (и, если задан
/// `--file`, транскрибация успешна); 1 — иначе. Конфиг — из `HDS_CONFIG`/проекта.
pub fn cmd_whisper_check(file: Option<String>, json: bool) -> i32 {
    let cfg = match load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    cmd_whisper_check_cfg(&cfg, file, json)
}

/// То же с явным конфигом (тесты не трогают боевой `config.yaml`).
pub fn cmd_whisper_check_cfg(cfg: &Config, file: Option<String>, json: bool) -> i32 {
    let engine = engine_dir(cfg);
    let model = resolve_whisper_model(cfg);

    let Some(file) = file else {
        return report_static(cfg, engine.as_deref(), model.as_deref(), json);
    };

    let client = TranscribeClient::new(TranscribeConfig::from_config(cfg));
    let t0 = Instant::now();
    let res = client.transcribe(Path::new(&file));
    let elapsed = t0.elapsed().as_secs_f64();
    match res {
        Ok(segments) => {
            if json {
                let segs: Vec<serde_json::Value> = segments
                    .iter()
                    .map(|s| {
                        serde_json::json!({"text": s.text, "t_start": s.t_start, "t_end": s.t_end})
                    })
                    .collect();
                let out = serde_json::json!({
                    "engine": engine.as_ref().map(|p| p.display().to_string()),
                    "model": model.as_ref().map(|p| p.display().to_string()),
                    "file": file,
                    "ok": true,
                    "segments": segs,
                    "elapsed_sec": elapsed,
                });
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            } else {
                print_static(cfg, engine.as_deref(), model.as_deref());
                println!(
                    "[ok] транскрибация «{}»: {} сегментов за {:.2} с",
                    file,
                    segments.len(),
                    elapsed
                );
                for s in &segments {
                    println!("  [{} -> {}] {}", fmt_t(s.t_start), fmt_t(s.t_end), s.text);
                }
            }
            0
        }
        Err(e) => {
            if json {
                let out = serde_json::json!({
                    "engine": engine.as_ref().map(|p| p.display().to_string()),
                    "model": model.as_ref().map(|p| p.display().to_string()),
                    "file": file,
                    "ok": false,
                    "error": e.message(),
                });
                println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            } else {
                print_static(cfg, engine.as_deref(), model.as_deref());
                println!("[!!] транскрибация «{}»: {}", file, e.message());
                println!(
                    "      Владелец GPU поднят? Проверьте: llm-host status; адрес — \
                     index.transcribe_url ({}).",
                    client.config().url
                );
            }
            1
        }
    }
}

/// Отчёт без `--file`: наличие движка/модели; код 0, если модель найдена.
fn report_static(cfg: &Config, engine: Option<&Path>, model: Option<&Path>, json: bool) -> i32 {
    let ok = model.is_some();
    let diar = hds_index::diarization_model_for(cfg);
    if json {
        let out = serde_json::json!({
            "engine": engine.map(|p| p.display().to_string()),
            "model": model.map(|p| p.display().to_string()),
            "diarization_model": diar.as_ref().map(|p| p.display().to_string()),
            "ok": ok,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        print_static(cfg, engine, model);
        if ok {
            println!("[ok] whisper-модель и движок готовы к транскрибации");
        } else {
            println!(
                "[!!] whisper-модель не найдена. Задайте index.whisper_model (путь к \
                 GGML .bin) или установите модель движка (кнопка «Скачать модель»)."
            );
        }
    }
    if ok {
        0
    } else {
        1
    }
}

/// Печать строк о движке/моделях (человеческий формат).
fn print_static(cfg: &Config, engine: Option<&Path>, model: Option<&Path>) {
    match engine {
        Some(p) => println!("[ok] движок: {}", p.display()),
        None => println!("[--] движок не найден (index.whisper_engine_dir / %APPDATA%)"),
    }
    match model {
        Some(p) => println!("[ok] модель: {}", p.display()),
        None => println!("[--] whisper-модель не найдена"),
    }
    // T5: диаризация обязательна для `mode: transcript` (жёсткий отказ без модели)
    match hds_index::diarization_model_for(cfg) {
        Some(p) => println!("[ok] модель диаризации: {}", p.display()),
        None => println!(
            "[--] модель диаризации (sortformer) не найдена — автотранскрибация упадёт \
             (installers\\fetch_diarization_model.ps1)"
        ),
    }
}

/// Косметика таймкода: `12.345` → `12.35`; `None` → `?`.
fn fmt_t(t: Option<f64>) -> String {
    match t {
        Some(v) => format!("{v:.2}"),
        None => "?".to_string(),
    }
}
