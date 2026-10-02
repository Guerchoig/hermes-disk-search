//! `hds clip-index` — порт `hds/cli.py::cmd_clip_index`: дозаполнение CLIP-векторов
//! проиндексированных картинок (ONNX Runtime, §7 плана).
//!
//! Vector создаётся и при обычной индексации (`pipeline::clip_store`); команда —
//! для уже проиндексированных картинок без вектора. Модели грузятся лениво
//! ([`hds_clip::shared`]); недоступны — возвращаем 1 (как Python при недоступном CLIP).

use std::path::Path;
use std::time::Instant;

use hds_core::{config, db};
use hds_index::vector_blob;

use crate::support::open_conn;

/// `cmd_clip_index`: 0 — успех (в т.ч. когда дозаполнять нечего); 1 — CLIP недоступен.
pub fn cmd_clip_index() -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("конфиг: {}", e.message());
            return 1;
        }
    };
    let conn = match open_conn(&cfg) {
        Ok(c) => c,
        Err(e) => {
            println!("База данных недоступна: {}", e.message());
            return 1;
        }
    };
    let Some(clip) = hds_clip::shared(&cfg) else {
        println!(
            "[--] CLIP недоступен: ONNX-модели не найдены \
             (index.clip_vision_model / index.clip_text_model) или index.clip: false"
        );
        return 1;
    };
    let rows: Vec<(i64, String)> = db::images_without_vector(&conn)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, p)| Path::new(p).exists())
        .collect();
    let total = rows.len();
    println!("[clip] картинок без векторов: {}", total);
    let t0 = Instant::now();
    let mut done = 0usize;
    for (i, (fid, path)) in rows.iter().enumerate() {
        match clip.embed_image(Path::new(path)) {
            Ok(v) => {
                if db::add_image_vector(&conn, *fid, &vector_blob(&v)).is_ok() {
                    done += 1;
                }
            }
            Err(e) => eprintln!("[clip] {}: {}", path, e.message()),
        }
        if i % 32 == 0 || i + 1 == total {
            println!("  {}/{}", i + 1, total);
        }
    }
    println!(
        "[clip] готово: {} картинок за {:.1} с",
        done,
        t0.elapsed().as_secs_f64()
    );
    0
}
