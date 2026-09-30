//! `content_hash` — дословный порт `hds/indexer.py:content_hash`
//! (MIGRATION_PLAN_RUST.md §3.1 п.2: паритет обязателен, иначе полная
//! переиндексация всей БД).
//!
//! Отпечаток = `blake2b(digest_size=16)` от
//! `str(size)` + первые 256 КБ + (если `size > 256 КБ`) последние 256 КБ.
//! Важно: `Blake2b<U16>` — это BLAKE2b **с параметром длины 16**, а не усечение
//! BLAKE2b-512 (паритет 50/50 подтверждён в спайке 2, `tools/parity/SPIKES.md` §4).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use blake2::digest::consts::U16;
use blake2::digest::{Digest, FixedOutput};
use blake2::Blake2b;

/// Окно файла (голова и хвост), участвующее в отпечатке.
pub const HASH_WINDOW: usize = 256 * 1024;

/// Быстрый отпечаток файла. `size` — размер, снятый вызывающим (`os.stat`):
/// Python передаёт его аргументом и включает в хэш как десятичную строку.
///
/// Ошибки ввода-вывода возвращаются как `Err` (в Python `content_hash` отдаёт
/// `None`, вызывающий трактует его как «файл пропустить»).
pub fn content_hash(path: &Path, size: u64) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut head_buf = vec![0u8; HASH_WINDOW];
    let n1 = read_full(&mut f, &mut head_buf)?;
    // Хвост читаем в ОТДЕЛЬНЫЙ буфер: переиспользование перезаписало бы голову
    // (эту ошибку поймал тест на векторе `len = 256 КБ + 1`).
    let mut tail_buf;
    let tail: Option<&[u8]> = if size > HASH_WINDOW as u64 {
        tail_buf = vec![0u8; HASH_WINDOW];
        f.seek(SeekFrom::End(-(HASH_WINDOW as i64)))?;
        let n2 = read_full(&mut f, &mut tail_buf)?;
        Some(&tail_buf[..n2])
    } else {
        None
    };
    Ok(hash_of_parts(size, &head_buf[..n1], tail))
}

/// Хэш по уже прочитанным частям — используется, когда файл открыт/прочитан
/// вызывающим (конвейер B4) и в тестах паритета на фиксированных векторах.
pub fn hash_of_parts(size: u64, head: &[u8], tail: Option<&[u8]>) -> String {
    let mut h = Blake2b::<U16>::new();
    h.update(size.to_string().as_bytes());
    h.update(head);
    if let Some(t) = tail {
        h.update(t);
    }
    hex::encode(h.finalize_fixed())
}

/// Дочитать буфер до конца (короткие чтения и «растущий» файл — как `f.read(n)`
/// в Python, который также может вернуть меньше запрошенного).
fn read_full(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match f.read(&mut buf[total..])? {
            0 => break,
            n => total += n,
        }
    }
    Ok(total)
}
