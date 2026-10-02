//! W3: медиа-ветка конвейера — аудио/видео транскрибирует **владелец GPU**
//! (`llm-host`, роль `whisper`), а конвейер индексации (`hds index`/`watch`)
//! лишь получает сегменты по HTTP `/internal/transcribe` и кладёт их в чанки.
//!
//! Почему так (факты W3, `tools/parity/W3_REPORT.md` §0–§3):
//! * кросс-процессной адресации инстансов движка нет → второй владелец GPU
//!   недопустим (§8.6.2 плана); транскрибирует тот, кто уже владеет движком;
//! * аудио-путь кластера требует execution group, поэтому `llm-host` делает это
//!   через bridge-API (`crates/hds-llama/src/bridge_audio.rs`/`whisper.rs`);
//! * `hds-index` не зависит от `hds-llama` — общение только по HTTP
//!   (`hds_core::http`), как с эмбеддингами (`crate::embed`).
//!
//! [`MediaRouter`] — обёртка-`Extractor`: для вида `media` зовёт фасад, для всех
//! прочих видов делегирует Python-воркеру (`crate::sidecar`). Так боевой путь
//! (`hds index`/`watch`) отправляет медиа на движок, а паритет B4 с golden
//! (снятым Python faster-whisper) остаётся нетронутым — там используется чистый
//! `Sidecar`.
//!
//! Поведение не-медиа и отказов сохранено с Python (`hds/extract_av.py`):
//! ведущий сегмент `"Медиафайл: <имя>\n<ffprobe>"`; при выключенном
//! `index.transcribe` — только он; при недоступном владельце/сбое — плюс
//! сегмент-сообщение, но файл **не** помечается ошибкой (деградация без падения).

use std::path::{Path, PathBuf};
use std::time::Duration;

use hds_core::config::{dig, Config};
use hds_core::error::{CoreError, Result};
use hds_core::http;
use serde_json::json;

use crate::chunker::Segment;
use crate::embed::split_base;
use crate::kinds;
use crate::sidecar::Extractor;

/// Адрес владельца GPU по умолчанию (`llm-host` на боевом чат-порту).
pub const DEFAULT_TRANSCRIBE_URL: &str = "http://127.0.0.1:8010";
/// Режим по умолчанию — `subtitle` (движок пишет `.srt` с таймкодами).
pub const DEFAULT_MODE: &str = "subtitle";
/// Окно субтитров по умолчанию (`custom`, секунды).
pub const DEFAULT_CUSTOM: &str = "4.5";
/// Таймаут запроса: длинные медиа транскрибируются минутами.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3600);

/// Настройки медиа-ветки (`index.transcribe*`, `index.whisper_*`, `gpu.*`).
#[derive(Debug, Clone)]
pub struct TranscribeConfig {
    /// Базовый URL владельца GPU (`index.transcribe_url`), без хвостового `/`.
    pub url: String,
    /// Путь к whisper-модели (GGML `.bin`); `None` — владелец разрешает сам.
    pub model: Option<PathBuf>,
    /// Режим движка: `subtitle` (таймкоды) или `speech` (сплошной текст).
    pub mode: String,
    /// Окно субтитров `custom` (`default`/`auto`/число-строкой), секунды.
    pub custom: String,
    /// Индекс GPU (или `-1` — CPU).
    pub gpu: i32,
    /// `index.transcribe` — включена ли транскрипция.
    pub enabled: bool,
}

impl TranscribeConfig {
    /// Собрать из конфига (`index.*`/`gpu.*`), с безопасными умолчаниями.
    pub fn from_config(cfg: &Config) -> Self {
        let url = dig_str(cfg, "index.transcribe_url")
            .unwrap_or_else(|| DEFAULT_TRANSCRIBE_URL.to_string());
        let url = url.trim_end_matches('/').to_string();
        let model = resolve_whisper_model(cfg);
        let mode = dig_str(cfg, "index.whisper_mode").unwrap_or_else(|| DEFAULT_MODE.to_string());
        let custom =
            dig_scalar(cfg, "index.whisper_custom").unwrap_or_else(|| DEFAULT_CUSTOM.to_string());
        let gpu = dig(cfg, "index.whisper_gpu")
            .and_then(|v| v.as_i64())
            .or_else(|| dig(cfg, "gpu.device_index").and_then(|v| v.as_i64()))
            .unwrap_or(0) as i32;
        let enabled = dig(cfg, "index.transcribe")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        TranscribeConfig {
            url,
            model,
            mode,
            custom,
            gpu,
            enabled,
        }
    }
}

