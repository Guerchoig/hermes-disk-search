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
    /// `*.full_attention_interval` — у гибридных моделей (Qwen3.5/Qwen3-Next и др.)
    /// полный KV-кэш держит только каждый N-й слой, остальные — SSM/линейное
    /// внимание с состоянием фиксированного размера. Находка A4 шага 2:
    /// у Qwen3.5-9B `full_attention_interval = 4` → KV держат 8 слоёв из 32,
    /// а не все (прежняя оценка завышала KV в 4 раза).
    pub full_attention_interval: Option<u32>,
}

impl GgufMeta {
    /// Размер головы (`key_length`, иначе `embedding_length / head_count`).
    pub fn head_dim(&self) -> u32 {
        if let Some(k) = self.key_length {
            return k;
        }
        self.embedding_length
            .checked_div(self.head_count)
            .unwrap_or(0)
    }

    /// Есть ли KV-кэш: энкодеры (`causal = false`) его не держат —
    /// подтверждено замером A1 (bge-m3: файл 605 МиБ, рост VRAM +636 МиБ).
    pub fn has_kv_cache(&self) -> bool {
        self.causal != Some(false)
    }

    /// Сколько слоёв держат **растущий** KV-кэш.
    ///
    /// У гибридных моделей (`full_attention_interval = N`) полное внимание только
    /// в каждом N-м слое: llama.cpp считает такие слои через `n_layer_kv_from_start`
    /// (`has_kv(il)`), а остальные слои — SSM/линейное внимание с состоянием
    /// фиксированного размера (в расчёт «на токен» не входят).
    pub fn kv_layer_count(&self) -> u32 {
        if !self.has_kv_cache() {
            return 0;
        }
        match self.full_attention_interval {
            Some(n) if n > 1 => self.block_count.div_ceil(n),
            _ => self.block_count,
        }
    }

    /// Сколько слоёв держат KV-кэш среди первых `offloaded` слоёв (для замера
    /// с частичным офлоадом: `n_gpu_layers = offloaded`).
    pub fn kv_layer_count_within(&self, offloaded: u32) -> u32 {
        let layers = offloaded.min(self.block_count);
        match self.full_attention_interval {
            Some(n) if n > 1 => layers.div_ceil(n),
            _ => layers,
        }
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
        || name.ends_with(".full_attention_interval")
        || name.ends_with(".embedding_length")
}

/// Прочитать метаданные модели. Ошибки чтения/формата — `EngineError::Other`
/// с понятным текстом (файл может быть недокачан — как в реальной жизни).
pub fn read_meta(path: &Path) -> Result<GgufMeta> {
    let (mut r, kv_count) = open_header(path)?;

    let mut arch = String::new();
    let mut block_count = 0u64;
    let mut head_count = 0u64;
    let mut head_kv = 0u64;
    let mut emb = 0u64;
    let mut key_len = 0u64;
    let mut value_len = 0u64;
    let mut causal: Option<bool> = None;
    let mut full_attn_interval = 0u64;
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
                } else if name.ends_with(".full_attention_interval") {
                    full_attn_interval = v;
                } else if name.ends_with(".block_count") {
                    block_count = v;
                } else if name.ends_with(".embedding_length") {
                    emb = v;
                }
            }
            _ => skip_value(&mut r, vtype, path)?,
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
        head_count_kv: if head_kv == 0 {
            head_count as u32
        } else {
            head_kv as u32
        },
        embedding_length: emb as u32,
        key_length: if key_len == 0 {
            None
        } else {
            Some(key_len as u32)
        },
        value_length: if value_len == 0 {
            None
        } else {
            Some(value_len as u32)
        },
        causal,
        full_attention_interval: if full_attn_interval > 1 {
            Some(full_attn_interval as u32)
        } else {
            None
        },
    })
}

