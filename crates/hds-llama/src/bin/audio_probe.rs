//! W3: probe транскрибации (bridge-API движка, `mode: subtitle` → сегменты с таймкодами).
//!
//! Запуск:
//! `cargo run -p hds-llama --release --bin audio_probe -- [<audio>] [<whisper.bin>] [<gpu_index>]`
//! Переменные: `HDS_WHISPER_MODE` (по умолчанию `subtitle`), `HDS_WHISPER_CUSTOM` (`4.5`).

use std::path::{Path, PathBuf};

use hds_llama::error::{EngineError, Result};
use hds_llama::whisper::Whisper;
use hds_llama::{BridgeAudio, Engine};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .to_path_buf()
}

/// whisper-модель из общего каталога движка (`*.bin`/`*.gguf` в каталоге turbo-модели).
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

    let engine = Engine::open(None)?;
    let _cwd = engine.activate()?;
    println!("engine: {}", engine.dir().display());
    println!("audio:  {}", audio.display());
    println!("model:  {}", model.display());
    println!("gpu(bridge index): {gpu}");

    let bridge = BridgeAudio::load(engine.dir())?;
    let whisper = Whisper::new(bridge, &model, gpu, -1);

    let mode = std::env::var("HDS_WHISPER_MODE").unwrap_or_else(|_| "subtitle".to_string());
    let custom = std::env::var("HDS_WHISPER_CUSTOM").unwrap_or_else(|_| "4.5".to_string());

    let t0 = std::time::Instant::now();
    let tr = whisper.transcribe_file(&audio, &mode, &custom)?;
    println!(
        "whisper: mode={mode} custom={custom} ({:.1} с)",
        t0.elapsed().as_secs_f64()
    );
    println!("json: {}", tr.json);
    println!("сегментов: {}", tr.segments.len());
    for s in &tr.segments {
        println!("[{:>7.2} -> {:>7.2}] {}", s.t_start, s.t_end, s.text);
    }
    if tr.segments.is_empty() {
        return Err(EngineError::Other(
            "нет сегментов (проверьте mode/модель)".into(),
        ));
    }
    Ok(())
}
