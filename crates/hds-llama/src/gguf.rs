//! Минимальное чтение метаданных GGUF — нужны только параметры модели для оценки
//! потребности в VRAM (A4, `PLAN_W2_LLM_HOST.md` §A4).
//!
//! Зачем не «калибровочная таблица на глаз»: диспетчер обязан сравнивать
//! «модель + KV» с реальной свободной VRAM **до** загрузки, а KV зависит от
//! `n_ctx`, `n_parallel`, числа слоёв и голов — всё это есть в метаданных GGUF.
//! Формат: magic `GGUF`, версия, число тензоров и KV-пар, затем пары
//! `имя (string) → тип (u32) → значение`.
//!
//! Читаем ровно то, что нужно: `llama.block_count`,
//! `llama.attention.head_count`, `llama.attention.head_count_kv`,
//! `llama.embedding_length`, `general.architecture`.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use crate::error::{EngineError, Result};

/// Параметры модели, влияющие на бюджет VRAM.
#[derive(Debug, Clone, PartialEq)]
pub struct GgufMeta {
    pub architecture: String,
    pub block_count: u32,
    pub head_count: u32,
    pub head_count_kv: u32,
    pub embedding_length: u32,
    /// Размер головы ключей, если задан явно (`*.attention.key_length`).
    pub key_length: Option<u32>,
    /// Размер головы значений, если задан явно (`*.attention.value_length`).
    pub value_length: Option<u32>,
    /// `*.attention.causal`: `false` — энкодер без KV-кэша (bge-m3),
    /// `None` — ключа нет (декодеры по умолчанию причинные).
    pub causal: Option<bool>,
}

impl GgufMeta {
    /// Размер головы (`key_length`, иначе `embedding_length / head_count`).
    pub fn head_dim(&self) -> u32 {
        if let Some(k) = self.key_length {
            return k;
        }
        if self.head_count == 0 {
            0
        } else {
            self.embedding_length / self.head_count
        }
    }

    /// Есть ли KV-кэш: энкодеры (`causal = false`) его не держат —
    /// подтверждено замером A1 (bge-m3: файл 605 МиБ, рост VRAM +636 МиБ).
    pub fn has_kv_cache(&self) -> bool {
        self.causal != Some(false)
    }

    /// Оценка KV-кэша в МиБ для `n_ctx × n_parallel` (см. [`crate::budget`]).
    pub fn kv_mib(&self, n_ctx: i64, n_parallel: i64, bits: KvBits) -> f64 {
        crate::budget::kv_cache_mib(self, n_ctx, n_parallel, bits)
    }
}

/// Тип KV-кэша (в llama-server на этой машине были `--cache-type-k/v q8_0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvBits {
    /// `f16` — 2 байта на элемент (дефолт llama.cpp).
    F16,
    /// `q8_0` — 34 байта на блок из 32 элементов (1,0625 байта на элемент).
    Q8_0,
}

impl KvBits {
    pub fn bytes_per_element(&self) -> f64 {
        match self {
            KvBits::F16 => 2.0,
            KvBits::Q8_0 => 34.0 / 32.0,
        }
    }
}

/// Значения метаданных GGUF, которые нам нужны.
///
/// Префикс ключа — **архитектура** из `general.architecture`, и она бывает
/// разной: `llama.*` у чат-моделей, `bert.*` у bge-m3 (находка A4 — первая
/// версия читателя искала только `llama.*` и не находила параметров у
/// эмбеддинг/реранк-моделей). Поэтому сверяем по суффиксу — это работает для
/// любой архитектуры (`qwen3`, `bert`, `llama`, `xlm-roberta`…).
fn is_wanted(name: &str) -> bool {
    name == "general.architecture"
        || name.ends_with(".block_count")
        || name.ends_with(".attention.head_count")
        || name.ends_with(".attention.head_count_kv")
        || name.ends_with(".attention.key_length")
        || name.ends_with(".attention.value_length")
        || name.ends_with(".attention.causal")
        || name.ends_with(".embedding_length")
}