/// Открыть файл GGUF и прочитать заголовок: `(reader, число KV-пар)`.
fn open_header(path: &Path) -> Result<(BufReader<File>, u64)> {
    let mut r =
        BufReader::new(File::open(path).map_err(|e| {
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
    if !(2..=3).contains(&version) {
        return Err(EngineError::Other(format!(
            "{}: версия GGUF {version} не поддерживается (ожидается 2 или 3)",
            path.display()
        )));
    }
    let _tensor_count = read_u64(&mut r)?;
    let kv_count = read_u64(&mut r)?;
    Ok((r, kv_count))
}

/// Тип значения GGUF — человекочитаемое имя (для `bin/gguf_dump`).
pub fn kv_type_name(t: u32) -> &'static str {
    match t {
        0 => "u8",
        1 => "i8",
        2 => "u16",
        3 => "i16",
        4 => "u32",
        5 => "i32",
        6 => "f32",
        7 => "bool",
        8 => "string",
        9 => "array",
        10 => "u64",
        11 => "i64",
        12 => "f64",
        _ => "unknown",
    }
}

/// Значение метаданных GGUF (полный аудит файла, а не только «нужные» ключи).
#[derive(Debug, Clone, PartialEq)]
pub enum GgufValue {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    Str(String),
    /// Массив: тип элемента, длина и первые значения (превью; длинные массивы
    /// читаются пропуском — токенизаторы весят десятки мегабайт).
    Array {
        elem_type: u32,
        len: u64,
        preview: Vec<String>,
    },
}

impl GgufValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            GgufValue::U8(_) => "u8",
            GgufValue::I8(_) => "i8",
            GgufValue::U16(_) => "u16",
            GgufValue::I16(_) => "i16",
            GgufValue::U32(_) => "u32",
            GgufValue::I32(_) => "i32",
            GgufValue::U64(_) => "u64",
            GgufValue::I64(_) => "i64",
            GgufValue::F32(_) => "f32",
            GgufValue::F64(_) => "f64",
            GgufValue::Bool(_) => "bool",
            GgufValue::Str(_) => "string",
            GgufValue::Array { .. } => "array",
        }
    }

    /// Числовое значение (для сверки параметров модели).
    pub fn as_i64(&self) -> Option<i64> {
        Some(match self {
            GgufValue::U8(v) => *v as i64,
            GgufValue::I8(v) => *v as i64,
            GgufValue::U16(v) => *v as i64,
            GgufValue::I16(v) => *v as i64,
            GgufValue::U32(v) => *v as i64,
            GgufValue::I32(v) => *v as i64,
            GgufValue::U64(v) => *v as i64,
            GgufValue::I64(v) => *v,
            GgufValue::F32(v) => *v as i64,
            GgufValue::F64(v) => *v as i64,
            GgufValue::Bool(v) => *v as i64,
            _ => return None,
        })
    }

    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            GgufValue::F32(v) => *v as f64,
            GgufValue::F64(v) => *v,
            other => other.as_i64()? as f64,
        })
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            GgufValue::Bool(v) => Some(*v),
            other => other.as_i64().map(|v| v != 0),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            GgufValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Короткая строка для печати/`--json`.
    pub fn describe(&self) -> String {
        const MAX: usize = 96;
        let cut = |s: &str| -> String {
            if s.chars().count() <= MAX {
                s.to_string()
            } else {
                let head: String = s.chars().take(MAX).collect();
                format!("{head}…")
            }
        };
        match self {
            GgufValue::Str(s) => format!("{:?}", cut(s)),
            GgufValue::F32(v) => format!("{v}"),
            GgufValue::F64(v) => format!("{v}"),
            GgufValue::Array {
                elem_type,
                len,
                preview,
            } => {
                let p = if preview.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", cut(&preview.join(", ")))
                };
                format!("{}[{len}]{p}", kv_type_name(*elem_type))
            }
            other => other.as_i64().map(|v| v.to_string()).unwrap_or_default(),
        }
    }
}

/// Одна KV-пара метаданных.
#[derive(Debug, Clone, PartialEq)]
pub struct GgufKv {
    pub name: String,
    pub type_id: u32,
    pub value: GgufValue,
}

