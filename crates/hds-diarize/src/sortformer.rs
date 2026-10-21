//! Nemotron-3 Diarization (Sortformer v3) через `parakeet-rs` + ONNX Runtime.
//!
//! `PLAN_DIARIZATION_NEMOTRON.md` W3. Что важно знать (проверено на исходнике
//! `parakeet-rs 0.4.2`, `src/sortformer.rs`):
//! * `feed()`/`flush()` возвращают **абсолютные** таймкоды (внутренний
//!   `elapsed_samples`), поэтому склейка блоков не требует пересчёта;
//! * `feed()` сам буферизует окна `chunk_len + right_context` и шагает на
//!   `chunk_len`, состояние (spkcache/FIFO) сохраняется между вызовами — это и
//!   есть чанкинг без роста памяти;
//! * при перекрытии окон один и тот же отрезок может прийти дважды ⇒ одинаковые
//!   соседние turns склеиваются [`segments_to_turns`].
//!
//! Вход — WAV 16 кГц mono (формат декодера варианта A). EP выбирается по
//! платформе и длительности: на коротких клипах DirectML проигрывает CPU
//! (оверхед сессии, спайк §6), поэтому `auto` включает GPU только с
//! `min_audio_min`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use parakeet_rs::ort::environment::Environment;
use parakeet_rs::sortformer::{DiarizationConfig, Sortformer, SpeakerSegment};
use parakeet_rs::{ExecutionConfig, ExecutionProvider};
use sha2::{Digest, Sha256};

use crate::{ort_env, DiarizeError, Result, SAMPLE_RATE};

/// Ожидаемый размер модели Nemotron-3 (байт) — быстрая проверка до хеширования.
pub const NEMOTRON_SIZE: u64 = 400_506_656;
/// sha256 модели `nemotron3_diar_v3.onnx` (пин, `DIARIZATION_NEMOTRON_HANDOFF.md`).
pub const NEMOTRON_SHA256: &str =
    "915e4fa23b0192ed9fadeb1cdd26847df986d50c92012d177be28d0343bbe03a";
/// Имя файла модели в каталоге `<models>/nemotron3/`.
pub const NEMOTRON_FILE: &str = "nemotron3_diar_v3.onnx";

/// Turn диаризации: интервал и номер спикера (0…7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Turn {
    /// Начало, секунды от начала файла.
    pub start: f64,
    /// Конец, секунды.
    pub end: f64,
    /// `speaker_id` модели (стабилен между блоками благодаря AOSC-кэшу).
    pub speaker: u32,
}

impl Turn {
    /// Длительность, секунды.
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    /// Пустой интервал (для фильтрации).
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// Устройство инференса (значение конфига `diarization.nemotron3.device`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// DirectML на Windows / CoreML на macOS для длинных файлов, иначе CPU.
    Auto,
    /// Только CPU.
    Cpu,
    /// DirectML (Windows).
    DirectMl,
    /// CoreML (macOS).
    CoreMl,
}

impl Device {
    /// Разбор строки из конфига (`auto`/`cpu`/`directml`/`coreml`).
    pub fn parse(s: &str) -> Option<Device> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "auto" => Some(Device::Auto),
            "cpu" => Some(Device::Cpu),
            "directml" | "dml" => Some(Device::DirectMl),
            "coreml" | "core_ml" => Some(Device::CoreMl),
            _ => None,
        }
    }

    /// Имя для логов и `stats`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Device::Auto => "auto",
            Device::Cpu => "cpu",
            Device::DirectMl => "directml",
            Device::CoreMl => "coreml",
        }
    }
}

/// Параметры диаризации (`diarization.nemotron3.*`).
#[derive(Debug, Clone)]
pub struct NemotronConfig {
    /// Путь к `.onnx` модели.
    pub model: PathBuf,
    /// Устройство (`auto` — DirectML/CoreML только для длинных файлов).
    pub device: Device,
    /// Размер блока `feed()`, минуты (память ~25 МБ/мин при `diarize()` целиком).
    pub chunk_min: f64,
    /// Короче этого — CPU даже при `auto` (оверхед сессии DirectML).
    pub min_audio_min: f64,
    /// `DiarizationConfig::custom(onset, offset)`.
    pub onset: f32,
    /// Второй порог постобработки.
    pub offset: f32,
}

