//! W3: probe транскрибации через АУДИО-API движка (cluster, batch-режим).
//!
//! Разведка/проверка пути W3: создаёт роль `whisper` (model_kind = 4) во владельце
//! инстансов, шлёт содержимое аудиофайла в `llama_server_cluster_audio_transcriptions_raw`
//! и печатает JSON-ответ движка. ASCII-стейджинг путей обязателен (спайк 5).
//!
//! Запуск:
//! `cargo run -p hds-llama --release --bin audio_probe -- [<audio>] [<whisper.bin>] [<gpu_index>]`
//! По умолчанию: `test_data/jfk.wav`, whisper-turbo из `%APPDATA%\OpenResearchTools\models`.

use std::path::{Path, PathBuf};

use serde_json::json;

use hds_llama::cluster::InstanceSpec;
use hds_llama::error::{EngineError, Result};
use hds_llama::ffi::{model_kind, retention};
use hds_llama::Engine;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// whisper-модель из общего каталога движка: `*.bin` в каталоге turbo-модели.
fn default_whisper_model() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    let dir = base
        .config_dir()
        .join("OpenResearchTools")
        .join("models")
        .join("openresearchtools__whisper-large-v3-turbo-GGML");
    let rd = std::fs::read_dir(&dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| x.eq_ignore_ascii_case("bin")).unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

fn main() -> Result<()> {
    let mut it = std::env::args().skip(1);
    let audio = it
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root().join("test_data").join("jfk.wav"));
    let model = it
        .next()
        .map(PathBuf::from)
        .or_else(default_whisper_model)
        .ok_or_else(|| EngineError::Other("не найдена whisper-модель (*.bin)".into()))?;
    let gpu_arg: Option<i32> = it.next().and_then(|s| s.parse().ok());

    let engine = Engine::open(None)?;
    let _cwd = engine.activate()?;
    println!("engine: {}", engine.dir().display());
    println!("audio:  {}", audio.display());
    println!("model:  {}", model.display());

    let cluster = engine.create_cluster()?;
    let devices = cluster.devices()?;
    for d in &devices {
        println!(
            "device bridge_index={} backend={} name={} accel={}",
            d.bridge_device_index,
            d.backend,
            d.name,
            d.is_accelerator()
        );
    }
    // устройство: явный GPU-индекс, иначе первый ускоритель, иначе CPU
    let gpu_index = gpu_arg.or_else(|| {
        devices
            .iter()
            .find(|d| d.is_accelerator())
            .map(|d| d.bridge_device_index)
    });

    // ASCII-стейджинг (грабля спайка 5): не-ASCII путь ломает имя результата
    let ascii_dir = std::env::temp_dir().join("hds_audio_probe");
    std::fs::create_dir_all(&ascii_dir).map_err(|e| EngineError::Other(format!("{ascii_dir:?}: {e}")))?;
    let staged = ascii_dir.join("audio_input.wav");
    std::fs::copy(&audio, &staged)
        .map_err(|e| EngineError::Other(format!("стейджинг {:?}: {e}", audio)))?;
    let bytes = std::fs::read(&staged).map_err(|e| EngineError::Other(format!("{staged:?}: {e}")))?;

    let mut spec = InstanceSpec::new("probe-whisper", &model.to_string_lossy());
    spec.model_kind = Some(model_kind::WHISPER);
    spec.retention_mode = Some(retention::LOAD_ON_DEMAND);
    match gpu_index {
        Some(idx) => spec.manual_devices_csv = Some(idx.to_string()),
        None => spec.allow_cpu = Some(true),
    }
    let id = cluster.create_instance(&spec)?;
    println!("whisper instance id={id} (gpu={gpu_index:?})");

    let mut meta = json!({
        "mode": "speech",
        "custom": "default",
        "output_dir": ascii_dir.to_string_lossy(),
        "audio_source_path": staged.to_string_lossy(),
        "whisper_model": model.to_string_lossy(),
        "transport": "audio_raw_bytes",
    });
    if let Some(idx) = gpu_index {
        meta["whisper_gpu_device"] = json!(idx);
    } else {
        meta["whisper_no_gpu"] = json!(true);
    }

    let t0 = std::time::Instant::now();
    let out = cluster.transcribe_audio_raw(id, &bytes, "wav", &meta.to_string(), true)?;
    println!("rc={} ok={} status={} ({:.1} с)", out.rc, out.ok, out.status, t0.elapsed().as_secs_f64());
    if !out.error.is_empty() {
        println!("--- error ---\n{}", out.error);
    }
    println!("--- json (первые 2000) ---\n{}", &out.json.chars().take(2000).collect::<String>());
    out.ensure_ok()?;
    Ok(())
}
