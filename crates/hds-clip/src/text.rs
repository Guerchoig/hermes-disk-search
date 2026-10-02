//! Text-энкодер CLIP (мультиязычный) на ONNX Runtime: запрос → нормализованный
//! вектор (dim 512). Токенайзер — `tokenizer.json` (`tokenizers`), pooling+Dense
//! уже внутри ONNX-графа (выход `text_embeds`).

use std::path::Path;
use std::sync::Mutex;

use hds_core::error::{CoreError, Result};
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::{Tokenizer, TruncationParams};

use crate::l2_normalize;

/// Максимальная длина запроса (`sentence_bert_config.max_seq_length`).
const MAX_LEN: usize = 128;

/// ONNX-сессия + токенайзер мультиязычного text-энкодера.
pub struct TextEncoder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

impl TextEncoder {
    /// Загрузить модель (`clip_text_dense.onnx`) и токенайзер (`tokenizer.json`).
    pub fn load(model: &Path, tokenizer_path: &Path) -> Result<Self> {
        let session = Session::builder()
            .map_err(|e| CoreError::Other(format!("text builder: {e}")))?
            .commit_from_file(model)
            .map_err(|e| CoreError::Other(format!("text ONNX {}: {e}", model.display())))?;
        let mut tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| {
            CoreError::Other(format!("tokenizer {}: {e}", tokenizer_path.display()))
        })?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_LEN,
                ..Default::default()
            }))
            .map_err(|e| CoreError::Other(format!("tokenizer truncation: {e}")))?;
        Ok(TextEncoder {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// Запрос → нормализованный вектор (dim 512).
    pub fn embed(&self, query: &str) -> Result<Vec<f32>> {
        let enc = self
            .tokenizer
            .encode(query, true)
            .map_err(|e| CoreError::Other(format!("tokenize: {e}")))?;
        let ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&i| i as i64).collect();
        let seq = ids.len() as i64;
        let ids_t = Tensor::from_array(([1i64, seq], ids))
            .map_err(|e| CoreError::Other(format!("ids tensor: {e}")))?;
        let mask_t = Tensor::from_array(([1i64, seq], mask))
            .map_err(|e| CoreError::Other(format!("mask tensor: {e}")))?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| CoreError::Other("text session mutex отравлен".into()))?;
        let outputs = session
            .run(ort::inputs!["input_ids" => ids_t, "attention_mask" => mask_t])
            .map_err(|e| CoreError::Other(format!("text run: {e}")))?;
        let (_shape, data) = outputs["text_embeds"]
            .try_extract_tensor::<f32>()
            .map_err(|e| CoreError::Other(format!("text output: {e}")))?;
        let mut v = data.to_vec();
        l2_normalize(&mut v);
        Ok(v)
    }
}