/// Прочитать **все** KV-пары метаданных (аудит модели; `bin/gguf_dump`).
pub fn read_all(path: &Path) -> Result<Vec<GgufKv>> {
    let (mut r, kv_count) = open_header(path)?;
    let mut out = Vec::new();
    for _ in 0..kv_count {
        let name = read_string(&mut r)?;
        let vtype = read_u32(&mut r)?;
        let value = read_value(&mut r, vtype, path)?;
        out.push(GgufKv {
            name,
            type_id: vtype,
            value,
        });
    }
    Ok(out)
}

fn io_err(e: std::io::Error) -> EngineError {
    EngineError::Other(format!("GGUF: {e}"))
}

/// Прочитать значение любого типа (длинные массивы — с превью, остальное пропуском).
fn read_value<R: Read + std::io::Seek>(r: &mut R, vtype: u32, path: &Path) -> Result<GgufValue> {
    const PREVIEW: usize = 8;
    const MAX_NUMERIC: u64 = 8192;
    match vtype {
        0 | 1 | 7 => {
            let mut b = [0u8; 1];
            r.read_exact(&mut b).map_err(io_err)?;
            Ok(match vtype {
                0 => GgufValue::U8(b[0]),
                1 => GgufValue::I8(b[0] as i8),
                _ => GgufValue::Bool(b[0] != 0),
            })
        }
        2 | 3 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b).map_err(io_err)?;
            Ok(if vtype == 2 {
                GgufValue::U16(u16::from_le_bytes(b))
            } else {
                GgufValue::I16(i16::from_le_bytes(b))
            })
        }
        4 => Ok(GgufValue::U32(read_u32(r)?)),
        5 => Ok(GgufValue::I32(read_u32(r)? as i32)),
        6 => Ok(GgufValue::F32(f32::from_bits(read_u32(r)?))),
        8 => Ok(GgufValue::Str(read_string(r)?)),
        10 => Ok(GgufValue::U64(read_u64(r)?)),
        11 => Ok(GgufValue::I64(read_u64(r)? as i64)),
        12 => Ok(GgufValue::F64(f64::from_bits(read_u64(r)?))),
        9 => {
            let elem_type = read_u32(r)?;
            let len = read_u64(r)?;
            let mut preview: Vec<String> = Vec::new();
            // числовой массив небольшого размера читаем целиком (например,
            // per-layer `head_count_kv` гибридных моделей)
            if fixed_size(elem_type).is_some() && len <= MAX_NUMERIC {
                for i in 0..len {
                    let v = read_value(r, elem_type, path)?;
                    if (i as usize) < PREVIEW {
                        preview.push(v.describe());
                    }
                }
                return Ok(GgufValue::Array {
                    elem_type,
                    len,
                    preview,
                });
            }
            // длинный/нечисловой: превью головы, хвост — пропуском
            let head = len.min(PREVIEW as u64);
            for _ in 0..head {
                let v = read_value(r, elem_type, path)?;
                preview.push(v.describe());
            }
            for _ in head..len {
                skip_value(r, elem_type, path)?;
            }
            Ok(GgufValue::Array {
                elem_type,
                len,
                preview,
            })
        }
        other => Err(EngineError::Other(format!(
            "{}: неизвестный тип значения GGUF {other}",
            path.display()
        ))),
    }
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
        4..=6 => Some(4),
        10..=12 => Some(8),
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

/// Разбор имени шардированной GGUF (`llama.cpp`: `<base>-00001-of-00003.gguf`):
/// возвращает `(база, индекс части с 1, всего частей)`; `None` — имя не шард.
pub fn parse_shard_name(file: &str) -> Option<(String, u32, u32)> {
    let stem = if file.to_ascii_lowercase().ends_with(".gguf") {
        &file[..file.len() - 5]
    } else {
        return None;
    };
    let (left, total_s) = stem.rsplit_once("-of-")?;
    let (base, idx_s) = left.rsplit_once('-')?;
    if base.is_empty()
        || !idx_s.bytes().all(|b| b.is_ascii_digit())
        || !total_s.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((base.to_string(), idx_s.parse().ok()?, total_s.parse().ok()?))
}

