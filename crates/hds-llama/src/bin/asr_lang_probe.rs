//! Проба W1: язык ASR + обрезка ведущей тишины + постфильтр «реплик из тишины».
//!
//! Идёт через **боевой** путь (`Whisper::transcribe_file` → `build_metadata`),
//! поэтому проверяет ровно то, что уйдёт в движок из `llm_host`.
//!
//! Запуск:
//! ```text
//! asr_lang_probe <media|wav> [--model PATH] [--gpu N] [--mode subtitle|speech]
//!                [--custom 4.5] [--language ru|auto|none] [--trim] [--json out.json]
//! ```
//!
//! * `--language ru` (по умолчанию) — фиксация языка (ключ `whisper_language`);
//! * `--language auto` — явная авто-детекция (ключ не передаётся);
//! * `--language none` — как «до W1»: язык не задан вовсе;
//! * `--trim` — обрезка ведущей тишины (W0); постфильтр считает энергию по полному
//!   WAV, а таймкоды печатаются в шкале полного файла.

use std::path::PathBuf;
use std::process::exit;
use std::time::Instant;

use hds_diarize::vad::{leading_silence_samples, TrimConfig};
use hds_diarize::{read_wav, write_wav, Wav, SAMPLE_RATE};
use hds_llama::asr_filter::{drop_silent_segments, FilterConfig};
use hds_llama::whisper::Whisper;
use hds_llama::{BridgeAudio, Engine};

/// whisper-модель из общего каталога движка (turbo-модель, `.bin`/`.gguf`).
fn default_whisper_model() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    let dir = base
        .config_dir()
        .join("OpenResearchTools")
        .join("models")
        .join("openresearchtools__whisper-large-v3-turbo-GGML");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .map(|x| x.eq_ignore_ascii_case("bin") || x.eq_ignore_ascii_case("gguf"))
                .unwrap_or(false)
        })
}

fn usage() -> ! {
    eprintln!(
        "использование: asr_lang_probe <медиа|wav> [--model PATH] [--gpu N] [--mode subtitle]\n\
         \x20              [--custom 4.5] [--language ru|auto|none] [--trim] [--json out.json]"
    );
    exit(2);
}

/// Абсолютный путь: `Engine::activate()` меняет рабочий каталог процесса, поэтому
/// относительные пути (вход/`--json`) надо зафиксировать заранее.
fn abs(p: PathBuf) -> PathBuf {
    std::path::absolute(&p).unwrap_or(p)
}