impl Default for NemotronConfig {
    fn default() -> Self {
        NemotronConfig {
            model: default_model_path().unwrap_or_default(),
            device: Device::Auto,
            chunk_min: 10.0,
            min_audio_min: 2.0,
            onset: 0.5,
            offset: 0.5,
        }
    }
}

impl NemotronConfig {
    /// Конфиг с явной моделью и остальными дефолтами.
    pub fn new(model: impl Into<PathBuf>) -> Self {
        NemotronConfig {
            model: model.into(),
            ..NemotronConfig::default()
        }
    }

    /// Проверка значений (до загрузки модели).
    pub fn validate(&self) -> Result<()> {
        if self.model.as_os_str().is_empty() {
            return Err(DiarizeError::Format(format!(
                "не задан путь к модели Nemotron-3 (ожидается <models>/nemotron3/{NEMOTRON_FILE})"
            )));
        }
        if !self.model.is_file() {
            return Err(DiarizeError::Io(format!(
                "модель Nemotron-3 не найдена: {}",
                self.model.display()
            )));
        }
        if !(2.0..=30.0).contains(&self.chunk_min) {
            return Err(DiarizeError::Format(format!(
                "chunk_min должен быть 2..=30, получено {}",
                self.chunk_min
            )));
        }
        if self.min_audio_min < 0.0 {
            return Err(DiarizeError::Format("min_audio_min < 0".to_string()));
        }
        // F12 (L3): пороги постобработки — доли 0..=1; `onset: 5`/`offset: -1`
        // раньше тихо уходили в модель.
        for (name, v) in [("onset", self.onset), ("offset", self.offset)] {
            if !(0.0..=1.0).contains(&v) {
                return Err(DiarizeError::Format(format!(
                    "{name} должен быть 0..=1, получено {v}"
                )));
            }
        }
        Ok(())
    }
}

/// Каталоги моделей в порядке приоритета: `HDS_MODELS_DIR` (машинный, для служб под
/// `LocalSystem`) → `%ProgramData%\hermes-disk-search\models` (машинная установка,
/// W6: туда пишет `fetch_nemotron3_model.ps1`) → `%APPDATA%\OpenResearchTools\models`
/// (desktop, общий каталог движка).
///
/// Единый порядок для `llm_host` (через [`default_model_path`]) и `hds-index`
/// (`hds check`, поиск sortformer-модели) — иначе служба не увидит модель,
/// поставленную машинно (W5).
pub fn models_roots() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    if let Some(v) = std::env::var_os("HDS_MODELS_DIR") {
        let p = PathBuf::from(v);
        if !p.as_os_str().is_empty() {
            out.push(p);
        }
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        let p = PathBuf::from(pd);
        if !p.as_os_str().is_empty() {
            out.push(p.join("hermes-disk-search").join("models"));
        }
    }
    if let Some(base) = directories::BaseDirs::new() {
        out.push(base.config_dir().join("OpenResearchTools").join("models"));
    }
    out
}

/// Путь к модели Nemotron-3: первый существующий `<root>/nemotron3/{NEMOTRON_FILE}`
/// по [`models_roots`]; если файла нет нигде — первый каталог-кандидат (чтобы в
/// ошибке был осмысленный путь).
pub fn default_model_path() -> Option<PathBuf> {
    let roots = models_roots();
    if let Some(p) = roots
        .iter()
        .map(|r| r.join("nemotron3").join(NEMOTRON_FILE))
        .find(|p| p.is_file())
    {
        return Some(p);
    }
    roots
        .first()
        .map(|r| r.join("nemotron3").join(NEMOTRON_FILE))
}

/// Результат проверки модели: `(путь, размер, mtime)` → `Ok`/причина отказа.
///
/// F6 (ревью M3): кэшируется и **отрицательный** результат. Раньше `Ok` попадал
/// в кэш только при успехе, поэтому битый файл (обрезанная закачка, подменённый
/// LICENSE) хешировался заново на **каждом** задании: при `required: false` это
/// тихая постоянная просадка времени.
type Verified = Mutex<HashMap<(PathBuf, u64, u64), std::result::Result<(), String>>>;
static VERIFIED: OnceLock<Verified> = OnceLock::new();

