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

use hds_core::config::{dig, project_root, Config};
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
/// Режим автотранскрибации: `transcript` = транскрибация **и** диаризация одним вызовом.
pub const AUTO_MODE: &str = "transcript";
/// Формат выхода автотранскрибации по умолчанию — как отдаёт движок (`.md`, спайк T0.1).
pub const DEFAULT_OUT_FORMAT: &str = "md";
/// Метка неприсвоенной реплики в выводе движка (спайк T0.1, §5.4 плана).
pub const DEFAULT_UNASSIGNED_LABEL: &str = "UNASSIGNED";
/// `diarization_feed_ms` референса (спайк T0.1 §1).
pub const DEFAULT_DIARIZATION_FEED_MS: f64 = 10_800_001.0;
/// Таймаут запроса: длинные медиа транскрибируются минутами.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3600);

/// Настройки диаризации для автотранскрибации (`auto_transcribe.diarization*`, §5.1/§5.3).
///
/// `enabled` + `required` — не украшение: при `enabled: true` движок **требует**
/// sortformer-модель и отказывает без неё (жёсткий отказ, §0 п.6). Деградации в
/// `speech` нет — это подтверждено спайком T0.1.
#[derive(Debug, Clone)]
pub struct DiarizationSettings {
    /// `auto_transcribe.diarization` — включать диаризацию.
    pub enabled: bool,
    /// `auto_transcribe.diarization_model` — `.gguf`; `None`/пусто → владелец ищет сам.
    pub model: Option<PathBuf>,
    /// `auto_transcribe.diarization_backend` (референс: `sortformer`).
    pub backend: String,
    /// `auto_transcribe.diarization_feed_ms`.
    pub feed_ms: f64,
    /// `auto_transcribe.diarization_required` — нет модели ⇒ отказ задания.
    pub required: bool,
}

impl Default for DiarizationSettings {
    fn default() -> Self {
        DiarizationSettings {
            enabled: true,
            model: None,
            backend: "sortformer".to_string(),
            feed_ms: DEFAULT_DIARIZATION_FEED_MS,
            required: true,
        }
    }
}

/// Настройки медиа-ветки (`index.transcribe*`, `index.whisper_*`, `gpu.*`).
///
/// Поля `out_dir`/`out_format`/`unassigned_label`/`diarization` используются
/// **только** конвейером автотранскрибации (`auto_transcribe.*`, §5.3): медиа-ветка
/// индексации их не трогает и работает как раньше.
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
    /// Каталог вывода автотранскрибации (`auto_transcribe.out_dir`); `None` — не задан.
    pub out_dir: Option<PathBuf>,
    /// Формат выходного файла (`auto_transcribe.out_format`, обычно `md`).
    pub out_format: String,
    /// Метка неприсвоенной реплики (`auto_transcribe.unassigned_label`).
    pub unassigned_label: String,
    /// Диаризация (§5.1).
    pub diarization: DiarizationSettings,
    /// Каталог сервисных файлов (`<stem>.orig.<ext>`/`<stem>.speakers.json`):
    /// `auto_transcribe.state_dir` — чтобы `out_dir` содержал только выход.
    /// `None` — писать рядом с выходом (прежнее поведение, §5.3).
    pub service_dir: Option<PathBuf>,
}