/// Строка из конфига (`None` — нет ключа/не строка/пусто).
fn dig_str(cfg: &Config, dotted: &str) -> Option<String> {
    dig(cfg, dotted)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Скаляр (`строка` или `число`) как строка — для `custom` (`4.5`) и т.п.
fn dig_scalar(cfg: &Config, dotted: &str) -> Option<String> {
    let v = dig(cfg, dotted)?;
    if let Some(s) = v.as_str() {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    if let Some(i) = v.as_i64() {
        return Some(i.to_string());
    }
    if let Some(f) = v.as_f64() {
        return Some(f.to_string());
    }
    None
}

/// Каталог моделей движка на этой машине (`%APPDATA%\OpenResearchTools\models`).
fn engine_models_dir() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    let root = PathBuf::from(appdata)
        .join("OpenResearchTools")
        .join("models");
    if root.is_dir() {
        Some(root)
    } else {
        None
    }
}

/// Первый `*.bin`/`*.gguf` в каталоге (рекурсивно, `*.bin` в приоритете).
fn first_weight_in(dir: &Path) -> Option<PathBuf> {
    let mut fallback: Option<PathBuf> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(found) = first_weight_in(&p) {
                return Some(found);
            }
            continue;
        }
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| x.to_ascii_lowercase());
        match ext.as_deref() {
            Some("bin") => return Some(p),
            Some("gguf") => fallback = Some(p),
            _ => {}
        }
    }
    fallback
}

/// Каталог whisper-модели под `%APPDATA%\OpenResearchTools\models` (имя содержит
/// `whisper`), из него — веса. Порт `default_whisper_model` из `hds-llama`.
fn engine_whisper_model() -> Option<PathBuf> {
    let root = engine_models_dir()?;
    for e in std::fs::read_dir(&root).ok()?.flatten() {
        let dir = e.path();
        let is_whisper = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase().contains("whisper"))
            .unwrap_or(false);
        if dir.is_dir() && is_whisper {
            if let Some(found) = first_weight_in(&dir) {
                return Some(found);
            }
        }
    }
    None
}

/// Разрешить путь к whisper-модели (GGML `.bin`).
///
/// Порядок (совместимость с Python-ключом `index.whisper_model`, который мог быть
/// и путём, и именем размера, и каталогом):
/// 1. `index.whisper_model` — существующий файл или каталог с весами;
/// 2. `index.whisper_dir` — каталог с весами;
/// 3. каталог моделей движка (`%APPDATA%\OpenResearchTools\models\*whisper*`).
pub fn resolve_whisper_model(cfg: &Config) -> Option<PathBuf> {
    if let Some(s) = dig_str(cfg, "index.whisper_model") {
        let p = PathBuf::from(&s);
        if p.is_file() {
            return Some(p);
        }
        if p.is_dir() {
            if let Some(found) = first_weight_in(&p) {
                return Some(found);
            }
        }
    }
    if let Some(dir) = dig_str(cfg, "index.whisper_dir") {
        let p = PathBuf::from(&dir);
        if p.is_dir() {
            if let Some(found) = first_weight_in(&p) {
                return Some(found);
            }
        }
    }
    engine_whisper_model()
}

/// Клиент внутренней транскрибации (`POST /internal/transcribe`).
#[derive(Debug, Clone)]
pub struct TranscribeClient {
    cfg: TranscribeConfig,
}

impl TranscribeClient {
    /// Новый клиент с явными настройками (для тестов и `whisper-check`).
    pub fn new(cfg: TranscribeConfig) -> Self {
        TranscribeClient { cfg }
    }

    /// Настройки (адрес/модель/режим) — для сообщений и проверок.
    pub fn config(&self) -> &TranscribeConfig {
        &self.cfg
    }

    /// Транскрибация файла через владельца GPU → сегменты с таймкодами.
    ///
    /// Ошибку (недоступный владелец/не-200) возвращаем как `Err` — вызывающий
    /// ([`MediaRouter`]) превращает её в сегмент-сообщение, не роняя файл.
    pub fn transcribe(&self, path: &Path) -> Result<Vec<Segment>> {
        let (host, port, prefix) = split_base(&self.cfg.url)?;
        let target = format!("{prefix}/internal/transcribe");
        let mut body = json!({
            "path": path.to_string_lossy(),
            "mode": self.cfg.mode,
            "custom": self.cfg.custom,
            "gpu": self.cfg.gpu,
        });
        if let Some(m) = &self.cfg.model {
            body["model"] = json!(m.to_string_lossy());
        }
        let resp = http::request(
            &host,
            port,
            "POST",
            &target,
            &[("Content-Type", "application/json")],
            Some(&body.to_string()),
            REQUEST_TIMEOUT,
        )?;
        if resp.status != 200 {
            return Err(CoreError::Other(format!(
                "владелец ответил статусом {}: {}",
                resp.status,
                resp.body.chars().take(300).collect::<String>()
            )));
        }
        let v = resp.json()?;
        Ok(segments_from_json(&v))
    }
}

