//! Vision-энкодер CLIP на ONNX Runtime: картинка → нормализованный вектор (dim 512).

use std::path::Path;
use std::sync::Mutex;

use hds_core::error::{CoreError, Result};
use ort::session::Session;
use ort::value::Tensor;

use crate::preprocess::preprocess_image;
use crate::{l2_normalize, IMAGE_SIZE};

/// ONNX-сессия vision-энкодера (`clip_vision.onnx`, вход `pixel_values`).
pub struct VisionEncoder {
    session: Mutex<Session>,
}

impl VisionEncoder {
    /// Загрузить модель (`clip_vision.onnx`).
    pub fn load(model: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| CoreError::Other(format!("vision builder: {e}")))?
            .commit_from_file(model)
            .map_err(|e| CoreError::Other(format!("vision ONNX {}: {e}", model.display())))?;
        Ok(VisionEncoder {
            session: Mutex::new(session),
        })
    }

    /// Картинка → нормализованный вектор (dim 512).
    pub fn embed(&self, path: &Path) -> Result<Vec<f32>> {
        let input = preprocess_image(path)?;
        self.embed_input(input)
    }

    /// Вектор из уже подготовленного входа `[1,3,224,224]` (для тестов/батча).
    pub fn embed_input(&self, input: Vec<f32>) -> Result<Vec<f32>> {
        let s = IMAGE_SIZE as i64;
        let tensor = Tensor::from_array(([1i64, 3, s, s], input))
            .map_err(|e| CoreError::Other(format!("vision tensor: {e}")))?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Other("vision session mutex отравлен".into()))?;
        let outputs = session
            .run(ort::inputs!["pixel_values" => tensor])
            .map_err(|e| CoreError::Other(format!("vision run: {e}")))?;
        let (_shape, data) = outputs["image_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(|e| CoreError::Other(format!("vision output: {e}")))?;
        let mut v = data.to_vec();
        l2_normalize(&mut v);
        Ok(v)
    }
}
