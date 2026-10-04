//! T0.1 (PLAN_AUTO_TRANSCRIBE): спайк офлайн-диаризации движка.
//!
//! Прогоняет движок в режиме `mode: transcript` (транскрибация whisper +
//! диаризация sortformer **одним** вызовом bridge-audio) и печатает **точный**
//! формат вывода: метку спикера, метку неприсвоенной реплики, расширение файла,
//! таймкоды. Нужен, чтобы зафиксировать §5.3–§5.4 плана до реализации.
//!
//! Запуск:
//! `cargo run -p hds-llama --release --bin diar_probe -- [<audio>] [<whisper.bin>] [<sortformer.gguf>] [<gpu>]`
//! Переменные: `HDS_DIAR_BACKEND` (`sortformer`), `HDS_DIAR_FEED_MS` (`10800001`),
//! `HDS_DIAR_DEVICE` (`CUDA<gpu>`), `HDS_WHISPER_CUSTOM` (`auto`).
//!
//! Использует **только публичный** bridge-API (как `audio_probe`), поэтому не
//! требует изменений в `whisper.rs` — это чистый спайк.

use std::path::{Path, PathBuf};

use serde_json::json;

use hds_llama::error::{EngineError, Result};
use hds_llama::{BridgeAudio, Engine};

/// Корень репозитория (для фикстур по умолчанию).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// Первый `*.bin`/`*.gguf` в каталоге (модель whisper из общего каталога движка).
fn first_model_in(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("bin") || x.eq_ignore_ascii_case("gguf"))
                .unwrap_or(false)
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Модель из `%APPDATA%\OpenResearchTools\models\<имя_каталога>`.
fn model_in_models_dir(dir_name: &str) -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    first_model_in(
        &base
            .config_dir()
            .join("OpenResearchTools")
            .join("models")
            .join(dir_name),
    )
}

fn main() -> Result<()> {
    let mut it = std::env::args().skip(1);
    let audio = it
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("test_data").join("jfk.wav"));
    let whisper_model = it
        .next()
        .map(PathBuf::from)
        .or_else(|| model_in_models_dir("openresearchtools__whisper-large-v3-turbo-GGML"))
        .ok_or_else(|| EngineError::Other("не найдена whisper-модель (*.bin/*.gguf)".into()))?;
    let diar_model = it
        .next()
        .map(PathBuf::from)
        .or_else(|| {
            model_in_models_dir("openresearchtools__diar_streaming_sortformer_4spk-v2.1-gguf")
        })
        .ok_or_else(|| EngineError::Other("не найдена sortformer-модель (*.gguf)".into()))?;
    let gpu: i32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let custom = std::env::var("HDS_WHISPER_CUSTOM").unwrap_or_else(|_| "auto".to_string());
    let backend = std::env::var("HDS_DIAR_BACKEND").unwrap_or_else(|_| "sortformer".to_string());
    let feed_ms: f64 = std::env::var("HDS_DIAR_FEED_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_800_001.0);
    let device = std::env::var("HDS_DIAR_DEVICE").unwrap_or_else(|_| format!("CUDA{gpu}"));

    let engine = Engine::open(None)?;
    let _cwd = engine.activate()?;
    println!("engine:  {}", engine.dir().display());
    println!("audio:   {}", audio.display());
    println!("whisper: {}", whisper_model.display());
    println!("diar:    {}", diar_model.display());
    println!("gpu(bridge index): {gpu}");

    // ASCII-стейджинг: движок не любит не-ASCII пути (баг подтверждён в W3).
    let ascii_dir = std::env::temp_dir().join("hds_diar");
    std::fs::create_dir_all(&ascii_dir)
        .map_err(|e| EngineError::Other(format!("{ascii_dir:?}: {e}")))?;
    let ext = audio
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("wav")
        .to_ascii_lowercase();
    let staged = ascii_dir.join(format!("input.{ext}"));
    std::fs::copy(&audio, &staged).map_err(|e| EngineError::Other(format!("{staged:?}: {e}")))?;
    let bytes =
        std::fs::read(&staged).map_err(|e| EngineError::Other(format!("{staged:?}: {e}")))?;

    let mut meta = json!({
        "mode": "transcript",
        "custom": custom,
        "whisper_model": whisper_model.to_string_lossy(),
        "output_dir": ascii_dir.to_string_lossy(),
        "audio_source_path": staged.to_string_lossy(),
        "diarization_model_path": diar_model.to_string_lossy(),
        "diarization_backend": backend,
        "diarization_feed_ms": feed_ms,
        "diarization_device": device,
    });
    if gpu >= 0 {
        meta["whisper_gpu_device"] = json!(gpu);
    } else {
        meta["whisper_no_gpu"] = json!(true);
    }

    let bridge_audio = BridgeAudio::load(engine.dir())?;
    let bridge = bridge_audio.create(None, Some(gpu), -1)?;

    println!("--- metadata ---\n{meta}");
    let t0 = std::time::Instant::now();
    let out = bridge.transcribe_raw(&bytes, &ext, &meta.to_string(), true)?;
    println!(
        "--- ({:.1} с) ok={} status={} ---",
        t0.elapsed().as_secs_f64(),
        out.ok,
        out.status
    );
    println!("--- json ---\n{}", out.json);
    if !out.ok {
        println!("--- error ---\n{}", out.error);
        return Err(EngineError::Other(format!(
            "движок вернул ok=0: {}",
            out.error
        )));
    }

    let v: serde_json::Value = serde_json::from_str(&out.json).unwrap_or(serde_json::Value::Null);
    if let Some(p) = v
        .get("output")
        .and_then(|o| o.get("path"))
        .and_then(|p| p.as_str())
    {
        println!("--- output.path ---\n{p}");
        match std::fs::read_to_string(p) {
            Ok(text) => {
                println!(
                    "--- output.ext ---\n{}",
                    Path::new(p)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                );
                println!("--- output.content.begin ---\n{text}\n--- output.content.end ---");
                let dump = ascii_dir.join(format!(
                    "result.{}",
                    Path::new(p)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("md")
                ));
                let _ = std::fs::write(&dump, &text);
                println!("--- dump ---\n{}", dump.display());
            }
            Err(e) => println!("--- output.content: НЕ ЧИТАЕТСЯ: {e}"),
        }
    } else {
        println!("--- output.path: отсутствует в JSON ---");
    }
    Ok(())
}
