//! W4: настройки диаризации `llm_host` (`diarization.*`) и служебные функции
//! nemotron3-ветки.
//!
//! Почему отдельный модуль: `host.rs` — сборка живой машины и уборка, а правила
//! «что выбрать и как перевести YAML в параметры» должны тестироваться **без
//! GPU, ONNX и движка**. Сшивка сегментов и формат `.md`/`.srt` живут в
//! `hds-diarize` (`merge`); здесь — только перевод шкал, причём `trim_offset`
//! прибавляется к таймкодам ASR **ровно один раз** (`PLAN_DIARIZATION_NEMOTRON.md`
//! §3.4 п. 4).
//!
//! `ffmpeg` ищется здесь, а не через `hds_core::tools`: `hds-llama` намеренно не
//! зависит от `hds-core` (W3 handoff §5) — иначе `llm_host` тянул бы rusqlite и
//! весь индексный слой. Порядок поиска повторяет `hds_core::tools::find_ffmpeg`
//! (`index.ffmpeg_cmd` → `PATH` → стандартные каталоги машинной установки), чтобы
//! поведение на службах с минимальным `PATH` совпадало.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use hds_diarize::{
    merge_segments_with_turns, AsrSpan, Block, Device, NemotronConfig, TrimConfig, Turn,
};

use crate::asr_filter::FilterConfig;
use crate::whisper::WhisperSegment;

/// Метка «спикер не определён» — как отдаёт движок (формат `.md` не меняем).
pub const MD_UNASSIGNED: &str = "UNASSIGNED";

/// Режим ASR в nemotron3-ветке: нужны таймкоды для сшивки с turns.
pub const ASR_MODE_SUBTITLE: &str = "subtitle";

/// Backend диаризации (`diarization.backend`, решение владельца №4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Nemotron-3 (Sortformer v3) через `parakeet-rs` — новый путь (W4).
    Nemotron3,
    /// Прежний путь движка (GGUF sortformer) — регресс не трогаем.
    Sortformer,
    /// Диаризация выключена (как `diarization: false`).
    Off,
}

impl Backend {
    /// Разбор значения конфига/тела (`nemotron3`/`sortformer`/`off`).
    ///
    /// Список строгий: неизвестное значение — `None`, вызывающий решает, что
    /// делать (конфиг — авто с предупреждением, тело — конфиг).
    pub fn parse(s: &str) -> Option<Backend> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nemotron3" => Some(Backend::Nemotron3),
            "sortformer" => Some(Backend::Sortformer),
            "off" => Some(Backend::Off),
            _ => None,
        }
    }

    /// Имя для конфига, логов и `stats`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Nemotron3 => "nemotron3",
            Backend::Sortformer => "sortformer",
            Backend::Off => "off",
        }
    }
}

/// Параметры `diarization.nemotron3.*`.
#[derive(Debug, Clone, PartialEq)]
pub struct NemotronSettings {
    /// Путь к `.onnx`; `None`/пусто — автопоиск (`HDS_MODELS_DIR`, каталог движка).
    pub model: Option<PathBuf>,
    /// Устройство инференса (`auto` — DirectML/CoreML только для длинных файлов).
    pub device: Device,
    /// Размер блока `feed()`/`flush()`, минуты (память ~25 МБ/мин).
    pub chunk_min: f64,
    /// Короче этого — CPU даже при `auto`, минуты.
    pub min_audio_min: f64,
    /// `DiarizationConfig::custom(onset, offset)`.
    pub onset: f32,
    /// Второй порог постобработки.
    pub offset: f32,
}

impl Default for NemotronSettings {
    fn default() -> Self {
        NemotronSettings {
            model: None,
            device: Device::Auto,
            chunk_min: 10.0,
            min_audio_min: 2.0,
            onset: 0.5,
            offset: 0.5,
        }
    }
}

/// Параметры `diarization.asr.*` — обрезка/фильтр **входа ASR** (вариант A).
#[derive(Debug, Clone, PartialEq)]
pub struct AsrSettings {
    /// Резать ведущую тишину перед ASR (диаризация всегда идёт по полному WAV).
    pub trim_leading_silence: bool,
    /// Порог кадра, dBFS.
    pub silence_threshold_db: f64,
    /// Сколько оставить перед найденной речью, секунды.
    pub trim_lead_in_sec: f64,
    /// Энергетический постфильтр «реплик из тишины».
    pub drop_silent_segments: bool,
}

