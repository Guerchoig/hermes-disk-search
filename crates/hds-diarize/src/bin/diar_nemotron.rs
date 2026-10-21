//! Проба W3: Nemotron-3 Diarization на WAV → turns (JSON в формате спайка).
//!
//! Формат JSON совпадает с `tools/parity/out/diar_spike/out/nemotron3_*.json`,
//! поэтому метрики считаются теми же скриптами спайка (`flatten.py`,
//! `eval_diar.py`).
//!
//! Запуск:
//! ```text
//! diar_nemotron <model.onnx> <audio16k.wav> [--device auto|cpu|directml|coreml]
//!               [--chunk-min 10] [--json out.json]
//! ```

use std::path::PathBuf;
use std::process::exit;
use std::time::Instant;

use hds_diarize::{default_model_path, Device, Diarizer, NemotronConfig};

fn usage() -> ! {
    eprintln!(
        "использование: diar_nemotron <model.onnx|-> <audio16k.wav> \
         [--device auto|cpu|directml|coreml] [--chunk-min N] [--json out.json]\n\
         \x20              '-' вместо модели — путь по умолчанию ({})",
        default_model_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "не найден".to_string())
    );
    exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut positional: Vec<String> = Vec::new();
    let mut device = Device::Auto;
    let mut chunk_min = 10.0f64;
    let mut json: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--device" => {
                i += 1;
                let Some(s) = args.get(i) else { usage() };
                device = Device::parse(s).unwrap_or_else(|| usage());
            }
            "--chunk-min" => {
                i += 1;
                chunk_min = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage());
            }
            "--json" => {
                i += 1;
                json = args.get(i).map(PathBuf::from);
            }
            other if other.starts_with("--") => usage(),
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    if positional.len() != 2 {
        usage();
    }
    let model = if positional[0] == "-" {
        default_model_path().unwrap_or_else(|| {
            eprintln!("модель по умолчанию не найдена: укажите путь");
            exit(1)
        })
    } else {
        PathBuf::from(&positional[0])
    };
    let wav = PathBuf::from(&positional[1]);

    let mut cfg = NemotronConfig::new(&model);
    cfg.device = device;
    cfg.chunk_min = chunk_min;
    let t0 = Instant::now();
    let mut diar = match Diarizer::load(&cfg) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("загрузка: {e}");
            exit(1);
        }
    };
    let load_sec = t0.elapsed().as_secs_f64();

    let t1 = Instant::now();
    let turns = match diar.run_wav(&wav) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("диаризация: {e}");
            exit(1);
        }
    };
    let infer_sec = t1.elapsed().as_secs_f64();

    let mut speakers: Vec<u32> = Vec::new();
    for t in &turns {
        if !speakers.contains(&t.speaker) {
            speakers.push(t.speaker);
        }
    }
    speakers.sort_unstable();
    let audio_sec = turns.last().map(|t| t.end).unwrap_or(0.0);
    println!(
        "nemotron3: device={} audio≈{audio_sec:.1}s load={load_sec:.2}s infer={infer_sec:.2}s \
         rtfx={:.1} turns={} speakers={} ({:?})",
        diar.device().as_str(),
        if infer_sec > 0.0 {
            audio_sec / infer_sec
        } else {
            0.0
        },
        turns.len(),
        speakers.len(),
        speakers
    );

    if let Some(path) = json {
        let mut body = String::from("{\n");
        body.push_str(&format!(
            " \"engine\": \"nemotron3-diarization\",\n \"model\": \"{}\",\n \
             \"audio_sec\": {audio_sec:.2},\n \"device\": \"{}\",\n \
             \"load_sec\": {load_sec:.2},\n \"infer_sec\": {infer_sec:.2},\n \
             \"rtfx\": {:.1},\n \"num_speakers\": {},\n \"speakers\": [{}],\n \"turns\": [\n",
            model.display().to_string().replace('\\', "/"),
            diar.device().as_str(),
            if infer_sec > 0.0 {
                audio_sec / infer_sec
            } else {
                0.0
            },
            speakers.len(),
            speakers
                .iter()
                .map(|s| format!("\"SPEAKER_{s:02}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for (k, t) in turns.iter().enumerate() {
            body.push_str(&format!(
                "  {{\"speaker\": \"SPEAKER_{:02}\", \"start\": {:.3}, \"end\": {:.3}}}{}\n",
                t.speaker,
                t.start,
                t.end,
                if k + 1 < turns.len() { "," } else { "" }
            ));
        }
        body.push_str(" ]\n}\n");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, body) {
            eprintln!("ошибка записи {}: {e}", path.display());
            exit(1);
        }
        println!("json: {}", path.display());
    }
}
