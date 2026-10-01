//! W3: probe транскрибации через bridge-API движка (direct-model, plan §8.3).
//!
//! `llama-server-bridge.dll` → `llama_server_bridge_create` (модель напрямую) →
//! `llama_server_bridge_audio_transcriptions_raw`. ASCII-стейджинг путей обязателен.
//! (Кластерный путь упирается в execution_group — см. W3_REPORT §1.3.)
//!
//! Запуск:
//! `cargo run -p hds-llama --release --bin audio_probe -- [<audio>] [<whisper.bin>] [<gpu_index>]`

use std::path::{Path, PathBuf};

use serde_json::json;

use hds_llama::bridge_audio::BridgeAudio;
use hds_llama::error::{EngineError, Result};
use hds_llama::Engine;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// whisper-модель из общего каталога движка: `*.bin`/`*.gguf` в каталоге turbo-модели.
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
        if p.extension()
            .and_then(|x| x.to_str())
            .map(|x| x.eq_ignore_ascii_case("bin") || x.eq_ignore_ascii_case("gguf"))
            .unwrap_or(false)
        {
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
        .ok_or_else(|| EngineError::Other("не найдена whisper-модель (*.bin/*.gguf)".into()))?;
    let gpu: i32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    // Движок: готовит путь поиска DLL и делает каталог движка текущим (A1/R32).
    let engine = Engine::open(None)?;
    let _cwd = engine.activate()?;
    println!("engine: {}", engine.dir().display());
    println!("audio:  {}", audio.display());
    println!("model:  {}", model.display());
    println!("gpu(bridge index): {gpu}");

    let bridge = BridgeAudio::load(engine.dir())?;

    // ASCII-стейджинг (грабля спайка 5).
    let ascii_dir = std::env::temp_dir().join("hds_audio_probe");
    std::fs::create_dir_all(&ascii_dir)
        .map_err(|e| EngineError::Other(format!("{ascii_dir:?}: {e}")))?;
    let staged = ascii_dir.join("audio_input.wav");
    std::fs::copy(&audio, &staged)
        .map_err(|e| EngineError::Other(format!("стейджинг {:?}: {e}", audio)))?;
    let bytes = std::fs::read(&staged).map_err(|e| EngineError::Other(format!("{staged:?}: {e}")))?;

    let meta = json!({
        "mode": "speech",
        "custom": "default",
        "output_dir": ascii_dir.to_string_lossy(),
        "audio_source_path": staged.to_string_lossy(),
        "transport": "audio_raw_bytes",
    });

    let t0 = std::time::Instant::now();
    let out = bridge.transcribe_raw(&model, Some(gpu), -1, &bytes, "wav", &meta.to_string(), true)?;
    println!(
        "bridge: ok={} status={} rc_ok ({:.1} с)",
        out.ok,
        out.status,
        t0.elapsed().as_secs_f64()
    );
    if !out.error.is_empty() {
        println!("--- error ---\n{}", out.error);
    }
    let preview: String = out.json.chars().take(2000).collect();
    println!("--- json (первые 2000) ---\n{preview}");
    if !out.ok {
        return Err(EngineError::Other(format!(
            "движок вернул ok=0 (status={}): {}",
            out.status,
            if out.error.is_empty() { "без текста ошибки" } else { &out.error }
        )));
    }
    Ok(())
}