/// sha256 файла (потоково, по 1 МБ — 400 МБ хешируются ~1–2 с).
pub fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)
        .map_err(|e| DiarizeError::Io(format!("{}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| DiarizeError::Io(format!("{}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Проверить размер и sha256 модели Nemotron-3 (константы из W3).
pub fn verify_model(path: &Path) -> Result<()> {
    verify_against(path, NEMOTRON_SIZE, NEMOTRON_SHA256)
}

/// Проверить файл против ожидаемых размера и sha256.
///
/// Результат кэшируется по `(путь, размер, mtime)` — и успех, **и** отказ
/// (F6/M3). Ошибка чтения/metadata не кэшируется: файл могут докачать.
/// Файл иного размера отсекается **до** хеширования (400 МБ не читаются).
pub fn verify_against(path: &Path, expected_size: u64, expected_sha: &str) -> Result<()> {
    let meta = std::fs::metadata(path)
        .map_err(|e| DiarizeError::Io(format!("{}: {e}", path.display())))?;
    if meta.len() != expected_size {
        return Err(DiarizeError::Format(format!(
            "размер модели {}: ожидалось {expected_size} байт, получено {} \
             (скачайте заново: installers\\fetch_nemotron3_model.ps1)",
            path.display(),
            meta.len()
        )));
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = (path.to_path_buf(), meta.len(), mtime);
    let cache = VERIFIED.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cached) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return cached.map_err(DiarizeError::Format);
    }
    // Файл мог исчезнуть между metadata и чтением — фиксируем ошибку строкой,
    // чтобы она не кэшировала `Io` (докачка должна «оживить» проверку).
    let result = match sha256_file(path) {
        Ok(got) if got == expected_sha => Ok(()),
        Ok(got) => Err(format!(
            "sha256 модели {} не совпал: ожидался {expected_sha}, получен {got}",
            path.display()
        )),
        Err(e) => return Err(e),
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(key, result.clone());
    }
    result.map_err(DiarizeError::Format)
}

/// Диаризатор: модель загружается лениво, при первом прогоне выбирается EP.
pub struct Diarizer {
    cfg: NemotronConfig,
    env: Environment,
    active: Option<(Device, Sortformer)>,
    /// Устройство, на котором реально шёл последний прогон.
    last_device: Device,
}

impl Diarizer {
    /// Загрузить (проверить модель) в общее ONNX-окружение процесса.
    pub fn load(cfg: &NemotronConfig) -> Result<Self> {
        Self::load_with_env(cfg, ort_env()?.clone())
    }

    /// Загрузить с **заданным** окружением (когда ORT уже инициализирован).
    pub fn load_with_env(cfg: &NemotronConfig, env: Environment) -> Result<Self> {
        cfg.validate()?;
        verify_model(&cfg.model)?;
        Ok(Diarizer {
            cfg: cfg.clone(),
            env,
            active: None,
            last_device: cfg.device,
        })
    }

    /// Конфиг (для логов/`stats`).
    pub fn config(&self) -> &NemotronConfig {
        &self.cfg
    }

    /// Устройство последнего прогона (`auto` уже разрешён в конкретное).
    pub fn device(&self) -> Device {
        self.last_device
    }

    /// Выбор EP: явный конфиг сильнее авто; `auto` — GPU только для длинных файлов.
    fn choose_device(&self, duration_sec: f64) -> Device {
        if self.cfg.device != Device::Auto {
            return self.cfg.device;
        }
        // `min_audio_min` — в минутах, длительность — в секундах (не путать: на этом
        // живой прогон W3 отправил 60-с клип на DirectML, где он медленнее CPU).
        if duration_sec < self.cfg.min_audio_min * 60.0 {
            return Device::Cpu;
        }
        #[cfg(windows)]
        {
            Device::DirectMl
        }
        #[cfg(target_os = "macos")]
        {
            Device::CoreMl
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            Device::Cpu
        }
    }

    /// Проверить, что EP допустим на платформе (понятная ошибка вместо падения ORT).
    fn check_platform(dev: Device) -> Result<()> {
        match dev {
            Device::DirectMl if !cfg!(windows) => Err(DiarizeError::Format(
                "directml доступен только на Windows".to_string(),
            )),
            Device::CoreMl if !cfg!(target_os = "macos") => Err(DiarizeError::Format(
                "coreml доступен только на macOS".to_string(),
            )),
            _ => Ok(()),
        }
    }

    /// Создать/переиспользовать `Sortformer` под выбранный EP.
    fn ensure(&mut self, dev: Device) -> Result<&mut Sortformer> {
        Self::check_platform(dev)?;
        if !matches!(&self.active, Some((d, _)) if *d == dev) {
            let sf = self.create(dev)?;
            self.active = Some((dev, sf));
            self.last_device = dev;
        }
        Ok(&mut self.active.as_mut().expect("только что создан").1)
    }

    fn create(&mut self, dev: Device) -> Result<Sortformer> {
        let ep = match dev {
            Device::Cpu | Device::Auto => ExecutionProvider::Cpu,
            #[cfg(windows)]
            Device::DirectMl => ExecutionProvider::DirectML,
            #[cfg(not(windows))]
            Device::DirectMl => ExecutionProvider::Cpu,
            #[cfg(target_os = "macos")]
            Device::CoreMl => ExecutionProvider::CoreML,
            #[cfg(not(target_os = "macos"))]
            Device::CoreMl => ExecutionProvider::Cpu,
        };
        let exec = ExecutionConfig::new()
            .with_environment(self.env.clone())
            .with_execution_provider(ep);
        let diar = DiarizationConfig::custom(self.cfg.onset, self.cfg.offset);
        match Sortformer::with_config(&self.cfg.model, Some(exec), diar) {
            Ok(sf) => Ok(sf),
            // auto: GPU-сессия не поднялась (нет провайдера/DirectML.dll) — CPU.
            Err(e) if self.cfg.device == Device::Auto && dev != Device::Cpu => {
                eprintln!("[diar] {dev:?} недоступен ({e}); перехожу на CPU");
                let exec = ExecutionConfig::new()
                    .with_environment(self.env.clone())
                    .with_execution_provider(ExecutionProvider::Cpu);
                let diar = DiarizationConfig::custom(self.cfg.onset, self.cfg.offset);
                Sortformer::with_config(&self.cfg.model, Some(exec), diar)
                    .map_err(|e| DiarizeError::Tool(format!("Sortformer (CPU): {e}")))
            }
            Err(e) => Err(DiarizeError::Tool(format!("Sortformer ({dev:?}): {e}"))),
        }
    }

    /// Диаризация WAV-файла (16 кГц mono) → turns в абсолютной шкале.
    pub fn run_wav(&mut self, wav: &Path) -> Result<Vec<Turn>> {
        let w = crate::read_wav(wav)?;
        if w.sample_rate != SAMPLE_RATE || w.channels != 1 {
            return Err(DiarizeError::Format(format!(
                "{}: ожидался WAV {SAMPLE_RATE} Гц mono, получено {} Гц / {} кан.",
                wav.display(),
                w.sample_rate,
                w.channels
            )));
        }
        self.run_i16(&w.samples, w.sample_rate)
    }

    /// Диаризация `f32`-сэмплов (вход Sortformer; 16 кГц mono).
    pub fn run_samples(&mut self, samples: &[f32], sample_rate: u32) -> Result<Vec<Turn>> {
        let pcm: Vec<i16> = samples
            .iter()
            .map(|v| (v.clamp(-1.0, 1.0) * 32_767.0) as i16)
            .collect();
        self.run_i16(&pcm, sample_rate)
    }

    /// Диаризация PCM: короткие файлы — `diarize()` целиком, длинные — блоками
    /// `chunk_min` через `feed()`/`flush()` (память не растёт с длиной).
    pub fn run_i16(&mut self, samples: &[i16], sample_rate: u32) -> Result<Vec<Turn>> {
        if sample_rate != SAMPLE_RATE {
            return Err(DiarizeError::Format(format!(
                "диаризация работает на {SAMPLE_RATE} Гц, получено {sample_rate}"
            )));
        }
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let duration = samples.len() as f64 / sample_rate as f64;
        let device = self.choose_device(duration);
        let chunk = ((self.cfg.chunk_min * 60.0 * sample_rate as f64) as usize).max(1);
        let whole = duration <= self.cfg.min_audio_min * 60.0;

        let sf = self.ensure(device)?;
        sf.reset_state();
        let segments = if whole {
            sf.diarize(to_f32(samples), sample_rate, 1)
                .map_err(|e| DiarizeError::Tool(format!("diarize: {e}")))?
        } else {
            let mut out: Vec<SpeakerSegment> = Vec::new();
            for block in samples.chunks(chunk) {
                out.extend(
                    sf.feed(&to_f32(block))
                        .map_err(|e| DiarizeError::Tool(format!("feed: {e}")))?,
                );
            }
            out.extend(
                sf.flush()
                    .map_err(|e| DiarizeError::Tool(format!("flush: {e}")))?,
            );
            out
        };
        Ok(segments_to_turns(&segments, sample_rate))
    }
}

/// `i16` → `f32` в диапазоне −1…1 (вход модели).
fn to_f32(samples: &[i16]) -> Vec<f32> {
    samples.iter().map(|s| *s as f32 / 32768.0).collect()
}

/// Сегменты модели → turns: сэмплы в секунды, сортировка, склейка дублей из
/// перекрывающихся окон `feed()` (одинаковый спикер и стык без разрыва).
pub fn segments_to_turns(segments: &[SpeakerSegment], sample_rate: u32) -> Vec<Turn> {
    let sr = sample_rate.max(1) as f64;
    let mut turns: Vec<Turn> = segments
        .iter()
        .map(|s| Turn {
            start: s.start as f64 / sr,
            end: s.end as f64 / sr,
            speaker: s.speaker_id as u32,
        })
        .filter(|t| !t.is_empty())
        .collect();
    turns.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.speaker.cmp(&b.speaker))
    });
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    for t in turns {
        match out.last_mut() {
            Some(last) if last.speaker == t.speaker && t.start <= last.end + 1e-3 => {
                last.end = last.end.max(t.end);
            }
            _ => out.push(t),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start_s: f64, end_s: f64, speaker_id: usize) -> SpeakerSegment {
        SpeakerSegment {
            start: (start_s * SAMPLE_RATE as f64) as u64,
            end: (end_s * SAMPLE_RATE as f64) as u64,
            speaker_id,
        }
    }

    /// Дубли из перекрывающихся окон и соседние куски одного спикера склеиваются.
    #[test]
    fn segments_to_turns_merges_duplicates() {
        let segs = [
            seg(1.0, 2.0, 0),
            seg(1.0, 2.0, 0),
            seg(2.0, 3.0, 0),
            seg(3.5, 4.0, 1),
            seg(4.0, 4.5, 1),
            seg(4.5, 5.0, 0),
        ];
        let turns = segments_to_turns(&segs, SAMPLE_RATE);
        assert_eq!(
            turns,
            vec![
                Turn {
                    start: 1.0,
                    end: 3.0,
                    speaker: 0
                },
                Turn {
                    start: 3.5,
                    end: 4.5,
                    speaker: 1
                },
                Turn {
                    start: 4.5,
                    end: 5.0,
                    speaker: 0
                },
            ]
        );
    }

    /// Пустые и обратные интервалы отбрасываются.
    #[test]
    fn segments_to_turns_filters_empty() {
        let segs = [seg(2.0, 2.0, 0), seg(3.0, 2.5, 1), seg(5.0, 6.0, 2)];
        let turns = segments_to_turns(&segs, SAMPLE_RATE);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].speaker, 2);
    }

    /// `auto`: короткий клип — CPU (оверхед сессии GPU), длинный — GPU платформы.
    #[test]
    fn auto_device_threshold_is_in_minutes() {
        let cfg = NemotronConfig {
            model: PathBuf::from("m.onnx"),
            device: Device::Auto,
            ..NemotronConfig::default()
        };
        let d = Diarizer {
            cfg,
            env: match ort_env() {
                Ok(e) => e.clone(),
                Err(_) => return, // без ONNX Runtime проверять нечего
            },
            active: None,
            last_device: Device::Auto,
        };
        // 60 с < 2 мин ⇒ CPU (баг W3: сравнивались секунды с минутами).
        assert_eq!(d.choose_device(60.0), Device::Cpu);
        assert_eq!(d.choose_device(119.0), Device::Cpu);
        let long = d.choose_device(600.0);
        if cfg!(windows) {
            assert_eq!(long, Device::DirectMl);
        } else if cfg!(target_os = "macos") {
            assert_eq!(long, Device::CoreMl);
        } else {
            assert_eq!(long, Device::Cpu);
        }
    }

    #[test]
    fn device_parse_and_names() {
        assert_eq!(Device::parse("auto"), Some(Device::Auto));
        assert_eq!(Device::parse(""), Some(Device::Auto));
        assert_eq!(Device::parse(" CPU "), Some(Device::Cpu));
        assert_eq!(Device::parse("DirectML"), Some(Device::DirectMl));
        assert_eq!(Device::parse("coreml"), Some(Device::CoreMl));
        assert_eq!(Device::parse("cuda"), None);
        assert_eq!(Device::DirectMl.as_str(), "directml");
    }

    #[test]
    fn config_validation() {
        let mut cfg = NemotronConfig::new("");
        assert!(cfg.validate().is_err(), "пустой путь");
        cfg.model = std::env::temp_dir().join("hds-no-such-model.onnx");
        assert!(matches!(cfg.validate(), Err(DiarizeError::Io(_))));
        let dir = std::env::temp_dir().join(format!("hds-diar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("m.onnx");
        std::fs::write(&model, b"x").unwrap();
        let mut cfg = NemotronConfig::new(&model);
        cfg.chunk_min = 1.0;
        assert!(cfg.validate().is_err(), "chunk_min вне 2..=30");
        cfg.chunk_min = 10.0;
        // F12 (L3): onset/offset вне 0..=1 не доходят до модели.
        cfg.onset = 1.5;
        assert!(cfg.validate().is_err(), "onset вне 0..=1");
        cfg.onset = 0.5;
        cfg.offset = -1.0;
        assert!(cfg.validate().is_err(), "offset вне 0..=1");
        cfg.offset = 1.0;
        assert!(cfg.validate().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Размер проверяется до хеширования — понятная ошибка для недокачанной модели.
    #[test]
    fn verify_model_rejects_wrong_size() {
        let dir = std::env::temp_dir().join(format!("hds-diar-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join(NEMOTRON_FILE);
        std::fs::write(&model, b"not a model").unwrap();
        let err = verify_model(&model).unwrap_err();
        assert!(err.to_string().contains("размер модели"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F6 (ревью M3): отрицательный результат проверки кэшируется — битая модель
    /// не хешируется на каждом задании. Ключ — `(путь, размер, mtime)`.
    #[test]
    fn negative_result_is_cached() {
        let dir = std::env::temp_dir().join(format!("hds-diar-neg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("m.onnx");
        std::fs::write(&model, b"abc").unwrap();
        let mtime = std::fs::metadata(&model).unwrap().modified().unwrap();
        let wrong = "00".repeat(32);

        // Первая проверка — sha не совпал (размер совпадает с ожидаемым).
        let err = verify_against(&model, 3, &wrong).unwrap_err();
        assert!(err.to_string().contains("sha256"), "{err}");

        // Файл подменён (тот же размер, тот же mtime) ⇒ ключ тот же, берётся кэш:
        // для верного sha результат всё равно прежний отказ.
        std::fs::write(&model, b"xyz").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&model)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let good = sha256_file(&model).unwrap();
        let err = verify_against(&model, 3, &good).unwrap_err();
        assert!(
            err.to_string().contains("sha256"),
            "отрицательный результат не закэширован: {err}"
        );

        // Новый mtime ⇒ ключ другой ⇒ проверка заново: верный sha проходит.
        std::fs::File::options()
            .write(true)
            .open(&model)
            .unwrap()
            .set_modified(mtime + std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            verify_against(&model, 3, &good).is_ok(),
            "после перезаписи файла проверка должна пройти заново"
        );

        // Удаление файла — понятная ошибка ввода-вывода (и она не кэшируется).
        std::fs::remove_file(&model).unwrap();
        let err = verify_against(&model, 3, &good).unwrap_err();
        assert!(matches!(err, DiarizeError::Io(_)), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// W8: EP чужой платформы отвергается понятной ошибкой (coreml вне macOS,
    /// directml вне Windows). Живую машину macOS в W8 нет, но гард проверяется на
    /// любой платформе: `device: auto` до него не доходит, явный EP — получает текст.
    #[test]
    fn foreign_execution_provider_is_rejected() {
        #[cfg(not(target_os = "macos"))]
        {
            let err = Diarizer::check_platform(Device::CoreMl).unwrap_err();
            assert!(err.to_string().contains("macOS"), "{err}");
        }
        #[cfg(not(windows))]
        {
            let err = Diarizer::check_platform(Device::DirectMl).unwrap_err();
            assert!(err.to_string().contains("Windows"), "{err}");
        }
        // CPU и `auto` проходят гард на любой платформе.
        assert!(Diarizer::check_platform(Device::Cpu).is_ok());
        assert!(Diarizer::check_platform(Device::Auto).is_ok());
    }

    /// sha256 считается верно (контрольное значение для «abc»).
    #[test]
    fn sha256_of_known_bytes() {
        let dir = std::env::temp_dir().join(format!("hds-diar-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("abc.bin");
        std::fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
