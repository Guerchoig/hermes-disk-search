//! Спайк 2 (W0 §4 п.4): паритет content_hash Rust vs Python.
//!
//! Читает tools/parity/out/hash_parity.jsonl (эталон от hds/indexer.py:content_hash),
//! пересчитывает каждый файл в Rust и сравнивает hex.
//! Критерий спайка: 50/50 совпадений.
//!
//! Запуск:  cargo run --release --bin hash_parity

use std::io::{BufRead, BufReader};
use std::path::Path;

use hds_spikes::content_hash;

fn main() {
    let default = concat!(env!("CARGO_MANIFEST_DIR"), r"\..\out\hash_parity.jsonl");
    let jsonl = std::env::args().nth(1).unwrap_or_else(|| default.to_string());
    let file = std::fs::File::open(&jsonl).unwrap_or_else(|e| {
        panic!("не удалось открыть {}: {} (сначала запустите tools/parity/spike2_hash.py)", jsonl, e)
    });
    let mut ok = 0usize;
    let mut fail = 0usize;
    for line in BufReader::new(file).lines() {
        let line = line.expect("read");
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let path = v["path"].as_str().unwrap_or_default().to_string();
        let size = v["size"].as_u64().unwrap_or(0);
        let expected = v["expected"].as_str().unwrap_or_default().to_string();
        match content_hash(Path::new(&path), size) {
            Ok(h) if h == expected => ok += 1,
            Ok(h) => {
                fail += 1;
                println!("MISMATCH {}:\n  python={}\n  rust  ={}", path, expected, h);
            }
            Err(e) => {
                fail += 1;
                println!("IO ERROR {}: {}", path, e);
            }
        }
    }
    println!("{}/{} совпадений content_hash (Rust Blake2b-16 == Python blake2b-16)", ok, ok + fail);
    if fail == 0 {
        println!("HASH_PARITY_OK");
    } else {
        println!("HASH_PARITY_FAIL: Rust-хэш расходится с Python -> потребуется полная переиндексация БД");
        std::process::exit(1);
    }
}