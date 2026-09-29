//! Спайки W0: общая библиотека (content_hash на Rust для спайка 2).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use blake2::digest::consts::U16;
use blake2::digest::{Digest, FixedOutput, KeyInit};
use blake2::Blake2b;

/// Точная копия hds/indexer.py:content_hash.
/// blake2b-16 от: str(size) + первые 256 КБ + (если size > 256 КБ) последние 256 КБ.
pub fn content_hash(path: &Path, size: u64) -> std::io::Result<String> {
    let mut h = Blake2b::<U16>::new();
    h.update(size.to_string().as_bytes());
    let head = 256usize * 1024;
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; head];
    let n1 = read_full(&mut f, &mut buf)?;
    h.update(&buf[..n1]);
    if size > head as u64 {
        f.seek(SeekFrom::End(-(head as i64)))?;
        let n2 = read_full(&mut f, &mut buf)?;
        h.update(&buf[..n2]);
    }
    Ok(hex::encode(h.finalize_fixed()))
}

fn read_full(f: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match f.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}