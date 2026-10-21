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
    // W5: backend/EP/модель Nemotron-3 — те же числа и тот же выбор, что у хоста.
    let ds = hds_index::diag::diarization_summary(cfg);
    if json {
        let out = serde_json::json!({
            "engine": engine.map(|p| p.display().to_string()),
            "model": model.map(|p| p.display().to_string()),
            // legacy sortformer — ключ сохранён для совместимости.
            "diarization_model": diar.as_ref().map(|p| p.display().to_string()),
            // W5 (аддитивные ключи).
            "diarization_backend": ds.backend.as_str(),
            "diarization_configured": ds.configured,
            "diarization_device": ds.device,
            "diarization_device_auto": ds.device_auto,
            "nemotron_model": ds.nemotron_model.as_ref().map(|p| p.display().to_string()),
            "nemotron_ok": ds.nemotron_ok(),
            "nemotron_error": match &ds.nemotron {
                Some(Err(e)) => Some(e.clone()),
                _ => None,
            },
            "diarization_warnings": ds.warnings,
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
    print_diarization(&hds_index::diag::diarization_summary(cfg));
}

/// W5: backend диаризации, EP, модель Nemotron-3 (путь, размер, sha256) и legacy.
///
/// T5 сохраняется: для `mode: transcript` модель обязательна (жёсткий отказ без
/// неё), поэтому отсутствие модели — `[--]`/`[!!]`, а не тишина.
fn print_diarization(s: &hds_index::diag::DiarSummary) {
    use hds_index::diag::DiarBackend;
    let device = format!("EP {} (auto → {})", s.device, s.device_auto);
    match s.backend {
        DiarBackend::Off => println!("[--] диаризация: off (выключена в конфиге, {device})"),
        DiarBackend::Sortformer => {
            println!("[--] диаризация: sortformer (legacy), {device}");
            match s.sortformer_model.as_deref() {
                Some(p) => println!("     sortformer-модель: {}", p.display()),
                None => println!(
                    "     sortformer-модель не найдена — автотранскрибация упадёт \
                     (installers\\fetch_diarization_model.ps1)"
                ),
            }
        }
        DiarBackend::Nemotron3 => {
            println!("[ok] диаризация: nemotron3 (Sortformer v3), {device}");
            match (&s.nemotron_model, &s.nemotron) {
                (Some(p), Some(Ok(()))) => println!(
                    "     модель: {} ({:.0} МБ, sha256 ok)",
                    p.display(),
                    size_mb(p)
                ),
                (Some(p), Some(Err(e))) => {
                    println!("[!!] модель {} не прошла проверку: {e}", p.display());
                }
                _ => println!(
                    "[--] модель Nemotron-3 не найдена — автотранскрибация упадёт \
                     (installers\\fetch_nemotron3_model.ps1)"
                ),
            }
            if !s.ffmpeg {
                println!("[--] ffmpeg не найден — WAV для диаризации декодировать нечем");
            }
        }
    }
    for w in &s.warnings {
        println!("     [!] {w}");
    }
}

/// Размер файла в МБ (0, если не читается) — для строки о модели.
fn size_mb(p: &Path) -> f64 {
    std::fs::metadata(p)
        .map(|m| m.len() as f64 / 1_048_576.0)
        .unwrap_or(0.0)
}

/// Косметика таймкода: `12.345` → `12.35`; `None` → `?`.
fn fmt_t(t: Option<f64>) -> String {
    match t {
        Some(v) => format!("{v:.2}"),
        None => "?".to_string(),
    }
}
