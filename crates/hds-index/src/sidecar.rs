//! Адаптер воркера `hds-extract` к трейтам конвейера (`Extractor`/`Lemmatizer`).
//!
//! Извлечение сегментов и лемматизация FTS остаются в Python (§2.5 плана); B6
//! вынес это в автономный воркер `sidecar/hds_extract/worker.py` и клиент
//! `hds-extract` (JSON-RPC 2.0 по stdio, §5 плана). Здесь — тонкая обёртка:
//! `Sidecar` реализует трейты конвейера поверх [`hds_extract::Worker`].
//!
//! Грабли B6 (в воркере):
//! * библиотеки и подпроцессы (`tesseract`/`ffmpeg`/java) уводим от протокола:
//!   stdout библиотек → stderr, а **stdin подпроцессов → nul** (иначе они
//!   наследуют протокольный pipe и блокируются);
//! * выход по EOF — корректное завершение (0,07 с);

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use hds_core::error::Result;

use crate::chunker::Segment;

/// Трейт извлечения: `path → (kind, segments)` (в Python — `extractors.extract`).
pub trait Extractor: Send + Sync {
    fn extract(&self, path: &Path) -> Result<(String, Vec<Segment>)>;
}

/// Extractor для ссылок: позволяет передавать `&Sidecar` внутрь `MediaRouter`
/// (`crates/hds-index/src/transcribe.rs`) без клонирования воркера.
impl<T: Extractor + ?Sized> Extractor for &T {
    fn extract(&self, path: &Path) -> Result<(String, Vec<Segment>)> {
        (**self).extract(path)
    }
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

/// Запущенный Python-воркер (единый объект: и Extractor, и Lemmatizer).
///
/// Реализация — тонкий адаптер над [`hds_extract::Worker`] (B6): протокол
/// JSON-RPC 2.0, `sidecar/hds_extract/worker.py`, перезапуск/таймауты/`shutdown`.
pub struct Sidecar {
    worker: Mutex<hds_extract::Worker>,
}

impl Sidecar {
    /// Запуск воркера (`sidecar/hds_extract/worker.py`) указанным интерпретатором.
    pub fn spawn(py: &Path, cwd: &Path, parity: bool) -> Result<Self> {
        Self::spawn_with(py, cwd, parity, Duration::from_secs(60))
    }

    /// Как [`Sidecar::spawn`], но с явным `idle_timeout` воркера.
    ///
    /// Нужно CLI (`reindex-fts`/`index`): между запросами идёт долгая работа на
    /// стороне родителя (например `DELETE FROM chunks_fts` на сотнях тысяч строк),
    /// за неё воркер успевает выйти по idle-timeout (60 с) и следующий запрос
    /// падает на закрытом stdin. Batch-операциям задаём большой таймаут.
    pub fn spawn_with(py: &Path, cwd: &Path, parity: bool, idle_timeout: Duration) -> Result<Self> {
        let mut cfg = hds_extract::WorkerConfig::new(py, cwd);
        cfg.parity = parity;
        cfg.idle_timeout = idle_timeout;
        let worker = hds_extract::Worker::spawn(cfg)?;
        Ok(Sidecar {
            worker: Mutex::new(worker),
        })
    }

    /// `hello`: возможности воркера (протокол уже проверен при старте).
    pub fn capabilities(&self) -> hds_extract::Capabilities {
        self.worker.lock().unwrap().capabilities().clone()
    }

    /// Живой ли процесс воркера (idle-timeout/сбой убили его?).
    pub fn is_alive(&self) -> bool {
        self.worker.lock().unwrap().is_alive()
    }

    /// Завершение: `shutdown` → EOF на stdin → процесс выходит (§5.1).
    pub fn shutdown(&self) {
        self.worker.lock().unwrap().shutdown();
    }

    /// PID воркера (для замера RSS по дереву процессов, §5.1).
    pub fn pid(&self) -> u32 {
        self.worker.lock().unwrap().pid()
    }
}

/// Конвертация сегмента воркера в сегмент конвейера (поля совпадают).
fn to_segment(s: hds_extract::Segment) -> Segment {
    Segment {
        text: s.text,
        page: s.page,
        t_start: s.t_start,
        t_end: s.t_end,
        head: s.head,
    }
}

impl Extractor for Sidecar {
    fn extract(&self, path: &Path) -> Result<(String, Vec<Segment>)> {
        let r = self.worker.lock().unwrap().extract(path)?;
        Ok((r.kind, r.segments.into_iter().map(to_segment).collect()))
    }
}

impl Lemmatizer for Sidecar {
    fn normalize_many(&self, texts: &[String]) -> Result<Vec<String>> {
        self.worker.lock().unwrap().normalize(texts)
    }
}
