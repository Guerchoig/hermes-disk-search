//! Клиент Python-воркера `hds.extract_sidecar` (прототип B6, `PLAN_W2_LLM_HOST.md`
//! §5/B4): извлечение сегментов и лемматизация FTS остаются в Python (§2.5 плана),
//! Rust зовёт их по протоколу **JSON-lines** (одно сообщение — одна строка JSON).
//!
//! Зачем это уже в B4: приёмка требует строгий паритет `segments` и `fts` с golden,
//! а без Python-стороны их не получить. В B6 контракт доводится до автономного
//! `sidecar/hds_extract` (requirements.lock, mpp/java, ffmpeg/whisper, CLIP).
//!
//! Грабли W0/B6, учтённые здесь:
//! * библиотеки Python печатают в stdout — воркер обязан увести их в stderr,
//!   протокол остаётся на stdout (см. `hds/extract_sidecar.py`);
//! * выход по EOF — корректное завершение (0,08 с в спайке 3);
//! * stderr воркера наследуется родителем (не смешивается с протоколом).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use hds_core::error::{CoreError, Result};
use serde_json::{json, Value};

use crate::chunker::Segment;

/// Трейт извлечения: `path → (kind, segments)` (в Python — `extractors.extract`).
pub trait Extractor: Send + Sync {
    fn extract(&self, path: &Path) -> Result<(String, Vec<Segment>)>;
}

/// Трейт лемматизации: тексты чанков → текст для `chunks_fts`.
pub trait Lemmatizer: Send + Sync {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>>;
}

/// Заглушка лемматизации без pymorphy3: токены `[\\w]{2,}` через пробел
/// (деградация без падения, как `hds/lemmatizer.py` при отсутствии словаря).
pub struct TokenLemmatizer;

impl Lemmatizer for TokenLemmatizer {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>> {
        Ok(texts.iter().map(|t| tokens_joined(t)).collect())
    }
}

/// Порт `lemmatizer.normalize` без словаря: `[\\w]{2,}` (Unicode) через пробел.
pub fn tokens_joined(text: &str) -> String {
    let mut out = String::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else {
            flush_token(&mut cur, &mut out);
        }
    }
    flush_token(&mut cur, &mut out);
    out
}

fn flush_token(cur: &mut String, out: &mut String) {
    if cur.chars().count() >= 2 {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(cur);
    }
    cur.clear();
}

struct Inner {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    child: Child,
}

/// Запущенный Python-воркер (единый объект: и Extractor, и Lemmatizer).
pub struct Sidecar {
    inner: Mutex<Inner>,
    py: PathBuf,
    cwd: PathBuf,
    /// Паритетный режим: воркер применяет правила golden.py (roots=[], transcribe
    /// по наличию модели) — нужно для строгого сравнения `segments` с golden.
    parity: bool,
}

impl Sidecar {
    /// Запуск `.venv\Scripts\python.exe -m hds.extract_sidecar` в корне проекта.
    pub fn spawn(py: &Path, cwd: &Path, parity: bool) -> Result<Self> {
        let mut child = Command::new(py)
            .arg("-m")
            .arg("hds.extract_sidecar")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // библиотеки Python шумят в stderr, не в протокол
            .spawn()
            .map_err(|e| CoreError::Other(format!("запуск воркера {}: {e}", py.display())))?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Ok(Sidecar {
            inner: Mutex::new(Inner {
                stdin,
                stdout,
                child,
            }),
            py: py.to_path_buf(),
            cwd: cwd.to_path_buf(),
            parity,
        })
    }

    /// Путь к интерпретатору (для сообщений).
    pub fn python(&self) -> &Path {
        &self.py
    }

    /// Рабочий каталог воркера (корень проекта).
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Обмен: отправляет запрос, читает одну строку JSON-ответа.
    fn call(&self, req: &Value) -> Result<Value> {
        let mut g = self.inner.lock().unwrap();
        let line = req.to_string();
        g.stdin
            .write_all(line.as_bytes())
            .and_then(|_| g.stdin.write_all(b"\n"))
            .and_then(|_| g.stdin.flush())
            .map_err(|e| CoreError::Other(format!("воркер: запись: {e}")))?;
        let mut resp = String::new();
        let n = g
            .stdout
            .read_line(&mut resp)
            .map_err(|e| CoreError::Other(format!("воркер: чтение: {e}")))?;
        if n == 0 {
            return Err(CoreError::Other(
                "воркер закрыл stdout (неожиданный выход)".into(),
            ));
        }
        serde_json::from_str(resp.trim())
            .map_err(|e| CoreError::Other(format!("воркер: не JSON-ответ ({e}): {resp}")))
    }

    /// `hello`: проверка версии/возможностей воркера.
    pub fn hello(&self) -> Result<Value> {
        self.call(&json!({"op": "hello"}))
    }

    /// Завершение: `shutdown` + ожидание процесса.
    pub fn shutdown(&self) {
        let _ = self.call(&json!({"op": "shutdown"}));
        let mut g = self.inner.lock().unwrap();
        let _ = g.child.wait();
    }
}

/// Разбор `segments` из ответа воркера (поля как `extractors.seg()`).
pub fn segments_from_json(v: &Value) -> Result<Vec<Segment>> {
    let arr = v
        .get("segments")
        .and_then(|s| s.as_array())
        .ok_or_else(|| CoreError::Other("в ответе нет segments".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for s in arr {
        out.push(Segment {
            text: s.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            page: s.get("page").and_then(|p| p.as_i64()),
            t_start: s.get("t_start").and_then(|t| t.as_f64()),
            t_end: s.get("t_end").and_then(|t| t.as_f64()),
            head: s.get("head").and_then(|h| h.as_str()).map(|h| h.to_string()),
        });
    }
    Ok(out)
}

impl Extractor for Sidecar {
    fn extract(&self, path: &Path) -> Result<(String, Vec<Segment>)> {
        let resp = self.call(&json!({"op": "extract", "path": path.to_string_lossy(), "parity": self.parity}))?;
        if !resp.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
            let err = resp
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("неизвестная ошибка воркера");
            return Err(CoreError::Other(err.to_string()));
        }
        let kind = resp
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        Ok((kind, segments_from_json(&resp)?))
    }
}

impl Lemmatizer for Sidecar {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let resp = self.call(&json!({"op": "normalize", "texts": texts}))?;
        if !resp.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
            let err = resp
                .get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("неизвестная ошибка нормализации");
            return Err(CoreError::Other(err.to_string()));
        }
        let arr = resp
            .get("fts")
            .and_then(|f| f.as_array())
            .ok_or_else(|| CoreError::Other("в ответе нет fts".into()))?;
        Ok(arr
            .iter()
            .map(|x| x.as_str().unwrap_or("").to_string())
            .collect())
    }
}