impl Default for TranscribeConfig {
    fn default() -> Self {
        TranscribeConfig {
            url: DEFAULT_TRANSCRIBE_URL.to_string(),
            model: None,
            mode: DEFAULT_MODE.to_string(),
            custom: DEFAULT_CUSTOM.to_string(),
            gpu: 0,
            enabled: true,
            out_dir: None,
            out_format: DEFAULT_OUT_FORMAT.to_string(),
            unassigned_label: DEFAULT_UNASSIGNED_LABEL.to_string(),
            diarization: DiarizationSettings::default(),
            service_dir: None,
        }
    }
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
            .or_else(|| {
                dig(cfg, "gpu.device_index")
                    .and_then(|v| v.as_i64())
                    .map(config_index_to_bridge)
            })
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
            ..TranscribeConfig::default()
        }
    }

    /// Настройки конвейера **автотранскрибации** (`auto_transcribe.*`): режим
    /// `transcript`, диаризация sortformer, каталог вывода, метка `UNASSIGNED`.
    ///
    /// Отдельно от [`TranscribeConfig::from_config`] намеренно: боевая медиа-ветка
    /// индексации (`hds index`/`watch`) читает только `index.*` и не должна менять
    /// поведение из-за ключей автотранскрибации (Python-паритет B4).
    pub fn from_auto_transcribe(cfg: &Config) -> Self {
        let gpu = dig(cfg, "auto_transcribe.whisper_gpu")
            .and_then(|v| v.as_i64())
            .filter(|v| *v >= 0)
            .or_else(|| {
                dig(cfg, "gpu.device_index")
                    .and_then(|v| v.as_i64())
                    .map(config_index_to_bridge)
            })
            .unwrap_or(0) as i32;
        // Сервисные файлы (.orig/.speakers.json) — в state_dir, не в out_dir:
        // в выходной папке должен оставаться только результат (§5.3).
        let state_dir = dig_str(cfg, "auto_transcribe.state_dir")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "data/auto-transcribe".to_string());
        let state = PathBuf::from(&state_dir);
        let service_dir = Some(if state.is_absolute() {
            state
        } else {
            project_root().join(state)
        });
        TranscribeConfig {
            url: dig_str(cfg, "index.transcribe_url")
                .unwrap_or_else(|| DEFAULT_TRANSCRIBE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            model: dig_str(cfg, "auto_transcribe.whisper_model")
                .map(PathBuf::from)
                .or_else(|| resolve_whisper_model(cfg)),
            mode: dig_str(cfg, "auto_transcribe.mode").unwrap_or_else(|| AUTO_MODE.to_string()),
            custom: dig_scalar(cfg, "auto_transcribe.custom").unwrap_or_else(|| "auto".to_string()),
            gpu,
            enabled: true, // признак ветки индексации; конвейер автотранскрибации им не гейтится
            out_dir: dig_str(cfg, "auto_transcribe.out_dir").map(PathBuf::from),
            out_format: dig_str(cfg, "auto_transcribe.out_format")
                .unwrap_or_else(|| DEFAULT_OUT_FORMAT.to_string()),
            unassigned_label: dig_str(cfg, "auto_transcribe.unassigned_label")
                .unwrap_or_else(|| DEFAULT_UNASSIGNED_LABEL.to_string()),
            diarization: DiarizationSettings {
                enabled: dig(cfg, "auto_transcribe.diarization")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                model: dig_str(cfg, "auto_transcribe.diarization_model").map(PathBuf::from),
                backend: dig_str(cfg, "auto_transcribe.diarization_backend")
                    .unwrap_or_else(|| "sortformer".to_string()),
                feed_ms: dig(cfg, "auto_transcribe.diarization_feed_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(DEFAULT_DIARIZATION_FEED_MS),
                required: dig(cfg, "auto_transcribe.diarization_required")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            },
            service_dir,
        }
    }
}

