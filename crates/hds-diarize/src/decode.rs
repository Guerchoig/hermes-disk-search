//! Декодирование медиа в WAV 16 кГц mono через CLI `ffmpeg` — **вариант A**
//! (`DIARIZATION_NEMOTRON_HANDOFF.md`, решение владельца №2): медиа декодируется
//! один раз, и этот же WAV идёт и в ASR, и в диаризацию.
//!
//! Почему CLI, а не библиотека: `ffprobe`/`ffmpeg` уже используются (спайк и
//! `hds-index::diag`), а `symphonia`/`ryf`/`opus-rs` — новые зависимости, которые
//! запрещены (`handoff` п. 7). Один проход по 77-мин mp4 занимает секунды.
//!
//! Команда фиксирована: `ffmpeg -nostdin -y -v error -i <src> -vn -ac 1 -ar 16000
//! -c:a pcm_s16le <dst>`. Путь к `ffmpeg` передаёт вызывающий (в продукте —
//! `hds-core::tools::find_ffmpeg`), чтобы модуль не зависел от конфига.

use std::path::Path;
use std::process::Command;

use crate::{DiarizeError, Result, SAMPLE_RATE};

/// Декодировать медиа в WAV (`pcm_s16le`, mono, [`SAMPLE_RATE`]).
///
/// Возвращает путь к созданному файлу (тот же `dst`). Ошибка содержит хвост
/// `stderr` — по нему видно, битый файл или нет кодека.
pub fn decode_to_wav(ffmpeg: &Path, src: &Path, dst: &Path) -> Result<()> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| DiarizeError::Io(format!("{}: {e}", dir.display())))?;
    }
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-nostdin")
        .arg("-y")
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(src)
        .arg("-vn")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg(SAMPLE_RATE.to_string())
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(dst)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // Службы на Windows не должны мигать консолью.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd
        .output()
        .map_err(|e| DiarizeError::Tool(format!("запуск {}: {e}", ffmpeg.display())))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: String = err.trim().chars().rev().take(400).collect::<String>();
        let tail: String = tail.chars().rev().collect();
        return Err(DiarizeError::Tool(format!(
            "ffmpeg вернул {} для {}: {tail}",
            out.status,
            src.display()
        )));
    }
    if !dst.is_file() {
        return Err(DiarizeError::Tool(format!(
            "ffmpeg не создал {}",
            dst.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{read_wav, write_wav};

    /// Поиск `ffmpeg` в `PATH` — тест пропускается, если утилиты нет (CI без ffmpeg).
    fn ffmpeg() -> Option<std::path::PathBuf> {
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

    /// WAV на входе ffmpeg остаётся WAV: длина и частота сохраняются, декод один.
    #[test]
    fn decodes_wav_losslessly() {
        let Some(ff) = ffmpeg() else {
            eprintln!("[skip] ffmpeg не найден в PATH");
            return;
        };
        let dir = std::env::temp_dir().join(format!("hds-diarize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.wav");
        let dst = dir.join("out.wav");
        let samples: Vec<i16> = (0..SAMPLE_RATE as usize)
            .map(|i| ((i as f32 * 0.05).sin() * 8000.0) as i16)
            .collect();
        write_wav(&src, &samples, SAMPLE_RATE).unwrap();
        decode_to_wav(&ff, &src, &dst).unwrap();
        let w = read_wav(&dst).unwrap();
        assert_eq!(w.sample_rate, SAMPLE_RATE);
        assert_eq!(w.channels, 1);
        assert_eq!(w.samples.len(), samples.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
