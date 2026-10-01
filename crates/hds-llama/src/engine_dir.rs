//! Поиск каталога и библиотеки движка (`PLAN_W2_LLM_HOST.md` §A1):
//! `index.whisper_engine_dir` → `%APPDATA%\OpenResearchTools\TranscribeOffline\Engine`
//! → рядом с исполняемым файлом.
//!
//! Имена библиотек держим в одном месте, а не в строках по коду (§11.6 плана:
//! Windows `multi-node-server.dll`, macOS `libmulti-node-server.dylib`).

use std::path::{Path, PathBuf};

use crate::error::{EngineError, Result};

/// Подкаталог движка внутри `%APPDATA%` (`~/Library/Application Support` на macOS).
pub const ENGINE_DIR_PARTS: &[&str] = &["OpenResearchTools", "TranscribeOffline", "Engine"];

/// Библиотека с cluster API.
#[cfg(windows)]
pub const ENGINE_LIB: &str = "multi-node-server.dll";
/// Библиотека с cluster API (macOS; не проверяется до появления Mac, §10.0).
#[cfg(target_os = "macos")]
pub const ENGINE_LIB: &str = "libmulti-node-server.dylib";
/// Библиотека с cluster API (Linux).
#[cfg(all(unix, not(target_os = "macos")))]
pub const ENGINE_LIB: &str = "libmulti-node-server.so";

/// Библиотека с bridge API (direct-model путь: chat/vlm/embeddings/rerank/audio).
#[cfg(windows)]
pub const BRIDGE_LIB: &str = "llama-server-bridge.dll";
/// Библиотека с bridge API (macOS; не проверяется до появления Mac).
#[cfg(target_os = "macos")]
pub const BRIDGE_LIB: &str = "libllama-server-bridge.dylib";
/// Библиотека с bridge API (Linux).
#[cfg(all(unix, not(target_os = "macos")))]
pub const BRIDGE_LIB: &str = "libllama-server-bridge.so";

/// Путь к библиотеке bridge API внутри каталога движка.
pub fn bridge_lib_path(dir: &Path) -> PathBuf {
    dir.join(BRIDGE_LIB)
}

/// Переменная окружения для переопределения каталога движка (тесты, ярлыки,
/// установщик) — включается в тот же порядок поиска, что и конфиг.
pub const ENGINE_DIR_ENV: &str = "HDS_ENGINE_DIR";

/// Каталог движка, из которого запущен текущий процесс (`hdsw.exe`): ищем
/// `Engine` рядом с бинарём и сам каталог бинаря (портативная раскладка).
fn exe_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("Engine"));
            out.push(dir.to_path_buf());
            // cargo target/<profile>/<bin>.exe → цель лежит на два уровня выше
            if let Some(up) = dir.parent().and_then(|p| p.parent()) {
                out.push(up.join("Engine"));
            }
        }
    }
    out
}

/// Каталог данных приложения (Roaming/%APPDATA%): там живёт установленный движок.
fn user_engine_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(base) = directories::BaseDirs::new() {
        for root in [base.config_dir(), base.data_dir()] {
            let mut p = root.to_path_buf();
            for part in ENGINE_DIR_PARTS {
                p.push(part);
            }
            out.push(p);
        }
    }
    out
}

/// Кандидаты в порядке приоритета (первый существующий с библиотекой побеждает).
pub fn candidate_dirs(config_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let push = |d: PathBuf, out: &mut Vec<PathBuf>| {
        if !out.contains(&d) {
            out.push(d);
        }
    };
    if let Some(d) = config_dir {
        // index.whisper_engine_dir — главный источник (пока конфиг читает Python/A2)
        push(d.to_path_buf(), &mut out);
    }
    if let Ok(env) = std::env::var(ENGINE_DIR_ENV) {
        if !env.trim().is_empty() {
            push(PathBuf::from(env), &mut out);
        }
    }
    for d in user_engine_dirs() {
        push(d, &mut out);
    }
    for d in exe_dirs() {
        push(d, &mut out);
    }
    out
}

/// Путь к библиотеке cluster API внутри каталога движка.
pub fn engine_lib_path(dir: &Path) -> PathBuf {
    dir.join(ENGINE_LIB)
}

/// Каталоги, которые нужно добавить в путь поиска DLL.
///
/// **Движок не самодостаточен** (находка A1): `multi-node-server.dll` →
/// `llama-server-bridge.dll` → `llama-server-audio.dll` статически импортирует
/// `avcodec-62.dll`/`avformat-62.dll`/`avutil-60.dll`/`swresample-6.dll`, а лежат
/// они в `Engine\vendor\ffmpeg\bin`. Без этого каталога `LoadLibraryExW` во всех
/// режимах падает с кодом 126 (`ERROR_MOD_NOT_FOUND`) — проверено
/// (`tools/parity/out/w2_a1_device.json`, поле `load_notes`).
pub fn dll_search_dirs(engine_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![engine_dir.to_path_buf()];
    let vendor = engine_dir.join("vendor");
    if let Ok(entries) = std::fs::read_dir(&vendor) {
        let mut subs: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        subs.sort();
        for sub in subs {
            let bin = sub.join("bin");
            if bin.is_dir() {
                dirs.push(bin.clone());
            }
            dirs.push(sub);
        }
    }
    dirs
}

/// Каталог движка: первый кандидат, в котором есть [`ENGINE_LIB`].
pub fn find_engine_dir(config_dir: Option<&Path>) -> Result<PathBuf> {
    let candidates = candidate_dirs(config_dir);
    for dir in &candidates {
        if engine_lib_path(dir).is_file() {
            return Ok(dir.clone());
        }
    }
    Err(EngineError::EngineDirNotFound {
        searched: candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("; "),
    })
}