/// Разбор ответа `/internal/transcribe` (`segments:[{text,t_start,t_end}]`).
pub fn segments_from_json(v: &serde_json::Value) -> Vec<Segment> {
    let mut out = Vec::new();
    let Some(arr) = v.get("segments").and_then(|s| s.as_array()) else {
        return out;
    };
    for s in arr {
        let text = s
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        out.push(Segment {
            text,
            page: None,
            t_start: s.get("t_start").and_then(|x| x.as_f64()),
            t_end: s.get("t_end").and_then(|x| x.as_f64()),
            head: None,
        });
    }
    out
}

/// Ведущий сегмент медиафайла — порт `extract_media`:
/// `"Медиафайл: <имя>\n<ffprobe|метаданные недоступны>"`.
pub fn lead_segment(path: &Path) -> Segment {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let meta = ffprobe_meta(path).unwrap_or_else(|| "метаданные недоступны".to_string());
    Segment {
        text: format!("Медиафайл: {name}\n{meta}"),
        page: None,
        t_start: None,
        t_end: None,
        head: None,
    }
}

/// `ffprobe`-метаданные из PATH (порт `hds/extract_av.py::_ffprobe`), best-effort.
fn ffprobe_meta(path: &Path) -> Option<String> {
    let ff = which("ffprobe")?;
    let mut cmd = std::process::Command::new(ff);
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=codec_name,width,height",
        "-of",
        "default=noprint_wrappers=1",
    ])
    .arg(path)
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Поиск программы в `PATH` (`shutil.which`): с `.exe` на Windows.
fn which(prog: &str) -> Option<PathBuf> {
    use std::path::Path as StdPath;
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(prog);
        if StdPath::new(&cand).is_file() {
            return Some(cand);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{prog}.exe"));
            if StdPath::new(&exe).is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// Обёртка-`Extractor`: медиа → владелец GPU, всё прочее → Python-воркер.
pub struct MediaRouter<E: Extractor> {
    inner: E,
    client: TranscribeClient,
    enabled: bool,
}

impl<E: Extractor> MediaRouter<E> {
    /// Построить из конфига (обёртка над готовым извлекателем).
    pub fn new(inner: E, cfg: &Config) -> Self {
        let tcfg = TranscribeConfig::from_config(cfg);
        let enabled = tcfg.enabled;
        MediaRouter {
            inner,
            client: TranscribeClient::new(tcfg),
            enabled,
        }
    }

    /// Явные настройки (для тестов/`whisper-check`).
    pub fn with_client(inner: E, client: TranscribeClient, enabled: bool) -> Self {
        MediaRouter {
            inner,
            client,
            enabled,
        }
    }

    /// Клиент владельца (доступ к настройкам).
    pub fn client(&self) -> &TranscribeClient {
        &self.client
    }

    /// Транскрибация одного медиафайла без обращения к `inner`:
    /// ведущий сегмент + сегменты владельца (или сообщение о сбое).
    pub fn transcribe_media(&self, path: &Path) -> Vec<Segment> {
        let mut segs = vec![lead_segment(path)];
        if !self.enabled {
            return segs;
        }
        match self.client.transcribe(path) {
            Ok(mut t) => segs.append(&mut t),
            Err(e) => {
                let msg = e.message();
                let low = msg.to_ascii_lowercase();
                let head = if low.contains("connect") || low.contains("адрес") {
                    "Транскрипция недоступна"
                } else {
                    "Транскрипция не удалась"
                };
                segs.push(Segment {
                    text: format!("{head}: {msg}"),
                    page: None,
                    t_start: None,
                    t_end: None,
                    head: None,
                });
            }
        }
        segs
    }
}

impl<E: Extractor> Extractor for MediaRouter<E> {
    fn extract(&self, path: &Path) -> Result<(String, Vec<Segment>)> {
        if kinds::kind_of_path(path) == Some("media") {
            return Ok(("media".to_string(), self.transcribe_media(path)));
        }
        self.inner.extract(path)
    }
}