impl Default for AsrSettings {
    fn default() -> Self {
        AsrSettings {
            trim_leading_silence: true,
            silence_threshold_db: -50.0,
            trim_lead_in_sec: 0.3,
            drop_silent_segments: true,
        }
    }
}

/// Секция `diarization` конфига хоста (`PLAN_DIARIZATION_NEMOTRON.md` §3.5).
#[derive(Debug, Clone, PartialEq)]
pub struct DiarSettings {
    /// `None` — секции/ключа нет: backend выбирается автоматически (решение №10).
    pub backend: Option<Backend>,
    /// `required: true` — сбой диаризации валит задание (как сейчас).
    pub required: bool,
    /// Nemotron-3.
    pub nemotron3: NemotronSettings,
    /// Вход ASR.
    pub asr: AsrSettings,
}

impl Default for DiarSettings {
    fn default() -> Self {
        DiarSettings {
            backend: None,
            required: true,
            nemotron3: NemotronSettings::default(),
            asr: AsrSettings::default(),
        }
    }
}

impl DiarSettings {
    /// Разобрать секцию `diarization` из разобранного `config.yaml`.
    ///
    /// Ошибки не фатальны (хост не должен не подняться из-за опечатки в
    /// необязательной секции): возвращаем настройки + список предупреждений,
    /// которые `Host::start` кладёт в лог. Жёсткая проверка значений — в
    /// `hds check` (W5) и в `NemotronConfig::validate` (перед загрузкой модели).
    pub fn from_yaml(root: &serde_yaml::Value) -> (DiarSettings, Vec<String>) {
        let mut out = DiarSettings::default();
        let mut warnings = Vec::new();
        let Some(sec) = root.get("diarization") else {
            return (out, warnings);
        };
        if !sec.is_mapping() {
            warnings.push("секция diarization не словарь — беру значения по умолчанию".to_string());
            return (out, warnings);
        }

        if let Some(raw) = crate::host::dig_str(sec, "backend") {
            match Backend::parse(&raw) {
                Some(b) => out.backend = Some(b),
                None => warnings.push(format!(
                    "diarization.backend '{raw}' неизвестен (ожидается nemotron3|sortformer|off) — \
                     backend выберу автоматически"
                )),
            }
        }
        out.required = bool_at(sec, "required").unwrap_or(out.required);

        if let Some(raw) = crate::host::dig_str(sec, "nemotron3.model") {
            out.nemotron3.model = Some(PathBuf::from(raw));
        }
        if let Some(raw) = crate::host::dig_str(sec, "nemotron3.device") {
            match Device::parse(&raw) {
                Some(d) => out.nemotron3.device = d,
                None => warnings.push(format!(
                    "diarization.nemotron3.device '{raw}' неизвестен (auto|cpu|directml|coreml) — \
                     беру auto"
                )),
            }
        }
        if let Some(v) = crate::host::dig_f64(sec, "nemotron3.chunk_min") {
            if (2.0..=30.0).contains(&v) {
                out.nemotron3.chunk_min = v;
            } else {
                warnings.push(format!(
                    "diarization.nemotron3.chunk_min {v} вне 2..=30 — оставляю {}",
                    out.nemotron3.chunk_min
                ));
            }
        }
        if let Some(v) = crate::host::dig_f64(sec, "nemotron3.min_audio_min") {
            if v >= 0.0 {
                out.nemotron3.min_audio_min = v;
            } else {
                warnings.push(format!(
                    "diarization.nemotron3.min_audio_min {v} < 0 — оставляю {}",
                    out.nemotron3.min_audio_min
                ));
            }
        }
        if let Some(v) = crate::host::dig_f64(sec, "nemotron3.onset") {
            out.nemotron3.onset = v as f32;
        }
        if let Some(v) = crate::host::dig_f64(sec, "nemotron3.offset") {
            out.nemotron3.offset = v as f32;
        }

        out.asr.trim_leading_silence =
            bool_at(sec, "asr.trim_leading_silence").unwrap_or(out.asr.trim_leading_silence);
        if let Some(v) = crate::host::dig_f64(sec, "asr.silence_threshold_db") {
            if v < 0.0 {
                out.asr.silence_threshold_db = v;
            } else {
                warnings.push(format!(
                    "diarization.asr.silence_threshold_db {v} должен быть < 0 dBFS — оставляю {}",
                    out.asr.silence_threshold_db
                ));
            }
        }
        if let Some(v) = crate::host::dig_f64(sec, "asr.trim_lead_in_sec") {
            if v >= 0.0 {
                out.asr.trim_lead_in_sec = v;
            } else {
                warnings.push(format!(
                    "diarization.asr.trim_lead_in_sec {v} < 0 — оставляю {}",
                    out.asr.trim_lead_in_sec
                ));
            }
        }
        out.asr.drop_silent_segments =
            bool_at(sec, "asr.drop_silent_segments").unwrap_or(out.asr.drop_silent_segments);

        (out, warnings)
    }