/// Прочитать метаданные модели. Ошибки чтения/формата — `EngineError::Other`
/// с понятным текстом (файл может быть недокачан — как в реальной жизни).
pub fn read_meta(path: &Path) -> Result<GgufMeta> {
    let mut r = BufReader::new(File::open(path).map_err(|e| {
        EngineError::Other(format!("GGUF {}: не читается ({e})", path.display()))
    })?);
    let mut magic = [0u8; 4];
    r.read_exact(&mut magic)
        .map_err(|e| EngineError::Other(format!("GGUF {}: {e}", path.display())))?;
    if &magic != b"GGUF" {
        return Err(EngineError::Other(format!(
            "{}: это не GGUF (магия {:?})",
            path.display(),
            String::from_utf8_lossy(&magic)
        )));
    }
    let version = read_u32(&mut r)?;
    if version < 2 || version > 3 {
        return Err(EngineError::Other(format!(
            "{}: версия GGUF {version} не поддерживается (ожидается 2 или 3)",
            path.display()
        )));
    }
    let _tensor_count = read_u64(&mut r)?;
    let kv_count = read_u64(&mut r)?;

    let mut arch = String::new();
    let mut block_count = 0u64;
    let mut head_count = 0u64;
    let mut head_kv = 0u64;
    let mut emb = 0u64;
    let mut key_len = 0u64;
    let mut value_len = 0u64;
    let mut causal: Option<bool> = None;
    for _ in 0..kv_count {
        let name = read_string(&mut r)?;
        let vtype = read_u32(&mut r)?;
        let wanted = is_wanted(&name);
        match (wanted, vtype) {
            (true, 8) => {
                let s = read_string(&mut r)?;
                if name == "general.architecture" {
                    arch = s;
                }
            }
            (true, 7) => {
                let mut b = [0u8; 1];
                r.read_exact(&mut b)
                    .map_err(|e| EngineError::Other(format!("GGUF: bool: {e}")))?;
                if name.ends_with(".attention.causal") {
                    causal = Some(b[0] != 0);
                }
            }
            (true, 4) | (true, 10) => {
                let v = if vtype == 4 {
                    read_u32(&mut r)? as u64
                } else {
                    read_u64(&mut r)?
                };
                if name.ends_with(".attention.key_length") {
                    key_len = v;
                } else if name.ends_with(".attention.value_length") {
                    value_len = v;
                } else if name.ends_with(".attention.head_count_kv") {
                    head_kv = v;
                } else if name.ends_with(".attention.head_count") {
                    head_count = v;
                } else if name.ends_with(".block_count") {
                    block_count = v;
                } else if name.ends_with(".embedding_length") {
                    emb = v;
                }
            }
            _ => skip_value(&mut r, vtype, &path)?,
        }
    }
    if block_count == 0 || head_count == 0 || emb == 0 {
        return Err(EngineError::Other(format!(
            "{}: в метаданных нет обязательных ключей (block_count={block_count}, \
             head_count={head_count}, embedding_length={emb})",
            path.display()
        )));
    }
    Ok(GgufMeta {
        architecture: arch,
        block_count: block_count as u32,
        head_count: head_count as u32,
        head_count_kv: if head_kv == 0 { head_count as u32 } else { head_kv as u32 },
        embedding_length: emb as u32,
        key_length: if key_len == 0 { None } else { Some(key_len as u32) },
        value_length: if value_len == 0 { None } else { Some(value_len as u32) },
        causal,
    })
}

fn read_u32<R: Read>(r: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)
        .map_err(|e| EngineError::Other(format!("GGUF: {e}")))?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64<R: Read>(r: &mut R) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)
        .map_err(|e| EngineError::Other(format!("GGUF: {e}")))?;
    Ok(u64::from_le_bytes(b))
}

fn read_string<R: Read>(r: &mut R) -> Result<String> {
    let n = read_u64(r)? as usize;
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)
        .map_err(|e| EngineError::Other(format!("GGUF: строка длиной {n}: {e}")))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Фиксированный размер значения типа (None — строки и массивы).
fn fixed_size(vtype: u32) -> Option<u64> {
    match vtype {
        0 | 1 | 7 => Some(1),
        2 | 3 => Some(2),
        4 | 5 | 6 => Some(4),
        10 | 11 | 12 => Some(8),
        _ => None,
    }
}

fn skip_n<R: Read + std::io::Seek>(r: &mut R, n: u64) -> Result<()> {
    r.seek(std::io::SeekFrom::Current(n as i64))
        .map_err(|e| EngineError::Other(format!("GGUF: пропуск {n} байт: {e}")))?;
    Ok(())
}