/// `gpu.device_index` (семантика ролей, `hds-llama::device`: `0` = CPU, `1` = первый
/// GPU) → семантика тела `/internal/transcribe` (хост трактует `gpu` как bridge-индекс
/// CUDA: `0` = `CUDA0`, отрицательное = CPU; диаризации достаётся имя `CUDA{gpu}`).
///
/// Без маппинга конфиг-индекс `1` уходил в тело как есть: whisper попадал на
/// CPU-устройство (bridge-индекс `1` = CPU), а диаризация — на несуществующий
/// `CUDA1` → `failed to initialize backend: CUDA1` (автотранскрибация падала
/// с ретраями). Прямые значения `index.whisper_gpu`/`auto_transcribe.whisper_gpu`
/// проходим как есть (их семантику задаёт пользователь).
fn config_index_to_bridge(index: i64) -> i64 {
    if index <= 0 {
        -1
    } else {
        index - 1
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

    /// Автотранскрибация + диаризация файла с **записью** результата (§5.3 плана).
    ///
    /// 1. POST `/internal/transcribe` (`return_text: true`, `diarization*`);
    /// 2. записать полный текст в `out_dir/<stem>.<out_format>` (коллизии — `-2`, `-3`…);
    /// 3. sidecar `<stem>.speakers.json` (карта «плейсхолдер → имя», пока пустая) и
    ///    `<stem>.orig.<ext>` — текст **до** подстановки имён (нужен UI, §8);
    /// 4. **утилизацию исходника** делает демон, а не этот метод (§5.5).
    ///
    /// Метки спикеров пишутся **как отдаёт движок** (`SPEAKER_NN`, `UNASSIGNED`) —
    /// без преобразования (§5.4).
    pub fn transcribe_to_file(&self, path: &Path) -> Result<TranscribeOutcome> {
        let out_dir = self
            .cfg
            .out_dir
            .clone()
            .ok_or_else(|| CoreError::Other("не задан auto_transcribe.out_dir".to_string()))?;
        let (host, port, prefix) = split_base(&self.cfg.url)?;
        let target = format!("{prefix}/internal/transcribe");
        let mut body = json!({
            "path": path.to_string_lossy(),
            "mode": self.cfg.mode,
            "custom": self.cfg.custom,
            "gpu": self.cfg.gpu,
            "return_text": true,
        });
        if let Some(m) = &self.cfg.model {
            body["model"] = json!(m.to_string_lossy());
        }
        let d = &self.cfg.diarization;
        if d.enabled {
            body["diarization"] = json!(true);
            body["diarization_backend"] = json!(d.backend);
            body["diarization_feed_ms"] = json!(d.feed_ms);
            if let Some(m) = &d.model {
                body["diarization_model"] = json!(m.to_string_lossy());
            }
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
        let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
        if text.trim().is_empty() {
            return Err(CoreError::Other(format!(
                "владелец вернул пустой текст (mode={}, файл {})",
                self.cfg.mode,
                path.display()
            )));
        }
        let ext = v
            .get("out_ext")
            .and_then(|e| e.as_str())
            .map(|e| e.trim().trim_start_matches('.').to_string())
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| self.cfg.out_format.trim_start_matches('.').to_string());
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string();
        std::fs::create_dir_all(&out_dir)?;
        let svc_dir = self
            .cfg
            .service_dir
            .clone()
            .unwrap_or_else(|| out_dir.clone());
        std::fs::create_dir_all(&svc_dir)?;
        let out_path = next_free_path(&out_dir, &stem, &ext);
        write_text_atomic(&out_path, text)?;
        let used_stem = out_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&stem)
            .to_string();
        let orig_path = svc_dir.join(format!("{used_stem}.orig.{ext}"));
        write_text_atomic(&orig_path, text)?;
        let speakers = detect_speaker_slots(text);
        let mut map = serde_json::Map::new();
        for s in &speakers {
            map.insert(s.clone(), json!(""));
        }
        let sidecar = json!({
            "speakers": serde_json::Value::Object(map),
            "unassigned_label": self.cfg.unassigned_label,
            "source": path.to_string_lossy(),
            "out": out_path.to_string_lossy(),
        });
        let speakers_path = svc_dir.join(format!("{used_stem}.speakers.json"));
        let pretty = serde_json::to_string_pretty(&sidecar)
            .map_err(|e| CoreError::Other(format!("speakers.json: {e}")))?;
        write_text_atomic(&speakers_path, &pretty)?;
        Ok(TranscribeOutcome {
            out_path,
            orig_path,
            speakers_path,
            speakers,
            chars: text.chars().count(),
        })
    }
}

