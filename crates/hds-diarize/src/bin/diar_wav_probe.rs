//! Проба W0: медиа → WAV 16 кГц mono, измерение ведущей тишины и доли речи.
//!
//! Живая приёмка `PLAN_DIARIZATION_NEMOTRON.md` W0 без запуска `llm_host`:
//! * 77-мин mp4 → WAV < 30 с;
//! * mp4(AAC) vs mp3 одной встречи — lag 0 мс (сверка скриптом спайка `compare_pcm.py`);
//! * 18-мин файл — offset ведущей тишины ≈ 16 с (правило «устойчивой речи»).
//!
//! Запуск:
//! ```text
//! diar_wav_probe <медиа|wav> [--ffmpeg PATH] [--wav-out decoded.wav] [--trimmed out.wav]
//!                [--threshold -50] [--lead-in 0.3] [--json out.json]
//! ```
//!
//! `--wav-out` сохраняет декодированный WAV (сверка lag mp4↔mp3 скриптом спайка
//! `compare_pcm.py`); `--trimmed` — срез для входа ASR (вариант A).

use std::path::PathBuf;
use std::process::exit;
use std::time::Instant;

use hds_diarize::vad::{leading_silence_samples, speech_stats};
use hds_diarize::{decode_to_wav, read_wav, write_wav, TrimConfig, SAMPLE_RATE};

/// Поиск `ffmpeg` в `PATH` (для пробы достаточно; в продукте — `hds_core::tools`).
fn find_ffmpeg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["ffmpeg.exe", "ffmpeg"] {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn usage() -> ! {
    eprintln!(
        "использование: diar_wav_probe <медиа|wav> [--ffmpeg PATH] [--trimmed out.wav]\n\
         \x20              [--threshold -50] [--lead-in 0.3] [--json out.json]"
    );
    exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut src: Option<PathBuf> = None;
    let mut ffmpeg: Option<PathBuf> = None;
    let mut trimmed: Option<PathBuf> = None;
    let mut wav_out: Option<PathBuf> = None;
    let mut json: Option<PathBuf> = None;
    let mut threshold = -50.0f64;
    let mut lead_in = 0.3f64;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ffmpeg" => {
                i += 1;
                ffmpeg = args.get(i).map(PathBuf::from);
            }
            "--trimmed" => {
                i += 1;
                trimmed = args.get(i).map(PathBuf::from);
            }
            "--wav-out" => {
                i += 1;
                wav_out = args.get(i).map(PathBuf::from);
            }
            "--json" => {
                i += 1;
                json = args.get(i).map(PathBuf::from);
            }
            "--threshold" => {
                i += 1;
                threshold = match args.get(i).and_then(|v| v.parse::<f64>().ok()) {
                    Some(v) => v,
                    None => usage(),
                };
            }
            "--lead-in" => {
                i += 1;
                lead_in = match args.get(i).and_then(|v| v.parse::<f64>().ok()) {
                    Some(v) => v,
                    None => usage(),
                };
            }
            other if other.starts_with("--") => usage(),
            other => {
                if src.is_some() {
                    usage();
                }
                src = Some(PathBuf::from(other));
            }
        }
        i += 1;
    }
    let Some(src) = src else { usage() };

    // 1) Декод (один проход) — если на входе уже WAV, читаем как есть.
    let tmp = std::env::temp_dir().join(format!("hds-diarize-probe-{}", std::process::id()));
    let wav_path = tmp.join("input.wav");
    let is_wav = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false);
    let t_decode = Instant::now();
    let decoded = if is_wav {
        src.clone()
    } else {
        let ff = ffmpeg.or_else(find_ffmpeg).unwrap_or_else(|| {
            eprintln!("ffmpeg не найден: укажите --ffmpeg PATH");
            exit(1);
        });
        if let Err(e) = decode_to_wav(&ff, &src, &wav_path) {
            eprintln!("ошибка декодирования: {e}");
            exit(1);
        }
        wav_path.clone()
    };
    let decode_sec = t_decode.elapsed().as_secs_f64();

    if let Some(dst) = &wav_out {
        if let Some(dir) = dst.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::copy(&decoded, dst) {
            eprintln!("ошибка копирования WAV в {}: {e}", dst.display());
            exit(1);
        }
    }

    let wav = match read_wav(&decoded) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("ошибка чтения WAV: {e}");
            exit(1);
        }
    };
    if wav.sample_rate != SAMPLE_RATE || wav.channels != 1 {
        eprintln!(
            "ожидался WAV {SAMPLE_RATE} Гц mono, получено {} Гц / {} кан.",
            wav.sample_rate, wav.channels
        );
        exit(1);
    }

    // 2) Обрезка ведущей тишины и доля речи по всему файлу.
    let cfg = TrimConfig::new(threshold, lead_in);
    let trim = leading_silence_samples(&wav.samples, wav.sample_rate, &cfg);
    let total = wav.duration_sec();
    let st = speech_stats(&wav.samples, wav.sample_rate, 0.0, total, threshold, 20.0);

    if let Some(out) = &trimmed {
        if let Err(e) = write_wav(out, wav.slice_from(trim), wav.sample_rate) {
            eprintln!("ошибка записи среза: {e}");
            exit(1);
        }
    }

    let trim_sec = trim as f64 / wav.sample_rate as f64;
    println!(
        "audio={total:.2}s decode={decode_sec:.2}s trim_offset={trim_sec:.3}s speech={:.1}% \
         trimmed_wav={}",
        st.ratio * 100.0,
        trimmed
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "нет".to_string())
    );

    if let Some(path) = json {
        let body = format!(
            "{{\n \"source\": \"{}\",\n \"wav\": \"{}\",\n \"duration_sec\": {total:.3},\n \
             \"sample_rate\": {},\n \"decode_sec\": {decode_sec:.3},\n \
             \"trim_offset_samples\": {trim},\n \"trim_offset_sec\": {trim_sec:.4},\n \
             \"speech_ratio\": {:.4},\n \"speech_sec\": {:.3},\n \
             \"threshold_db\": {threshold}\n}}\n",
            src.display().to_string().replace('\\', "/"),
            decoded.display().to_string().replace('\\', "/"),
            wav.sample_rate,
            st.ratio,
            st.speech_sec
        );
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, body) {
            eprintln!("ошибка записи {}: {e}", path.display());
            exit(1);
        }
        println!("json: {}", path.display());
    }

    if !is_wav {
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