/// Маркеры галлюцинаций (английские и русские) — счётчик в отчёте.
const MARKERS: [&str; 6] = [
    "Thank you",
    "Obrigado",
    "Lukoyle",
    "Продолжение следует",
    "Субтитры",
    "DimaTorzok",
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut audio: Option<PathBuf> = None;
    let mut model: Option<PathBuf> = None;
    let mut gpu: i32 = 0;
    let mut mode = "subtitle".to_string();
    let mut custom = "4.5".to_string();
    let mut language = "ru".to_string();
    let mut trim = false;
    let mut json: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                model = args.get(i).map(PathBuf::from);
            }
            "--gpu" => {
                i += 1;
                gpu = match args.get(i).and_then(|v| v.parse::<i32>().ok()) {
                    Some(v) => v,
                    None => usage(),
                };
            }
            "--mode" => {
                i += 1;
                mode = args.get(i).cloned().unwrap_or_else(|| usage());
            }
            "--custom" => {
                i += 1;
                custom = args.get(i).cloned().unwrap_or_else(|| usage());
            }
            "--language" => {
                i += 1;
                language = args.get(i).cloned().unwrap_or_else(|| usage());
            }
            "--trim" => trim = true,
            "--json" => {
                i += 1;
                json = args.get(i).map(PathBuf::from);
            }
            other if other.starts_with("--") => usage(),
            other => {
                if audio.is_some() {
                    usage();
                }
                audio = Some(PathBuf::from(other));
            }
        }
        i += 1;
    }
    let Some(audio) = audio else { usage() };
    let audio = abs(audio);
    let json = json.map(abs);
    let model = model
        .map(abs)
        .or_else(default_whisper_model)
        .unwrap_or_else(|| {
            eprintln!("не найдена whisper-модель (*.bin/*.gguf): укажите --model");
            exit(1);
        });
    // Полный WAV нужен постфильтру; для медиа его нет (W4 декодирует сам).
    let full: Option<Wav> = read_wav(&audio).ok();
    let mut asr_input = audio.clone();
    let mut offset_sec = 0.0;
    let tmp = std::env::temp_dir().join(format!("hds-asr-probe-{}", std::process::id()));
    if trim {
        let Some(w) = full.as_ref() else {
            eprintln!("--trim требует WAV 16 кГц mono на входе");
            exit(1);
        };
        if w.sample_rate != SAMPLE_RATE || w.channels != 1 {
            eprintln!(
                "ожидался WAV {SAMPLE_RATE} Гц mono, получено {} Гц / {} кан.",
                w.sample_rate, w.channels
            );
            exit(1);
        }
        let cut = leading_silence_samples(&w.samples, w.sample_rate, &TrimConfig::default());
        offset_sec = cut as f64 / w.sample_rate as f64;
        std::fs::create_dir_all(&tmp).ok();
        asr_input = tmp.join("asr_input.wav");
        if let Err(e) = write_wav(&asr_input, w.slice_from(cut), w.sample_rate) {
            eprintln!("ошибка записи среза: {e}");
            exit(1);
        }
    }

    let lang: Option<String> = match language.trim() {
        "none" | "off" => None,
        "auto" => Some(String::new()),
        other => Some(other.to_string()),
    };

    let engine = match Engine::open(None) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("движок: {e}");
            exit(1);
        }
    };
    let _cwd = match engine.activate() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("активация движка: {e}");
            exit(1);
        }
    };
    let bridge = match BridgeAudio::load(engine.dir()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bridge: {e}");
            exit(1);
        }
    };
    let whisper = match Whisper::new(bridge, &model, gpu, -1) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("whisper: {e}");
            exit(1);
        }
    };

    let t0 = Instant::now();
    let tr = match whisper.transcribe_file(&asr_input, &mode, &custom, None, lang.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("транскрибация: {e}");
            exit(1);
        }
    };
    let elapsed = t0.elapsed().as_secs_f64();

    let mut counts = [0usize; MARKERS.len()];
    for s in &tr.segments {
        for (k, m) in MARKERS.iter().enumerate() {
            if s.text.contains(m) {
                counts[k] += 1;
            }
        }
    }
    let filter = full
        .as_ref()
        .map(|w| drop_silent_segments(&tr.segments, w, offset_sec, &FilterConfig::default()));
    let (kept, dropped) = match &filter {
        Some(f) => (f.kept.len(), f.filtered_count()),
        None => (tr.segments.len(), 0),
    };

    println!(
        "asr: mode={mode} custom={custom} language={} trim={offset_sec:.3}s segments={} ({elapsed:.1} с)",
        lang.as_deref().unwrap_or("none"),
        tr.segments.len()
    );
    println!(
        "маркеры: {}",
        MARKERS
            .iter()
            .zip(counts.iter())
            .map(|(m, n)| format!("{m}={n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(f) = &filter {
        println!("постфильтр: оставлено {kept}, отброшено {dropped}");
        for s in &f.dropped {
            println!(
                "  [-] [{:>7.2} -> {:>7.2}] {}",
                s.t_start + offset_sec,
                s.t_end + offset_sec,
                s.text
            );
        }
    }

    if let Some(path) = json {
        let mut body = String::from("{\n");
        body.push_str(&format!(
            " \"audio\": \"{}\",\n \"asr_input\": \"{}\",\n \"mode\": \"{mode}\",\n \
             \"language\": {},\n \"trim_offset_sec\": {offset_sec:.4},\n \
             \"elapsed_sec\": {elapsed:.2},\n \"segments\": {},\n \"kept\": {kept},\n \
             \"filtered\": {dropped},\n \"markers\": {{",
            audio.display().to_string().replace('\\', "/"),
            asr_input.display().to_string().replace('\\', "/"),
            match lang.as_deref() {
                None => "null".to_string(),
                Some(s) => format!("\"{s}\""),
            },
            tr.segments.len()
        ));
        for (k, (m, n)) in MARKERS.iter().zip(counts.iter()).enumerate() {
            if k > 0 {
                body.push_str(", ");
            }
            body.push_str(&format!("\"{m}\": {n}"));
        }
        body.push_str("},\n \"dropped\": [");
        if let Some(f) = &filter {
            for (k, s) in f.dropped.iter().enumerate() {
                if k > 0 {
                    body.push_str(", ");
                }
                body.push_str(&format!(
                    "{{\"t_start\": {:.2}, \"t_end\": {:.2}, \"text\": \"{}\"}}",
                    s.t_start + offset_sec,
                    s.t_end + offset_sec,
                    s.text.replace('"', "'")
                ));
            }
        }
        // Реплики целиком — для независимой проверки текста (маркеры, WER и пр.):
        // логи консоли на Windows искажают кириллицу, JSON читается без потерь.
        body.push_str("],\n \"kept_segments\": [");
        let kept_list: Vec<_> = match &filter {
            Some(f) => f.kept.iter().collect(),
            None => tr.segments.iter().collect(),
        };
        for (k, s) in kept_list.iter().enumerate() {
            if k > 0 {
                body.push_str(", ");
            }
            body.push_str(&format!(
                "{{\"t_start\": {:.2}, \"t_end\": {:.2}, \"text\": \"{}\"}}",
                s.t_start + offset_sec,
                s.t_end + offset_sec,
                s.text.replace('"', "'")
            ));
        }
        body.push_str("]\n}\n");
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, body) {
            eprintln!("ошибка записи {}: {e}", path.display());
            exit(1);
        }
        println!("json: {}", path.display());
    }

    if trim {
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
