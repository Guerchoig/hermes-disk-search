//! `hds-clip` — CLIP на ONNX Runtime (`MIGRATION_PLAN_RUST.md` §7).
//!
//! Два энкодера:
//! * **vision** — при индексации картинки (`pipeline::clip_store`), выход `images_vec` (dim 512);
//! * **text** — для поиска/`clip-index` (мультиязычный, понимает русский), резидентный.
//!
//! Модели — ONNX-экспорт спайка 4/`clip_onnx_w3.py`
//! (`tools/parity/out/clip_onnx/{vision/clip_vision.onnx,text/clip_text_dense.onnx}`),
//! препроцессинг — строго как `CLIPImageProcessor` (`openai/clip-vit-base-patch32`).
//! Деградация без моделей: [`Clip::load`] вернёт `None`, индексация/поиск работают без CLIP.

#![forbid(unsafe_code)]

pub mod config;
pub mod preprocess;
pub mod text;
pub mod vision;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use config::ClipConfig;

pub use preprocess::preprocess_image;
pub use text::TextEncoder;
pub use vision::VisionEncoder;

/// Размерность CLIP-вектора (`images_vec`, `dim=512`).
pub const CLIP_DIM: usize = 512;
/// Сторона входа vision-энкодера (`CLIPImageProcessor` crop_size).
pub const IMAGE_SIZE: usize = 224;

/// Оба энкодера (загружаются по требованию).
pub struct Clip {
    vision: VisionEncoder,
    text: TextEncoder,
    dim: usize,
}

impl Clip {
    /// Загрузить оба энкодера из конфига; `None` — модели недоступны (деградация).
    pub fn load(cfg: &hds_core::config::Config) -> Option<Clip> {
        let c = ClipConfig::from_config(cfg);
        if !c.enabled || !c.models_present() {
            return None;
        }
        let vision = VisionEncoder::load(&c.vision_model).ok()?;
        let text = TextEncoder::load(&c.text_model, &c.tokenizer).ok()?;
        Some(Clip {
            vision,
            text,
            dim: CLIP_DIM,
        })
    }

    /// Размерность вектора.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Вектор картинки (нормализованный, dim 512).
    pub fn embed_image(&self, path: &Path) -> hds_core::error::Result<Vec<f32>> {
        self.vision.embed(path)
    }

    /// Вектор текстового запроса (нормализованный, dim 512).
    pub fn embed_text(&self, query: &str) -> hds_core::error::Result<Vec<f32>> {
        self.text.embed(query)
    }
}

/// L2-нормализация вектора на месте (`normalize_embeddings=True` в Python).
pub(crate) fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Косинус двух векторов (для тестов/диагностики).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for i in 0..a.len().min(b.len()) {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    dot / (na.sqrt() * nb.sqrt() + 1e-12)
}

/// Процессно-общий CLIP (ленивая загрузка при первом обращении; `None` — деградация).
///
/// Модели фиксированы на прогон, поэтому конфиг первого вызова и определяет
/// инстанс — повторные вызовы переиспользуют его (как `clip_index._img_model`).
static SHARED: OnceLock<Option<Clip>> = OnceLock::new();

/// Общий CLIP для индексации/поиска.
pub fn shared(cfg: &hds_core::config::Config) -> Option<&'static Clip> {
    SHARED.get_or_init(|| Clip::load(cfg)).as_ref()
}

/// Базовый каталог ONNX-моделей CLIP.
///
/// Поставка: `models\clip_onnx` (кладёт `installers\fetch_clip_models.ps1`) —
/// рядом с установкой, в git не хранится. Dev-фолбэк: `tools/parity/out/clip_onnx`
/// (каталог экспорта W3, `tools/parity/clip_onnx_w3.py`).
pub fn default_onnx_dir() -> PathBuf {
    let root = hds_core::config::project_root();
    let installed = root.join("models").join("clip_onnx");
    let dev = root
        .join("tools")
        .join("parity")
        .join("out")
        .join("clip_onnx");
    let installed_has = installed.join("vision").join("clip_vision.onnx").is_file()
        || installed
            .join("text")
            .join("clip_text_dense.onnx")
            .is_file()
        || installed.join("text").join("tokenizer.json").is_file();
    if installed_has {
        installed
    } else if dev.is_dir() {
        dev
    } else {
        installed
    }
}
