//! Кадрирование и разбор JSON-RPC 2.0 (NDJSON) — контракт §5 плана.

use hds_core::error::{CoreError, Result};
use serde_json::{json, Value};

/// Версия протокола воркера (`hello.protocol`); родитель сверяет её.
pub const PROTOCOL_VERSION: i64 = 1;

/// Сегмент извлечения — как `hds/extractors.seg()`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Segment {
    pub text: String,
    pub page: Option<i64>,
    pub t_start: Option<f64>,
    pub t_end: Option<f64>,
    pub head: Option<String>,
}

/// Результат `extract`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtractResult {
    pub kind: String,
    pub segments: Vec<Segment>,
    pub warnings: Vec<String>,
    pub elapsed_ms: f64,
}

/// Ответ `hello`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities {
    pub protocol: i64,
    pub python: String,
    pub pid: u32,
    pub capabilities: Vec<String>,
}

impl Capabilities {
    /// Есть ли возможность (например `normalize`, `ocr`).
    pub fn has(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }
}

/// Структурированная ошибка JSON-RPC (`{code, message, hint}`).
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub hint: Option<String>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)?;
        if let Some(h) = &self.hint {
            write!(f, " — {h}")?;
        }
        Ok(())
    }
}

impl RpcError {
    /// Как ошибка ядра (текст попадает в `files.error`, как `str(e)` в Python).
    pub fn to_core(&self) -> CoreError {
        CoreError::Other(self.to_string())
    }
}

/// Строка запроса JSON-RPC 2.0 (без перевода строки — его добавит `Worker`).
pub fn request(id: i64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// Разбор строки-ответа: `result` при успехе, иначе ошибка (сверяется `id`).
pub fn parse_response(line: &str, expect_id: i64) -> Result<Value> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| CoreError::Other(format!("воркер: не JSON ({e}): {line}")))?;
    if let Some(err) = v.get("error") {
        return Err(parse_error(err).to_core());
    }
    let id = v.get("id").and_then(|i| i.as_i64());
    if id != Some(expect_id) {
        return Err(CoreError::Other(format!(
            "воркер: неожиданный id {id:?} (ждали {expect_id})"
        )));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

/// Разбор объекта ошибки JSON-RPC.
pub fn parse_error(v: &Value) -> RpcError {
    RpcError {
        code: v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
        message: v
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("неизвестная ошибка воркера")
            .to_string(),
        hint: v
            .get("hint")
            .and_then(|h| h.as_str())
            .map(|h| h.to_string()),
    }
}

/// Разбор `hello.result`.
pub fn capabilities_from(v: &Value) -> Capabilities {
    Capabilities {
        protocol: v.get("protocol").and_then(|p| p.as_i64()).unwrap_or(0),
        python: v
            .get("python")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string(),
        pid: v.get("pid").and_then(|p| p.as_u64()).unwrap_or(0) as u32,
        capabilities: v
            .get("capabilities")
            .and_then(|c| c.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Разбор `extract.result`.
pub fn extract_from(v: &Value) -> ExtractResult {
    let segments = v
        .get("segments")
        .and_then(|s| s.as_array())
        .map(|a| a.iter().map(segment_from).collect())
        .unwrap_or_default();
    ExtractResult {
        kind: v
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string(),
        segments,
        warnings: v
            .get("warnings")
            .and_then(|w| w.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        elapsed_ms: v.get("elapsed_ms").and_then(|e| e.as_f64()).unwrap_or(0.0),
    }
}

fn segment_from(s: &Value) -> Segment {
    Segment {
        text: s
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string(),
        page: s.get("page").and_then(|p| p.as_i64()),
        t_start: s.get("t_start").and_then(|t| t.as_f64()),
        t_end: s.get("t_end").and_then(|t| t.as_f64()),
        head: s
            .get("head")
            .and_then(|h| h.as_str())
            .map(|h| h.to_string()),
    }
}

/// Разбор `normalize.result`.
pub fn lemmas_from(v: &Value) -> Vec<String> {
    v.get("lemmas")
        .and_then(|l| l.as_array())
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}
