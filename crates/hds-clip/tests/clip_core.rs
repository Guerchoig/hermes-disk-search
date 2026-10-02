//! Быстрые тесты CLIP без ONNX-моделей: препроцессинг, разбор конфига, деградация.

use std::path::PathBuf;

use hds_clip::{cosine, preprocess_image, Clip, ClipConfig, CLIP_DIM, IMAGE_SIZE};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tools")
        .join("parity")
        .join("fixtures")
}

fn cfg(yaml: &str) -> hds_core::config::Config {
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn preprocess_shape_and_range() {
    let v = preprocess_image(&fixtures().join("цветы_без_exif.jpg")).unwrap();
    assert_eq!(v.len(), 3 * IMAGE_SIZE * IMAGE_SIZE, "CHW [1,3,224,224]");
    // нормализованные значения: не NaN и в разумных пределах (не «сырые» 0..255)
    assert!(v.iter().all(|x| x.is_finite()));
    assert!(
        v.iter().any(|x| x.abs() > 1.0),
        "значения должны быть нормализованы"
    );
}

#[test]
fn config_disabled_by_flag() {
    let c = ClipConfig::from_config(&cfg("index:\n  clip: false\n"));
    assert!(!c.enabled);
    assert!(Clip::load(&cfg("index:\n  clip: false\n")).is_none());
}

#[test]
fn config_default_paths() {
    let c = ClipConfig::from_config(&cfg("index:\n  clip: true\n"));
    assert!(c.enabled);
    assert!(c
        .vision_model
        .to_string_lossy()
        .ends_with("clip_vision.onnx"));
    assert!(c
        .text_model
        .to_string_lossy()
        .ends_with("clip_text_dense.onnx"));
    assert!(c.tokenizer.to_string_lossy().ends_with("tokenizer.json"));
}

#[test]
fn config_explicit_paths() {
    let c = ClipConfig::from_config(&cfg(
        "index:\n  clip_vision_model: C:\\m\\v.onnx\n  clip_text_model: C:\\m\\t.onnx\n  clip_tokenizer: C:\\m\\tok.json\n",
    ));
    assert_eq!(c.vision_model, PathBuf::from("C:\\m\\v.onnx"));
    assert_eq!(c.text_model, PathBuf::from("C:\\m\\t.onnx"));
    assert_eq!(c.tokenizer, PathBuf::from("C:\\m\\tok.json"));
}

#[test]
fn missing_models_degrade() {
    // пути указывают в пустоту → модели не найдены → Clip::load = None (не паника)
    let yaml = "index:\n  clip_vision_model: C:\\nope\\v.onnx\n  clip_text_model: C:\\nope\\t.onnx\n  clip_tokenizer: C:\\nope\\tok.json\n";
    let c = ClipConfig::from_config(&cfg(yaml));
    assert!(!c.models_present());
    assert!(Clip::load(&cfg(yaml)).is_none());
}

#[test]
fn cosine_basic() {
    let a = vec![1.0f32, 0.0, 0.0];
    assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
    let b = vec![-1.0f32, 0.0, 0.0];
    assert!((cosine(&a, &b) + 1.0).abs() < 1e-6);
    assert_eq!(CLIP_DIM, 512);
}
