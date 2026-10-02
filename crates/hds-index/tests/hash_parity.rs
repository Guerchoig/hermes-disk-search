//! Паритет `content_hash` с Python-версией (B2, `PLAN_W2_LLM_HOST.md` §5).
//!
//! Два уровня проверки:
//! 1. **фиксированные векторы** — хэши эталонных байтовых последовательностей,
//!    снятые Python-реализацией (`tools/parity/hash_vectors.py`); константы
//!    вписаны в тест, поэтому он работает и без `out/hash_vectors.json`;
//! 2. **50 реальных файлов** — `tools/parity/out/hash_parity.jsonl` от спайка 2
//!    (`#[ignore]`, нужен прогон `spike2_hash.py` на этой машине).

use std::io::Write;
use std::path::{Path, PathBuf};

use hds_index::hash::{content_hash, hash_of_parts, HASH_WINDOW};

/// Байты вектора: `content[i] = i % 251` (см. `hash_vectors.py`).
fn vector_bytes(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

/// Ожидаемые значения сняты Python-версией (`hashlib.blake2b(digest_size=16)`)
/// 30.09.2026 — включая все границы окна 256 КБ.
const FIXED: &[(usize, &str)] = &[
    (0, "1240a4684403b160e6597a653a88f56c"),
    (1, "1e8819f8482394ff1dcb603b1d9c1142"),
    (1024, "8426f71bb35a522de902d3b86aedba59"),
    (HASH_WINDOW - 1, "71c59257dafd1e24caf5a537dd245b7a"),
    (HASH_WINDOW, "709f695e536f25cd2947886fff0aff77"),
    (HASH_WINDOW + 1, "4f9cd13544d03380e34eda412d3ca91c"),
    (HASH_WINDOW * 2 - 1, "700b6f898c19c0ce123ab4f43c36dfe7"),
    (HASH_WINDOW * 2, "7f217c5faf7d6c039b956c4f49563af0"),
    (HASH_WINDOW * 3 + 7, "84f67f6ccd512774b78f084ae9adf516"),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Временный файл (папка — в системном temp, уборка в `Drop`).
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn new(name: &str, bytes: &[u8]) -> TempFile {
        let dir = std::env::temp_dir().join("hds-index-tests");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{}-{}", std::process::id(), name));
        let mut f = std::fs::File::create(&path).expect("создание временного файла");
        f.write_all(bytes).expect("запись временного файла");
        TempFile { path }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Главное: `content_hash` (через файл) и `hash_of_parts` совпадают с Python.
#[test]
fn content_hash_matches_python_fixed_vectors() {
    for (len, expected) in FIXED {
        let bytes = vector_bytes(*len);
        let f = TempFile::new(&format!("v{len}.bin"), &bytes);
        let got = content_hash(&f.path, *len as u64).expect("content_hash");
        assert_eq!(
            &got, expected,
            "расхождение на векторе len={len}: Rust {got} против Python {expected}"
        );
        // Проверка «нарезки» на части (путь конвейера B4: голова+хвост уже в памяти)
        let head_len = bytes.len().min(HASH_WINDOW);
        let head = &bytes[..head_len];
        let tail = if bytes.len() > HASH_WINDOW {
            Some(&bytes[bytes.len() - HASH_WINDOW..])
        } else {
            None
        };
        assert_eq!(
            hash_of_parts(*len as u64, head, tail),
            *expected,
            "hash_of_parts расходится на векторе len={len}"
        );
    }
}

/// Размер, переданный вызывающим, входит в хэш как десятичная строка —
/// поэтому «другой size» (гонка записи) даёт другой отпечаток, как в Python.
#[test]
fn size_is_part_of_the_hash() {
    let bytes = vector_bytes(2048);
    let f = TempFile::new("size.bin", &bytes);
    assert_ne!(
        content_hash(&f.path, bytes.len() as u64).unwrap(),
        content_hash(&f.path, bytes.len() as u64 + 1).unwrap()
    );
}

/// Файл короче окна читается целиком (хвост не добавляется).
#[test]
fn short_file_has_no_tail() {
    let bytes = vector_bytes(1000);
    let f = TempFile::new("short.bin", &bytes);
    assert_eq!(
        content_hash(&f.path, bytes.len() as u64).unwrap(),
        hash_of_parts(bytes.len() as u64, &bytes, None)
    );
}

/// Расширенная проверка по `out/hash_vectors.json` (тот же набор плюс любые
/// добавленные позже). Файла нет — тест молча пропускается.
#[test]
fn content_hash_matches_hash_vectors_json() {
    let path = repo_root().join("tools/parity/out/hash_vectors.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!(
            "пропуск: нет {} (запустите tools/parity/hash_vectors.py)",
            path.display()
        );
        return;
    };
    let v: serde_json::Value = serde_json::from_str(&text).expect("hash_vectors.json");
    let vectors = v["vectors"].as_array().expect("vectors");
    let mut checked = 0usize;
    for vec in vectors {
        let name = vec["name"].as_str().unwrap_or_default();
        let len = vec["len"].as_u64().expect("len") as usize;
        let expected = vec["expected"].as_str().expect("expected");
        let bytes = vector_bytes(len);
        let f = TempFile::new(&format!("{name}.bin"), &bytes);
        assert_eq!(
            content_hash(&f.path, len as u64).unwrap(),
            expected,
            "расхождение на векторе {name}"
        );
        checked += 1;
    }
    println!("фиксированных векторов проверено: {checked}");
}

/// Паритет на 50 реальных файлах (`spike2_hash.py`): вручную —
/// `cargo test -p hds-index --test hash_parity -- --ignored --nocapture`.
#[test]
#[ignore = "нужен прогон tools/parity/spike2_hash.py на этой машине"]
fn content_hash_matches_50_real_files() {
    let path = repo_root().join("tools/parity/out/hash_parity.jsonl");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("нет {}: {e}", path.display()));
    let (mut ok, mut fail) = (0usize, 0usize);
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let p = v["path"].as_str().unwrap_or_default();
        let size = v["size"].as_u64().unwrap_or(0);
        let expected = v["expected"].as_str().unwrap_or_default();
        match content_hash(Path::new(p), size) {
            Ok(h) if h == expected => ok += 1,
            Ok(h) => {
                fail += 1;
                println!("MISMATCH {p}\n  python={expected}\n  rust  ={h}");
            }
            Err(e) => {
                fail += 1;
                println!("IO ERROR {p}: {e}");
            }
        }
    }
    println!("{ok}/{} совпадений content_hash", ok + fail);
    assert_eq!(
        fail, 0,
        "хэш расходится с Python — переиндексация БД недопустима"
    );
}