/// Пропустить значение (токенизаторы и прочие большие массивы не читаем).
fn skip_value<R: Read + std::io::Seek>(r: &mut R, vtype: u32, path: &Path) -> Result<()> {
    if let Some(sz) = fixed_size(vtype) {
        return skip_n(r, sz);
    }
    match vtype {
        8 => {
            let n = read_u64(r)?;
            skip_n(r, n)
        }
        9 => {
            let et = read_u32(r)?;
            let count = read_u64(r)?;
            match fixed_size(et) {
                // массив чисел — пропускаем целиком одним seek
                Some(sz) => skip_n(r, count.saturating_mul(sz)),
                // массив строк/массивов — поэлементно
                None => {
                    for _ in 0..count {
                        skip_value(r, et, path)?;
                    }
                    Ok(())
                }
            }
        }
        other => Err(EngineError::Other(format!(
            "{}: неизвестный тип значения GGUF {other}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Синтетический GGUF: заголовок + нужные KV + «мусорные» значения
    /// (массив чисел и массив строк), чтобы проверить их пропуск.
    fn synthetic(block_count: u32, head_count: u32, head_kv: u32, emb: u32) -> Vec<u8> {
        fn kv(name: &str, vtype: u32, val: &[u8], b: &mut Vec<u8>) {
            b.extend_from_slice(&(name.len() as u64).to_le_bytes());
            b.extend_from_slice(name.as_bytes());
            b.extend_from_slice(&vtype.to_le_bytes());
            b.extend_from_slice(val);
        }
        fn str_val(s: &str) -> Vec<u8> {
            let mut v = (s.len() as u64).to_le_bytes().to_vec();
            v.extend_from_slice(s.as_bytes());
            v
        }
        let mut b: Vec<u8> = Vec::new();
        b.extend_from_slice(b"GGUF");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes()); // тензоров нет
        b.extend_from_slice(&6u64.to_le_bytes()); // шесть KV
        kv("general.architecture", 8, &str_val("llama"), &mut b);
        kv("llama.block_count", 4, &block_count.to_le_bytes(), &mut b);
        kv("llama.attention.head_count", 4, &head_count.to_le_bytes(), &mut b);
        kv("llama.attention.head_count_kv", 4, &head_kv.to_le_bytes(), &mut b);
        kv("llama.embedding_length", 4, &emb.to_le_bytes(), &mut b);
        let mut arr = 4u32.to_le_bytes().to_vec(); // массив u32: 3 элемента
        arr.extend_from_slice(&3u64.to_le_bytes());
        for x in [1u32, 2, 3] {
            arr.extend_from_slice(&x.to_le_bytes());
        }
        kv("arrays.numbers", 9, &arr, &mut b);
        let mut arr_s = 8u32.to_le_bytes().to_vec(); // массив строк: 2 элемента
        arr_s.extend_from_slice(&2u64.to_le_bytes());
        for s in ["ab", "cd"] {
            arr_s.extend_from_slice(&str_val(s));
        }
        kv("arrays.strings", 9, &arr_s, &mut b);
        b
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("hds-llama-gguf");
        std::fs::create_dir_all(&dir).expect("temp");
        dir.join(format!("{name}-{}.gguf", std::process::id()))
    }

    #[test]
    fn reads_required_metadata_and_skips_arrays() {
        let p = temp_path("synthetic");
        std::fs::write(&p, synthetic(48, 32, 8, 4096)).expect("write");
        let meta = read_meta(&p).expect("meta");
        assert_eq!(meta.architecture, "llama");
        assert_eq!(meta.block_count, 48);
        assert_eq!(meta.head_count, 32);
        assert_eq!(meta.head_count_kv, 8);
        assert_eq!(meta.embedding_length, 4096);
        assert_eq!(meta.head_dim(), 128);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rejects_non_gguf_and_truncated_files() {
        let p = temp_path("bad");
        std::fs::write(&p, b"NOPE").expect("write");
        assert!(read_meta(&p).unwrap_err().to_string().contains("не GGUF"));
        std::fs::write(&p, b"GGUF").expect("write"); // заголовок обрезан
        assert!(read_meta(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }
}


