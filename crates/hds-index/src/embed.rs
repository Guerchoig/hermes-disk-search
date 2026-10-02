//! Клиент эмбеддингов — порт `hds/embedder.py`, но **только через фасад**
//! `llm-host` (`:8011`, `PLAN_W2_LLM_HOST.md` §5/B4): кросс-процессной адресации
//! инстансов нет (замер A3, `W2_REPORT.md` §6), поэтому единственный путь —
//! OpenAI-совместимый `/v1/embeddings`.
//!
//! Сохранено поведение Python:
//! * батчи `embedding.batch_size`, ответ сортируется по `index`;
//! * при `len(data) != len(batch)` — ошибка «ожидались N векторов, пришло M»;
//! * 4 попытки с паузой `2*(attempt+1)`; `400/404` — без ретраев;
//! * после первой ошибки `available = false` (поиск/индексация не тормозят);
//! * тексты хинтов — как в `embedder.py` (смысл сохранён, дословно по-русски).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hds_core::config::Config;
use hds_core::error::{CoreError, Result};
use hds_core::http;

use hds_core::dig;

/// Размер батча по умолчанию (`embedding.batch_size`, как `make_embedder`).
pub const DEFAULT_BATCH: usize = 64;
/// Таймаут запроса (как `Embedder(timeout=600)`).
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Клиент эмбеддингов OpenAI-совместимого API (фасад `:8011`).
pub struct Embedder {
    base_url: String,
    model: String,
    batch: usize,
    timeout: Duration,
    /// Выключается после первой ошибки (как `self.available = False`).
    available: AtomicBool,
}

impl Embedder {
    /// Явные параметры (для тестов).
    pub fn new(base_url: &str, model: &str, batch_size: usize, timeout: Duration) -> Self {
        Embedder {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            batch: batch_size.max(1),
            timeout,
            available: AtomicBool::new(true),
        }
    }

    /// Порт `make_embedder(cfg)`: `embedding.base_url/model/batch_size`.
    pub fn from_config(cfg: &Config) -> Self {
        let base = dig(cfg, "embedding.base_url")
            .and_then(|v| v.as_str())
            .unwrap_or("http://127.0.0.1:8011/v1");
        let model = dig(cfg, "embedding.model")
            .and_then(|v| v.as_str())
            .unwrap_or("text-embedding-bge-m3");
        let batch = dig(cfg, "embedding.batch_size")
            .and_then(|v| v.as_i64())
            .unwrap_or(DEFAULT_BATCH as i64) as usize;
        Embedder::new(
            base,
            model,
            batch,
            Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        )
    }

    /// Адрес базовой точки (для сообщений об ошибках).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Модель (для сообщений об ошибках).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Текущее состояние доступности.
    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    /// Порт `embed(texts)`: батчами по `batch_size`, с сохранением порядка.
    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.batch) {
            out.extend(self.post(chunk)?);
        }
        Ok(out)
    }

    /// Порт `embed_query(text)`.
    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(&[text.to_string()])?;
        v.pop()
            .ok_or_else(|| CoreError::Other("пустой ответ эмбеддингов".into()))
    }

    /// Порт `ping()`: один короткий запрос «ping».
    pub fn ping(&self) -> Result<()> {
        self.embed(&["ping".to_string()]).map(|_| {
            self.available.store(true, Ordering::Relaxed);
        })
    }

    /// Порт `_post(batch)`: POST `/embeddings` с ретраями и хинтами об ошибках.
    fn post(&self, batch: &[String]) -> Result<Vec<Vec<f32>>> {
        let (host, port, prefix) = split_base(&self.base_url)?;
        let path = format!("{prefix}/embeddings");
        let body = serde_json::json!({"model": self.model, "input": batch}).to_string();
        let mut last = String::new();
        for attempt in 0..4 {
            let result = http::request(
                &host,
                port,
                "POST",
                &path,
                &[("Content-Type", "application/json")],
                Some(&body),
                self.timeout,
            );
            match result {
                Ok(resp) if resp.status == 200 => {
                    let data = resp
                        .json()?
                        .get("data")
                        .and_then(|d| d.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if data.len() != batch.len() {
                        return Err(self.fail(format!(
                            "ожидались {} векторов, пришло {}",
                            batch.len(),
                            data.len()
                        )));
                    }
                    let mut items: Vec<(i64, Vec<f32>)> = Vec::with_capacity(data.len());
                    for d in &data {
                        let idx = d.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                        let vec = d
                            .get("embedding")
                            .and_then(|e| e.as_array())
                            .map(|a| {
                                a.iter()
                                    .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                                    .collect::<Vec<f32>>()
                            })
                            .unwrap_or_default();
                        items.push((idx, vec));
                    }
                    items.sort_by_key(|(i, _)| *i);
                    return Ok(items.into_iter().map(|(_, v)| v).collect());
                }
                Ok(resp) => {
                    last = format!(
                        "HTTP {}: {}",
                        resp.status,
                        resp.body.chars().take(300).collect::<String>()
                    );
                    if resp.status == 400 || resp.status == 404 {
                        break; // модель не установлена/не загружена — ретраи бессмысленны
                    }
                }
                Err(e) => last = e.message(),
            }
            if attempt < 3 {
                std::thread::sleep(Duration::from_secs(2 * (attempt as u64 + 1)));
            }
        }
        Err(self.fail(last))
    }

    /// Формирует ошибку с хинтом (как `EmbeddingError` в Python) и гасит `available`.
    fn fail(&self, last: String) -> CoreError {
        self.available.store(false, Ordering::Relaxed);
        let low = last.to_lowercase();
        let hint = if low.contains("exceed_context") || low.contains("context size") {
            "llama-server отвечает ЯВНОЙ ошибкой контекста: модель загружена с ctx \
             меньше длины входа. Перезапустите роль (контекст задаётся \
             llm_server.embedding.ctx_per_slot = 8192): bin\\llm_host.exe stop, затем bin\\llm_host.exe run."
        } else if low.contains("connection")
            || low.contains("max retries")
            || low.contains("failed to establish")
            || low.contains("connect")
        {
            "Похоже, llama-server (роль embedding) не запущен: запустите владельца \
             ролей — bin\\llm_host.exe run (или задача HermesDiskSearchLlmHost)."
        } else if low.contains("model") && (low.contains("not found") || low.contains("no model")) {
            "GGUF-модель не найдена на диске (общий llama-рантайм). Скачайте её \
             кнопкой «Скачать модель» в веб-интерфейсе или установщиком рантайма."
        } else {
            "Проверьте состояние сервера: карточка «Проверка компонентов» в UI \
             (hds ui) или bin\\hds.exe check."
        };
        CoreError::Other(format!(
            "Эмбеддинги недоступны (модель '{}' на {}): {}. {}",
            self.model, self.base_url, last, hint
        ))
    }
}

/// Разбор `http://host:port/prefix` → `(host, port, "/prefix")`.
pub fn split_base(url: &str) -> Result<(String, u16, String)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| CoreError::Other(format!("не HTTP-адрес: {url}")))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|e| CoreError::Other(format!("порт в {url}: {e}")))?,
        ),
        None => (hostport.to_string(), 80),
    };
    Ok((host, port, path.to_string()))
}

/// Упаковка вектора в blob `struct.pack("<%df", *v)` (как `_commit_file`).
pub fn vector_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