/// Итог [`TranscribeClient::transcribe_to_file`]: что и куда записано (§5.3).
#[derive(Debug, Clone)]
pub struct TranscribeOutcome {
    /// Записанный выход (`out_dir/<stem>.<out_format>`).
    pub out_path: PathBuf,
    /// Копия текста **до** подстановки имён (`out_dir/<stem>.orig.<ext>`).
    pub orig_path: PathBuf,
    /// Карта спикеров (`out_dir/<stem>.speakers.json`).
    pub speakers_path: PathBuf,
    /// Обнаруженные метки спикеров в порядке первого появления.
    pub speakers: Vec<String>,
    /// Длина текста в символах (критерий «выход непустой» перед утилизацией, §5.5).
    pub chars: usize,
}

/// Метка спикера формата движка: `SPEAKER_<цифры>` или литерал `UNASSIGNED`.
///
/// Рукописный разбор **без `regex`** (новые зависимости в проекте не вводим, §12);
/// регистр не нормализуем — движок отдаёт `SPEAKER_00`/`UNASSIGNED` (спайк T0.1).
pub fn is_speaker_token(token: &str) -> bool {
    if token == DEFAULT_UNASSIGNED_LABEL {
        return true;
    }
    match token.strip_prefix("SPEAKER_") {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

/// Метки спикеров из **заголовков реплик**, в порядке первого появления.
///
/// Формат движка (спайк T0.1): `### SPEAKER_00 [hh:mm:ss - hh:mm:ss]`, неприсвоенная
/// реплика — `### UNASSIGNED [ … ]`. Сканируем только строки-заголовки (`### ` +
/// первый токен): так под замену имён не попадают упоминания метки в тексте.
pub fn detect_speaker_slots(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("###") else {
            continue;
        };
        let token = rest.split_whitespace().next().unwrap_or("");
        if !is_speaker_token(token) {
            continue;
        }
        if !out.iter().any(|s| s == token) {
            out.push(token.to_string());
        }
    }
    out
}

/// Свободный путь `dir/<stem>.<ext>`; при коллизии — `dir/<stem>-2.<ext>`, `-3`… (§5.3).
pub fn next_free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 2..10_000u32 {
        let cand = dir.join(format!("{stem}-{n}.{ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    first
}

/// Атомарная запись текста: временный файл рядом + `replace_file` (ретраи на Windows).
fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or("dat"),
        std::process::id()
    ));
    std::fs::write(&tmp, text)?;
    hds_core::config::replace_file(&tmp, path)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `gpu.device_index` (конфиг-семантика: 0 = CPU, 1 = первый GPU) маппится в
    /// семантику тела `/internal/transcribe` (bridge-индекс CUDA / `-1` = CPU) —
    /// иначе диаризация получала несуществующее `CUDA1` (баг 04.10.2026).
    #[test]
    fn device_index_fallback_maps_to_bridge() {
        let cfg: hds_core::config::Config =
            serde_yaml::from_str("gpu:\n  device_index: 1\n").unwrap();
        assert_eq!(
            TranscribeConfig::from_config(&cfg).gpu,
            0,
            "первый GPU → CUDA0"
        );
        assert_eq!(
            TranscribeConfig::from_auto_transcribe(&cfg).gpu,
            0,
            "auto_transcribe: первый GPU → CUDA0"
        );
        let cpu: hds_core::config::Config =
            serde_yaml::from_str("gpu:\n  device_index: 0\n").unwrap();
        assert_eq!(TranscribeConfig::from_config(&cpu).gpu, -1, "CPU → -1");
        // Прямые значения whisper_gpu проходят как есть (семантику задаёт пользователь).
        let direct: hds_core::config::Config =
            serde_yaml::from_str("index:\n  whisper_gpu: 2\ngpu:\n  device_index: 1\n").unwrap();
        assert_eq!(TranscribeConfig::from_config(&direct).gpu, 2);
    }

    /// Сканер: заголовки `### SPEAKER_NN`/`### UNASSIGNED`, порядок первого появления,
    /// без дублей; упоминания меток в теле текста игнорируются (§5.4, спайк T0.1).
    #[test]
    fn speaker_slots_are_scanned_from_turn_headers() {
        let text = "### SPEAKER_01 [00:00:01 - 00:00:24]\n\
                    Привет, SPEAKER_00 — это не заголовок.\n\n\
                    ### SPEAKER_00 [00:00:25 - 00:00:54]\nОтвет.\n\n\
                    ### UNASSIGNED [00:02:12 - 00:02:13]\nХвост.\n\n\
                    ### SPEAKER_01 [00:02:14 - 00:02:20]\nСнова я.\n";
        assert_eq!(
            detect_speaker_slots(text),
            vec!["SPEAKER_01", "SPEAKER_00", "UNASSIGNED"]
        );
    }

    #[test]
    fn speaker_token_recognizes_only_engine_labels() {
        assert!(is_speaker_token("SPEAKER_00"));
        assert!(is_speaker_token("SPEAKER_123"));
        assert!(is_speaker_token("UNASSIGNED"));
        assert!(!is_speaker_token("SPEAKER_"));
        assert!(!is_speaker_token("SPEAKER_0A"));
        assert!(
            !is_speaker_token("speaker_00"),
            "регистр — как отдаёт движок"
        );
        assert!(!is_speaker_token("UNKNOWN"));
        assert!(!is_speaker_token("###"));
    }

    /// Без строк-заголовков (`### `) меток нет — сканируем только реплики.
    #[test]
    fn detect_ignores_plain_speaker_tokens() {
        assert!(detect_speaker_slots("SPEAKER_00 просто текст\n").is_empty());
    }

    #[test]
    fn next_free_path_adds_suffix_on_collision() {
        let dir = std::env::temp_dir().join(format!("hds-next-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let name = |p: &Path| p.file_name().unwrap().to_str().unwrap().to_string();
        let first = next_free_path(&dir, "клип", "md");
        assert_eq!(name(&first), "клип.md");
        std::fs::write(&first, "x").unwrap();
        let second = next_free_path(&dir, "клип", "md");
        assert_eq!(name(&second), "клип-2.md");
        std::fs::write(&second, "x").unwrap();
        assert_eq!(name(&next_free_path(&dir, "клип", "md")), "клип-3.md");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `from_auto_transcribe` читает `auto_transcribe.*` (опции диаризации — наши,
    /// не из медиа-ветки индексации).
    #[test]
    fn auto_transcribe_config_reads_keys() {
        let cfg: Config = serde_yaml::from_str(
            "auto_transcribe:\n  mode: transcript\n  out_dir: 'D:\\out'\n  out_format: md\n  \
             unassigned_label: UNASSIGNED\n  diarization: true\n  \
             diarization_backend: sortformer\n  diarization_feed_ms: 10800001\n  whisper_gpu: 1\n",
        )
        .unwrap();
        let tc = TranscribeConfig::from_auto_transcribe(&cfg);
        assert_eq!(tc.mode, "transcript");
        assert_eq!(tc.out_format, "md");
        assert_eq!(tc.gpu, 1);
        assert_eq!(tc.diarization.backend, "sortformer");
        assert_eq!(tc.diarization.feed_ms, 10_800_001.0);
        assert!(tc.diarization.enabled && tc.diarization.required);
        assert_eq!(tc.out_dir.as_deref(), Some(Path::new("D:\\out")));
    }

    /// Индексная ветка (`index.whisper_gpu: -1`) не включает диаризацию и не читает
    /// `auto_transcribe.*` — паритет боевого `watch`/`index` не тронут.
    #[test]
    fn index_branch_config_ignores_auto_transcribe_section() {
        let cfg: Config = serde_yaml::from_str(
            "index:\n  whisper_mode: speech\nauto_transcribe:\n  mode: transcript\n",
        )
        .unwrap();
        let tc = TranscribeConfig::from_config(&cfg);
        assert_eq!(tc.mode, "speech");
        assert_eq!(tc.out_dir, None);
        assert_eq!(tc.out_format, DEFAULT_OUT_FORMAT);
    }
}
