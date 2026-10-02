//! Препроцессинг картинки — строго как `CLIPImageProcessor`
//! (`openai/clip-vit-base-patch32`): resize по короткой стороне до 224 (bicubic),
//! center-crop 224×224, `rescale=1/255`, нормализация CLIP mean/std, CHW.
//!
//! Точность важна: паритет с sentence-transformers — cos ≥ 0,999 (`SPIKES.md` §6).

use std::path::Path;

use hds_core::error::{CoreError, Result};
use image::imageops::FilterType;
use image::RgbImage;

use crate::IMAGE_SIZE;

/// CLIP mean (RGB) из `preprocessor_config.json`.
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
/// CLIP std (RGB).
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

/// Прочитать картинку и подготовить вход vision-энкодера `[1,3,224,224]` (CHW).
pub fn preprocess_image(path: &Path) -> Result<Vec<f32>> {
    let img = image::open(path)
        .map_err(|e| CoreError::Other(format!("{}: {e}", path.display())))?
        .to_rgb8();
    Ok(to_input(&img))
}

/// `[1,3,224,224]` из RGB-картинки (resize+center-crop+normalize).
pub fn to_input(img: &RgbImage) -> Vec<f32> {
    let (w, h) = (img.width(), img.height());
    let size = IMAGE_SIZE as u32;
    let (nw, nh) = resize_shortest(w, h, size);
    let resized = image::imageops::resize(img, nw, nh, FilterType::CatmullRom);
    let (x0, y0) = ((nw - size) / 2, (nh - size) / 2);
    let cropped = image::imageops::crop_imm(&resized, x0, y0, size, size).to_image();

    let s = IMAGE_SIZE;
    let plane = s * s;
    let mut out = vec![0f32; 3 * plane];
    for y in 0..s {
        for x in 0..s {
            let px = cropped.get_pixel(x as u32, y as u32);
            for c in 0..3 {
                let v = (px[c] as f32 / 255.0 - MEAN[c]) / STD[c];
                out[c * plane + (y * s + x)] = v;
            }
        }
    }
    out
}

/// Целевой размер resize по короткой стороне (`shortest_edge = size`), как в HF
/// (`int(size * long/short)`, короткая сторона ровно `size`).
fn resize_shortest(w: u32, h: u32, size: u32) -> (u32, u32) {
    if w <= h {
        let nh = (size as f64 * h as f64 / w as f64) as u32;
        (size, nh.max(size))
    } else {
        let nw = (size as f64 * w as f64 / h as f64) as u32;
        (nw.max(size), size)
    }
}