/// Размер модели в МиБ с учётом шардов: llama.cpp грузит модель по ПЕРВОМУ
/// фрагменту (`…-00001-of-000NN.gguf`), остальные части находит рядом — поэтому
/// «размер файла» для оценки VRAM берём как сумму всех частей каталога.
/// Для не-шардированного файла — обычный размер.
pub fn file_total_mib(path: &std::path::Path) -> u64 {
    let own = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let (base, _idx, total) = match parse_shard_name(&name) {
        Some(x) => x,
        None => return own >> 20,
    };
    let dir = match path.parent() {
        Some(d) => d,
        None => return own >> 20,
    };
    let mut sum = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if matches!(
                parse_shard_name(&n),
                Some((b, _, t)) if b == base && t == total
            ) {
                sum += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (if sum > 0 { sum } else { own }) >> 20
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
        b.extend_from_slice(&7u64.to_le_bytes()); // семь KV-пар (см. ниже)
        kv("general.architecture", 8, &str_val("llama"), &mut b);
        kv("llama.block_count", 4, &block_count.to_le_bytes(), &mut b);
        kv(
            "llama.attention.head_count",
            4,
            &head_count.to_le_bytes(),
            &mut b,
        );
        kv(
            "llama.attention.head_count_kv",
            4,
            &head_kv.to_le_bytes(),
            &mut b,
        );
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
    fn reads_hybrid_kv_layers_from_full_attention_interval() {
        // копия синтетики + ключ гибридности (как у Qwen3.5-9B: интервал 4)
        let mut b = synthetic(32, 16, 4, 4096);
        // дописываем KV-пару: имя + тип u32 + значение
        let name = "qwen35.full_attention_interval";
        b.extend_from_slice(&(name.len() as u64).to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        // и правим число KV-пар: было 7, стало 8 (наша пара — восьмая)
        let kv_count_offset = 4 + 4 + 8;
        b[kv_count_offset..kv_count_offset + 8].copy_from_slice(&8u64.to_le_bytes());

        let p = temp_path("hybrid");
        std::fs::write(&p, b).expect("write");
        let all = read_all(&p).expect("all");
        assert_eq!(
            all.last().map(|kv| kv.name.as_str()),
            Some("qwen35.full_attention_interval"),
            "дампа должно быть 8 пар, последняя — наша"
        );
        let meta = read_meta(&p).expect("meta");
        assert_eq!(meta.full_attention_interval, Some(4));
        assert_eq!(meta.kv_layer_count(), 8, "32 слоя / интервал 4");
        assert_eq!(meta.kv_layer_count_within(8), 2);
        assert_eq!(meta.kv_layer_count_within(32), 8);
        // KV f16 при 32768 = 32768 × 8 × 4 головы × 512 × 2 Б = 1024 МиБ
        let kv = crate::budget::kv_cache_mib(&meta, 32768, 1, KvBits::F16);
        assert!((kv - 1024.0).abs() < 1.0, "KV = {kv}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn parse_shard_name_and_sizes() {
        assert_eq!(
            parse_shard_name("Qwen3.5-14B-Q6_K-00001-of-00003.gguf"),
            Some(("Qwen3.5-14B-Q6_K".to_string(), 1, 3))
        );
        assert_eq!(
            parse_shard_name("m-00003-of-00012.gguf"),
            Some(("m".to_string(), 3, 12))
        );
        // не шард: без суффикса, не .gguf, «-of-» без цифр
        assert_eq!(parse_shard_name("model.gguf"), None);
        assert_eq!(parse_shard_name("a-of-b.gguf"), None);
        assert_eq!(parse_shard_name("m-00001-of-00003.bin"), None);
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
