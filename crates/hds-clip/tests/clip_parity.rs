//! W3/§7: паритет CLIP Rust ↔ sentence-transformers (golden `out/clip_parity.json`).
//!
//! Требует ONNX-модели спайка (`tools/parity/out/clip_onnx/`) — тест `#[ignore]`.
//! Запуск: `cargo test -p hds-clip --test clip_parity -- --ignored --nocapture`.
//!
//! Критерий §7/спайка 4: cosine ≥ 0,999 (vision и text).

use std::path::PathBuf;

use hds_clip::{cosine, TextEncoder, VisionEncoder};
use serde_json::Value;

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tools")
        .join("parity")
        .join("out")
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tools")
        .join("parity")
        .join("fixtures")
}

fn vec_of(v: &Value) -> Vec<f32> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[ignore = "требует ONNX-модели спайка (out/clip_onnx)"]
fn vision_and_text_parity() {
    let golden_path = out_dir().join("clip_parity.json");
    let golden: Value = serde_json::from_str(&std::fs::read_to_string(&golden_path).unwrap())
        .expect("clip_parity.json");
    let onnx = out_dir().join("clip_onnx");

    let vis = VisionEncoder::load(&onnx.join("vision").join("clip_vision.onnx")).unwrap();
    let txt = TextEncoder::load(
        &onnx.join("text").join("clip_text_dense.onnx"),
        &onnx.join("text").join("tokenizer.json"),
    )
    .unwrap();

    let mut cos_min = 1.0f32;
    for (name, want) in golden["images"].as_object().unwrap() {
        let got = vis.embed(&fixtures().join(name)).unwrap();
        let c = cosine(&got, &vec_of(want));
        println!("vision {name}: dim={} cos={c:.6}", got.len());
        assert_eq!(got.len(), 512);
        cos_min = cos_min.min(c);
    }
    for (q, want) in golden["queries"].as_object().unwrap() {
        let got = txt.embed(q).unwrap();
        let c = cosine(&got, &vec_of(want));
        println!("text   «{q}»: dim={} cos={c:.6}", got.len());
        assert_eq!(got.len(), 512);
        cos_min = cos_min.min(c);
    }
    println!("CLIP: cos_min={cos_min:.6} (порог 0,999)");
    assert!(
        cos_min >= 0.999,
        "паритет CLIP не сошёлся: cos_min={cos_min}"
    );
}