    /// Параметры `Diarizer` (модель по умолчанию — `HDS_MODELS_DIR`/каталог движка).
    pub fn nemotron_config(&self) -> NemotronConfig {
        let n = &self.nemotron3;
        NemotronConfig {
            model: n
                .model
                .clone()
                .or_else(hds_diarize::default_model_path)
                .unwrap_or_default(),
            device: n.device,
            chunk_min: n.chunk_min,
            min_audio_min: n.min_audio_min,
            onset: n.onset,
            offset: n.offset,
        }
    }

    /// Параметры обрезки ведущей тишины (только для ASR).
    pub fn trim_config(&self) -> TrimConfig {
        TrimConfig::new(self.asr.silence_threshold_db, self.asr.trim_lead_in_sec)
    }

    /// Параметры энергетического постфильтра — порог тот же, что у обрезки.
    pub fn filter_config(&self) -> FilterConfig {
        FilterConfig {
            threshold_db: self.asr.silence_threshold_db,
            ..FilterConfig::default()
        }
    }
}

/// Выбор backend: тело запроса сильнее конфига (решение владельца 21.10.2026).
///
/// `body` — `diarization_backend` из `/internal/transcribe`; неизвестное значение
/// не ломает запрос, а уступает конфигу (предупреждение пишет вызывающий).
pub fn pick_backend(body: Option<&str>, configured: Backend) -> Backend {
    body.and_then(Backend::parse).unwrap_or(configured)
}

/// Сегменты ASR (шкала среза) + turns (абсолютная шкала) → блоки `.md`.
///
/// `trim_offset` прибавляется здесь **ровно один раз**; тест-инвариант §3.4 п. 4:
/// `merge_absolute(segments, turns, off) == merge(sift(segments, off), turns)`.
pub fn merge_absolute(segments: &[WhisperSegment], turns: &[Turn], trim_offset: f64) -> Vec<Block> {
    let spans: Vec<AsrSpan> = segments
        .iter()
        .map(|s| {
            AsrSpan::new(
                s.t_start + trim_offset,
                s.t_end + trim_offset,
                s.text.clone(),
            )
        })
        .collect();
    merge_segments_with_turns(&spans, turns)
}

/// Сколько разных спикеров среди turns (идентификаторы модели могут быть с
/// пропусками — считаем именно различные значения).
pub fn speaker_count(turns: &[Turn]) -> usize {
    turns
        .iter()
        .map(|t| t.speaker)
        .collect::<BTreeSet<_>>()
        .len()
}

/// Времена этапов задания (секунды) — уходят в `stats.timings_sec`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NemotronTimings {
    /// Декодирование ffmpeg (один проход).
    pub decode: f64,
    /// ASR по срезу.
    pub asr: f64,
    /// Диаризация по полному WAV.
    pub diar: f64,
    /// Всё задание целиком.
    pub total: f64,
}

