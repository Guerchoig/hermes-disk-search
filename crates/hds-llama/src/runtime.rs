//! Общий llama-рантайм машины — порт путей и разрешения моделей из
//! `hds/llama_runtime.py` (файл помечен `SYNC-COPY`, поэтому пути обязаны
//! совпадать байт-в-байт).
//!
//! Раскладка (`%LLAMA_RUNTIME_DIR%`, иначе по ОС):
//! ```text
//! bin\                        — llama-server + DLL (одна сборка cuda|vulkan)
//! models\<role>\*.gguf        — GGUF роли (chat, embedding, rerank)
//! models\<role>\current.json  — {"file": "…gguf", "switched_at": "…"}
//! version.json / projects.json
//! ```
//!
//! `shared:<role>` в конфиге проекта означает «файл из манифеста общего рантайма».
//! Факт W0 (`SPIKES.md` §14.7): штатный старт роли chat на этой машине падал,
//! потому что модель лежала в общем рантайме, а искалась в каталоге проекта —
//! поэтому разрешение `shared:` портируется один в один.

use std::path::{Path, PathBuf};

use crate::error::{EngineError, Result};

/// Переменная окружения для переопределения каталога рантайма.
pub const RUNTIME_DIR_ENV: &str = "LLAMA_RUNTIME_DIR";
/// Имя каталога рантайма по умолчанию.
pub const DEFAULT_DIRNAME: &str = "llama-runtime";
/// Префикс «модель из общего рантайма».
pub const SHARED_PREFIX: &str = "shared:";

/// Каталог общего рантайма: env → локальные данные пользователя → ошибка.
///
/// Windows: `%LLAMA_RUNTIME_DIR%` или `%LOCALAPPDATA%\llama-runtime`;
/// macOS: `~/Library/Application Support/llama-runtime`;
/// прочие: `~/.local/share/llama-runtime`.
pub fn runtime_dir() -> Result<PathBuf> {
    match std::env::var(RUNTIME_DIR_ENV) {
        Ok(v) if !v.trim().is_empty() => Ok(PathBuf::from(v.trim())),
        _ => {
            let base = directories::BaseDirs::new().ok_or_else(|| {
                EngineError::Other("не удалось определить каталог данных пользователя".into())
            })?;
            let local = base.data_local_dir().to_path_buf();
            // на macOS/Unix data_local_dir даёт ~/.local/share — совпадает с Python
            Ok(local.join(DEFAULT_DIRNAME))
        }
    }
}

/// Каталог `bin` рантайма (llama-server; в W2 не используется, но остаётся в API).
pub fn bin_dir(root: &Path) -> PathBuf {
    root.join("bin")
}

/// Каталог моделей: `models` или `models/<role>`.
pub fn models_dir(root: &Path, role: Option<&str>) -> PathBuf {
    let d = root.join("models");
    match role {
        Some(r) => d.join(r),
        None => d,
    }
}

/// Манифест активной модели роли.
pub fn current_file(root: &Path, role: &str) -> PathBuf {
    models_dir(root, Some(role)).join("current.json")
}

/// Прочитать JSON-файл; битый/отсутствующий = пусто (как `_read_json` в Python).
fn read_json(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// Имя файла активной модели роли из `current.json` (пустая строка = не задано).
pub fn read_current(root: &Path, role: &str) -> String {
    read_json(&current_file(root, role))
        .get("file")
        .and_then(|f| f.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Порт `resolve_model`: `shared:<role>` → путь GGUF из манифеста рантайма,
/// иначе — путь как есть (существование проверяется отдельно, как в Python).
pub fn resolve_model(root: &Path, spec: &str, role: &str) -> Result<PathBuf> {
    let mut s = spec.trim().to_string();
    if s == SHARED_PREFIX.trim_end_matches(':') {
        s = format!("{SHARED_PREFIX}{role}");
    }
    if !s.starts_with(SHARED_PREFIX) {
        return Ok(PathBuf::from(s));
    }
    let r = s[SHARED_PREFIX.len()..].trim();
    let r = if r.is_empty() { role } else { r };
    let dir = models_dir(root, Some(r));
    let name = read_current(root, r);
    if !name.is_empty() {
        let p = dir.join(&name);
        if p.is_file() {
            return Ok(p);
        }
        // регистр в манифесте мог отличаться (Windows ФС регистронезависима)
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension()
                    .map(|x| x.eq_ignore_ascii_case("gguf"))
                    .unwrap_or(false)
                    && e.file_name().to_string_lossy().eq_ignore_ascii_case(&name)
                {
                    return Ok(p);
                }
            }
        }
        return Err(EngineError::ModelNotFound {
            role: role.to_string(),
            searched: format!(
                "{} (манифест current.json указывает на {name}, файла нет) — \
                 обновите манифест сменой модели в UI или положите файл в {}",
                p.display(),
                dir.display()
            ),
        });
    }
    let mut ggufs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .map(|x| x.eq_ignore_ascii_case("gguf"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    ggufs.sort();
    if ggufs.len() == 1 {
        // единственная модель роли = активная (манифест ещё не создан)
        return Ok(ggufs.remove(0));
    }
    Err(EngineError::ModelNotFound {
        role: role.to_string(),
        searched: format!(
            "{} (манифест current.json отсутствует/битый, GGUF в каталоге: {})",
            dir.display(),
            ggufs.len()
        ),
    })
}

/// Разрешить модель и проверить, что файл существует (для явных путей в конфиге:
/// `shared:` уже проверяется внутри [`resolve_model`]).
pub fn resolve_model_checked(root: &Path, spec: &str, role: &str) -> Result<PathBuf> {
    let p = resolve_model(root, spec, role)?;
    if p.is_file() {
        return Ok(p);
    }
    Err(EngineError::ModelNotFound {
        role: role.to_string(),
        searched: format!(
            "{} (значение конфига: «{}»; относительные пути резолвятся от корня проекта)",
            p.display(),
            spec.trim()
        ),
    })
}

/// Пути общего рантайма — для логов, `hdsw status` и проверок (`hds check`).
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root: PathBuf,
    pub bin: PathBuf,
    pub models: PathBuf,
}

impl RuntimePaths {
    pub fn new(root: PathBuf) -> Self {
        let bin = bin_dir(&root);
        let models = models_dir(&root, None);
        RuntimePaths { root, bin, models }
    }

    /// Пути из окружения (`LLAMA_RUNTIME_DIR` или ОС-умолчание).
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(runtime_dir()?))
    }

    /// Каталог моделей роли (`models/<role>`).
    pub fn models_for(&self, role: &str) -> PathBuf {
        self.models.join(role)
    }

    /// Имя бинаря llama-server для текущей ОС (в W2 не запускаем, но понадобится
    /// для отката на llama-server до конца волны).
    pub fn llama_server_name() -> &'static str {
        if cfg!(windows) {
            "llama-server.exe"
        } else {
            "llama-server"
        }
    }

    /// Есть ли `bin/llama-server` (для `hds check`: старый путь ещё жив).
    pub fn has_llama_server(&self) -> bool {
        self.bin.join(Self::llama_server_name()).is_file()
    }
}
