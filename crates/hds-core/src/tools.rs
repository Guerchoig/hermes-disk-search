//! Внешние CLI-утилиты: поиск исполняемых файлов и `ffmpeg`.
//!
//! Зачем отдельный модуль: `ffmpeg` нужен в двух местах — `hds-index` (проверка
//! `hds check`, `ffprobe`-метаданные) и новом пути диаризации Nemotron-3
//! (`hds-diarize::decode`, `PLAN_DIARIZATION_NEMOTRON.md` W0). Раньше поиск жил
//! в `hds-index::diag`, но `hds-llama` (владелец `llm_host`) не зависит от
//! `hds-index` — поэтому единая функция переехала в общий слой.
//!
//! Службы (`LocalSystem`/LaunchDaemon) стартуют с минимальным `PATH`, поэтому
//! машинная установка (Program Files, chocolatey, winget) иначе выглядит как
//! «ffmpeg не найден» (живой инцидент 15.10.2026).

use std::path::PathBuf;

use crate::config::{dig, Config};

/// Поиск программы в `PATH` (порт `shutil.which`).
///
/// На Windows дополнительно пробуется имя с `.exe` — вызывающему не нужно знать
/// про расширение (`which("ffmpeg")` находит `ffmpeg.exe`).
pub fn which(prog: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(prog);
        if cand.is_file() {
            return Some(cand);
        }
        #[cfg(windows)]
        {
            if !prog.to_ascii_lowercase().ends_with(".exe") {
                let exe = dir.join(format!("{prog}.exe"));
                if exe.is_file() {
                    return Some(exe);
                }
            }
        }
    }
    None
}

/// Каталоги установки CLI `ffmpeg` (кроме `PATH`).
#[cfg(windows)]
fn standard_ffmpeg_paths() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        v.push(
            PathBuf::from(&pf)
                .join("ffmpeg")
                .join("bin")
                .join("ffmpeg.exe"),
        );
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        v.push(
            PathBuf::from(pd)
                .join("chocolatey")
                .join("bin")
                .join("ffmpeg.exe"),
        );
    }
    // winget: пользовательская (%LOCALAPPDATA%\Microsoft\WinGet\Packages) и
    // машинная (%ProgramFiles%\WinGet\Packages) области — `Gyan.FFmpeg*\<ver>\bin`.
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            PathBuf::from(la)
                .join("Microsoft")
                .join("WinGet")
                .join("Packages"),
        );
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(pf).join("WinGet").join("Packages"));
    }
    for root in roots {
        let Ok(pkgs) = std::fs::read_dir(&root) else {
            continue;
        };
        for pkg in pkgs.flatten() {
            if !pkg
                .file_name()
                .to_string_lossy()
                .to_lowercase()
                .starts_with("gyan.ffmpeg")
            {
                continue;
            }
            let Ok(vers) = std::fs::read_dir(pkg.path()) else {
                continue;
            };
            for ver in vers.flatten() {
                v.push(ver.path().join("bin").join("ffmpeg.exe"));
            }
        }
    }
    v
}

#[cfg(not(windows))]
fn standard_ffmpeg_paths() -> Vec<PathBuf> {
    [
        "/opt/homebrew/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/usr/bin/ffmpeg",
    ]
    .iter()
    .map(PathBuf::from)
    .collect()
}

/// Путь к CLI `ffmpeg`: `index.ffmpeg_cmd` → `PATH` → стандартные каталоги.
///
/// Явный ключ конфига сильнее всего, но только если файл существует (опечатка в
/// пути не должна «прятать» ffmpeg из `PATH`).
pub fn find_ffmpeg(cfg: &Config) -> Option<PathBuf> {
    if let Some(cmd) = dig(cfg, "index.ffmpeg_cmd").and_then(|v| v.as_str()) {
        let p = std::path::Path::new(cmd.trim());
        if !cmd.trim().is_empty() && p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    if let Some(p) = which("ffmpeg") {
        return Some(p);
    }
    standard_ffmpeg_paths().into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_yaml(text: &str) -> Config {
        serde_yaml::from_str(text).unwrap()
    }

    /// Явный `index.ffmpeg_cmd` с существующим файлом побеждает поиск в `PATH`.
    #[test]
    fn explicit_config_path_wins_when_exists() {
        let dir = std::env::temp_dir().join(format!("hds-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        std::fs::write(&fake, b"x").unwrap();
        let yaml = format!(
            "index:\n  ffmpeg_cmd: '{}'\n",
            fake.display().to_string().replace('\\', "/")
        );
        let got = find_ffmpeg(&cfg_yaml(&yaml)).unwrap();
        assert_eq!(got, fake);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Несуществующий путь в конфиге не отменяет поиска (иначе опечатка «прячет» ffmpeg).
    #[test]
    fn missing_config_path_falls_back() {
        let cfg = cfg_yaml("index:\n  ffmpeg_cmd: 'Z:/no/such/ffmpeg'\n");
        // Результат зависит от машины (dev — ffmpeg есть); проверяем только то,
        // что это не указанный несуществующий путь.
        if let Some(p) = find_ffmpeg(&cfg) {
            assert!(p.is_file(), "{}", p.display());
            assert!(!p.to_string_lossy().contains("no/such"));
        }
    }

    /// Пустой `PATH` и пустой конфиг: `which` не паникует.
    #[test]
    fn which_is_total() {
        let _ = which("hds-такой-программы-нет");
    }
}