/// `stats` nemotron3-ветки: числа задания поверх статистики движка.
#[derive(Debug, Clone, PartialEq)]
pub struct NemotronStats {
    /// Различных спикеров.
    pub num_speakers: usize,
    /// Turns диаризации (до сшивки).
    pub num_turns: usize,
    /// Сколько «реплик из тишины» отброшено.
    pub filtered_segments: usize,
    /// Времена этапов.
    pub timings: NemotronTimings,
}

impl NemotronStats {
    /// JSON для ответа: ключи движка (`tr.json["stats"]`) сохраняются, аддитивные
    /// ключи W4 добавляются поверх (старые клиенты их не знают и не ломаются).
    pub fn to_json(&self, engine_stats: Option<&Value>) -> Value {
        let mut out = engine_stats
            .filter(|v| v.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        out["num_speakers"] = json!(self.num_speakers);
        out["num_turns"] = json!(self.num_turns);
        out["filtered_segments"] = json!(self.filtered_segments);
        out["timings_sec"] = json!({
            "decode": round3(self.timings.decode),
            "asr": round3(self.timings.asr),
            "diar": round3(self.timings.diar),
            "total": round3(self.timings.total),
        });
        out
    }
}

/// Округление секунд до миллисекунд — чтобы `stats` читались глазами.
fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// `bool` из YAML по пути; не-`bool` — `None` (значение по умолчанию).
fn bool_at(root: &serde_yaml::Value, path: &str) -> Option<bool> {
    crate::host::dig(root, path).and_then(|v| v.as_bool())
}

/// Явный `index.ffmpeg_cmd` из конфига — только если файл существует (опечатка
/// в пути не должна «прятать» `ffmpeg` из `PATH`, как в `hds-core::tools`).
pub fn configured_ffmpeg(yaml: &serde_yaml::Value) -> Option<PathBuf> {
    crate::host::dig_str(yaml, "index.ffmpeg_cmd")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
}

/// Путь к CLI `ffmpeg`: явный конфиг → `PATH` → стандартные каталоги установки.
pub fn find_ffmpeg(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit.filter(|p| p.is_file()) {
        return Some(p.to_path_buf());
    }
    which("ffmpeg").or_else(|| standard_ffmpeg_paths().into_iter().find(|p| p.is_file()))
}

/// Поиск программы в `PATH` (на Windows дополнительно пробуется `.exe`).
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

/// Стандартные каталоги установки `ffmpeg` (кроме `PATH`) — как в `hds-core::tools`.
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
    // winget: пользовательская и машинная области — `Gyan.FFmpeg*\<ver>\bin`.
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

/// Временный каталог задания (`%TEMP%/hds_<имя>_<pid>_<n>`); удаляется на `Drop`.
///
/// Свой каталог на **каждое** задание: параллельные транскрибации не должны
/// перезаписывать WAV друг друга.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Создать каталог; имя уникально в пределах процесса.
    pub fn new(prefix: &str) -> std::io::Result<TempDir> {
        let base = std::env::temp_dir();
        for n in 0..100u32 {
            let path = base.join(format!("{prefix}_{}_{n}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(TempDir { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("{}: {e}", path.display()),
                    ))
                }
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "не удалось создать временный каталог {prefix}_* в {}",
                base.display()
            ),
        ))
    }

    /// Путь каталога.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value as Yaml;

    fn yaml(text: &str) -> Yaml {
        serde_yaml::from_str(text).expect("yaml")
    }

    /// Секции нет — дефолты, backend не задан (решение №10: авто).
    #[test]
    fn missing_section_keeps_defaults() {
        let (s, w) = DiarSettings::from_yaml(&yaml("index:\n  whisper_language: ru\n"));
        assert_eq!(s, DiarSettings::default());
        assert!(w.is_empty());
        assert_eq!(s.backend, None);
    }

    /// Полная секция §3.5 разбирается как есть.
    #[test]
    fn full_section_is_parsed() {
        let y = yaml(
            "diarization:\n\
             \x20 backend: nemotron3\n\
             \x20 required: false\n\
             \x20 nemotron3:\n\
             \x20   model: 'D:/m/nemotron3_diar_v3.onnx'\n\
             \x20   device: directml\n\
             \x20   chunk_min: 5\n\
             \x20   min_audio_min: 3\n\
             \x20   onset: 0.4\n\
             \x20   offset: 0.6\n\
             \x20 asr:\n\
             \x20   trim_leading_silence: false\n\
             \x20   silence_threshold_db: -45\n\
             \x20   trim_lead_in_sec: 0.5\n\
             \x20   drop_silent_segments: false\n",
        );
        let (s, w) = DiarSettings::from_yaml(&y);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(s.backend, Some(Backend::Nemotron3));
        assert!(!s.required);
        assert_eq!(
            s.nemotron3.model.as_deref(),
            Some(Path::new("D:/m/nemotron3_diar_v3.onnx"))
        );
        assert_eq!(s.nemotron3.device, Device::DirectMl);
        assert_eq!(s.nemotron3.chunk_min, 5.0);
        assert_eq!(s.nemotron3.min_audio_min, 3.0);
        assert!((s.nemotron3.onset - 0.4).abs() < 1e-6);
        assert!((s.nemotron3.offset - 0.6).abs() < 1e-6);
        assert!(!s.asr.trim_leading_silence);
        assert_eq!(s.asr.silence_threshold_db, -45.0);
        assert_eq!(s.asr.trim_lead_in_sec, 0.5);
        assert!(!s.asr.drop_silent_segments);
    }

    /// Неизвестный backend — предупреждение и «авто», а не падение хоста.
    #[test]
    fn unknown_backend_falls_back_to_auto() {
        let (s, w) = DiarSettings::from_yaml(&yaml("diarization:\n  backend: nemo\n"));
        assert_eq!(s.backend, None);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("nemo"), "{w:?}");
    }

    /// Неизвестное устройство — `auto` + предупреждение; `chunk_min` вне 2..=30
    /// не применяется (иначе `NemotronConfig::validate` уронил бы задание).
    #[test]
    fn invalid_values_are_warned_not_fatal() {
        let (s, w) = DiarSettings::from_yaml(&yaml(
            "diarization:\n  nemotron3:\n    device: cuda\n    chunk_min: 45\n",
        ));
        assert_eq!(s.nemotron3.device, Device::Auto);
        assert_eq!(s.nemotron3.chunk_min, 10.0);
        assert_eq!(w.len(), 2, "{w:?}");
    }

    /// Разбор и имена backend.
    #[test]
    fn backend_parse_and_names() {
        assert_eq!(Backend::parse("nemotron3"), Some(Backend::Nemotron3));
        assert_eq!(Backend::parse(" SortFormer "), Some(Backend::Sortformer));
        assert_eq!(Backend::parse("OFF"), Some(Backend::Off));
        assert_eq!(Backend::parse("nemotron"), None);
        assert_eq!(Backend::Nemotron3.as_str(), "nemotron3");
        assert_eq!(Backend::Off.as_str(), "off");
    }

    /// Тело `diarization_backend` сильнее конфига; мусор в теле — конфиг.
    #[test]
    fn body_backend_overrides_config() {
        assert_eq!(
            pick_backend(Some("nemotron3"), Backend::Sortformer),
            Backend::Nemotron3
        );
        assert_eq!(
            pick_backend(Some("bogus"), Backend::Sortformer),
            Backend::Sortformer
        );
        assert_eq!(pick_backend(None, Backend::Off), Backend::Off);
    }

    /// §3.4 п. 4: offset применяется ровно один раз — результат совпадает со
    /// сшивкой уже сдвинутых сегментов.
    #[test]
    fn offset_is_applied_once() {
        let turns = [
            Turn {
                start: 16.0,
                end: 20.0,
                speaker: 0,
            },
            Turn {
                start: 20.0,
                end: 30.0,
                speaker: 1,
            },
        ];
        // Сегменты в шкале среза (offset 16.28 с, как на 18-мин файле).
        let segments = [
            WhisperSegment {
                text: "Ольга,".into(),
                t_start: 1.14,
                t_end: 3.0,
            },
            WhisperSegment {
                text: "коллеги, добрый день.".into(),
                t_start: 5.7,
                t_end: 11.0,
            },
        ];
        let off = 16.28;
        let shifted: Vec<WhisperSegment> = segments
            .iter()
            .map(|s| WhisperSegment {
                text: s.text.clone(),
                t_start: s.t_start + off,
                t_end: s.t_end + off,
            })
            .collect();
        let a = merge_absolute(&segments, &turns, off);
        let b = merge_absolute(&shifted, &turns, 0.0);
        assert_eq!(a, b, "offset применён один раз");
        assert_eq!(a[0].speaker, Some(0), "17.42 с — первый спикер");
        assert_eq!(a[1].speaker, Some(1), "21.98 с — второй спикер");
    }

    /// Спикеры считаются по различным id, а не по количеству turns.
    #[test]
    fn speaker_count_is_distinct_ids() {
        let t = |speaker| Turn {
            start: 0.0,
            end: 1.0,
            speaker,
        };
        assert_eq!(speaker_count(&[]), 0);
        assert_eq!(speaker_count(&[t(0), t(0), t(2), t(5)]), 3);
    }

    /// `stats` движка сохраняются, аддитивные ключи W4 добавляются поверх.
    #[test]
    fn stats_merge_engine_keys() {
        let base = json!({ "tokens": 12, "timings": { "total": 44.0 } });
        let st = NemotronStats {
            num_speakers: 5,
            num_turns: 326,
            filtered_segments: 3,
            timings: NemotronTimings {
                decode: 0.6712,
                asr: 44.0,
                diar: 12.9345,
                total: 57.6,
            },
        };
        let v = st.to_json(Some(&base));
        assert_eq!(v["tokens"], 12);
        assert_eq!(v["timings"]["total"], 44.0);
        assert_eq!(v["num_speakers"], 5);
        assert_eq!(v["num_turns"], 326);
        assert_eq!(v["filtered_segments"], 3);
        assert_eq!(v["timings_sec"]["decode"], 0.671);
        assert_eq!(v["timings_sec"]["diar"], 12.935);
        // Нет статистики движка — просто объект с нашими ключами.
        let v = st.to_json(None);
        assert_eq!(v["num_speakers"], 5);
    }

    /// Явный `index.ffmpeg_cmd` с существующим файлом побеждает поиск в `PATH`.
    #[test]
    fn explicit_ffmpeg_wins_when_exists() {
        let dir = std::env::temp_dir().join(format!("hds-diar-ff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join(if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        std::fs::write(&fake, b"x").unwrap();
        let y = yaml(&format!(
            "index:\n  ffmpeg_cmd: '{}'\n",
            fake.display().to_string().replace('\\', "/")
        ));
        let explicit = configured_ffmpeg(&y).expect("явный путь");
        assert_eq!(explicit, fake);
        assert_eq!(find_ffmpeg(Some(&explicit)), Some(fake));
        // Несуществующий явный путь не отменяет поиска (результат зависит от машины).
        let missing = dir.join("no-such-ffmpeg");
        if let Some(p) = find_ffmpeg(Some(&missing)) {
            assert!(p.is_file(), "{}", p.display());
            assert_ne!(p, missing);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Временный каталог уникален и удаляется на `Drop`.
    #[test]
    fn temp_dir_is_unique_and_removed() {
        let a = TempDir::new("hds-diar-tmp").unwrap();
        let b = TempDir::new("hds-diar-tmp").unwrap();
        assert_ne!(a.path(), b.path());
        assert!(a.path().is_dir());
        let p = a.path().to_path_buf();
        drop(a);
        assert!(!p.exists(), "каталог задания удалён");
        drop(b);
    }

    /// `nemotron_config` берёт модель из секции и не теряет остальные поля.
    #[test]
    fn nemotron_config_maps_section() {
        let (s, _) = DiarSettings::from_yaml(&yaml(
            "diarization:\n  nemotron3:\n    model: 'D:/m/x.onnx'\n    device: cpu\n    chunk_min: 7\n",
        ));
        let cfg = s.nemotron_config();
        assert_eq!(cfg.model, PathBuf::from("D:/m/x.onnx"));
        assert_eq!(cfg.device, Device::Cpu);
        assert_eq!(cfg.chunk_min, 7.0);
        assert_eq!(cfg.min_audio_min, 2.0);
    }
}
