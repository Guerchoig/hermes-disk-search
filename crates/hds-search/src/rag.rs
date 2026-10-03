//! RAG-ответ на свободный вопрос — порт `hds/rag.py`: поиск → контекст → чат-роль
//! (`chat.base_url`, `/chat/completions`). Чат-роль — только генератор (инструменты
//! не передаются); thinking выключается шаблонно (`chat_template_kwargs.enable_thinking`).

use hds_core::config::{dig, Config};
use hds_core::http;
use hds_index::{Embedder, Lemmatizer};
use rusqlite::Connection;
use serde_json::json;

use crate::snippet::format_location;
use crate::{rerank::rerank_results, search, SearchResult};

/// Системный промпт (дословно из `hds/rag.py`).
const SYSTEM_PROMPT: &str = "Ты помогаешь искать информацию в локальных файлах пользователя. \
Отвечай на русском языке. Опирайся ТОЛЬКО на приведённые фрагменты документов; если их \
недостаточно — так и скажи. При упоминании фактов указывай источник в формате [N], где N — \
номер фрагмента. Отвечай компактно: суть в нескольких предложениях и короткий список \
источников (путь, страница/таймкод) в конце — ответ генерирует локальная модель, длинные \
ответы не помещаются в таймаут клиента. Не вызывай никаких инструментов, просто ответь текстом.";

/// Подсказка-дозапрос при пустом `content` (всё ушло в размышления).
const CONTINUE_NUDGE: &str = "Твой предыдущий ответ не содержал финального текста — только \
размышления. Дай итоговый ответ на вопрос пользователя по приведённым фрагментам.";

/// Теги размышлений — конкатенацией (как в Python: не держим похожие на разметку строки).
const T_OPEN: &str = concat!("<", "think", ">");
const T_CLOSE: &str = concat!("<", "/", "think", ">");

/// Итог `ask`: ответ модели + использованные источники.
#[derive(Debug, Clone)]
pub struct Answer {
    pub answer: String,
    pub sources: Vec<SearchResult>,
}

impl Answer {
    /// JSON `{answer, sources[]}` (как Python-словарь `rag.ask`).
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "answer": self.answer,
            "sources": self.sources.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
        })
    }
}

/// Убрать inline-теги размышлений из `content` (порт `_strip_think`).
fn strip_think(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(open) = out.find(T_OPEN) {
        match out[open + T_OPEN.len()..].find(T_CLOSE) {
            Some(rel) => {
                let close = open + T_OPEN.len() + rel + T_CLOSE.len();
                out.replace_range(open..close, "");
            }
            None => {
                out.truncate(open);
                break;
            }
        }
    }
    // одиночный закрывающий тег → всё до него (как `(T_CLOSE).*` с DOTALL)
    if let Some(pos) = out.find(T_CLOSE) {
        out.truncate(pos);
    }
    out.trim().to_string()
}

/// `HH:MM:SS` (порт `_fmt_time`).
fn fmt_time(t: f64) -> String {
    let h = (t / 3600.0) as i64;
    let m = ((t / 60.0) as i64) % 60;
    let s = (t as i64) % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Собрать контекст из результатов (порт `build_context`) → `(текст, число блоков)`.
pub fn build_context(results: &[SearchResult], max_chars: usize) -> (String, usize) {
    let mut blocks: Vec<String> = Vec::new();
    let mut used = 0usize;
    for (i, r) in results.iter().enumerate() {
        let mut loc = r.path.clone();
        if let Some(p) = r.page {
            loc.push_str(&format!(", стр. {p}"));
        }
        if let Some(t) = r.t_start {
            let end = match r.t_end {
                Some(v) if v != 0.0 => v,
                _ => t,
            };
            loc.push_str(&format!(", время {}–{}", fmt_time(t), fmt_time(end)));
        }
        let frag: String = r.text.chars().take(3000).collect();
        let block = format!("[{}] Файл: {loc}\n{frag}", i + 1);
        if used + block.chars().count() > max_chars {
            break;
        }
        used += block.chars().count();
        blocks.push(block);
    }
    (blocks.join("\n\n"), blocks.len())
}

/// Тело `/chat/completions` (порт `_chat_payload`): thinking=off → шаблонно выключить.
fn chat_payload(cfg: &Config, messages: serde_json::Value) -> serde_json::Value {
    // Бюджет ответа RAG отделён от общего `chat.max_tokens`: последний стал потолком
    // вывода роли chat для АГЕНТОВ (Cline и др.), которым нужны длинные ответы, а
    // RAG-ответ намеренно короткий — его ждёт MCP-клиент (таймаут 300 с).
    let max_tokens = dig(cfg, "chat.rag_max_tokens")
        .or_else(|| dig(cfg, "chat.max_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(600);
    let mut payload = json!({
        "model": dig(cfg, "chat.model").and_then(|v| v.as_str()).unwrap_or("qwen3.5-9b"),
        "temperature": dig(cfg, "chat.temperature").and_then(|v| v.as_f64()).unwrap_or(0.2),
        "max_tokens": max_tokens,
        "messages": messages,
    });
    let thinking = dig(cfg, "chat.thinking")
        .and_then(|v| v.as_str())
        .unwrap_or("off");
    if thinking.trim().to_lowercase() == "off" {
        payload["chat_template_kwargs"] = json!({ "enable_thinking": false });
    }
    payload
}

/// Ответ на вопрос по локальным файлам (порт `rag.ask`).
pub fn ask(
    conn: &Connection,
    emb: Option<&Embedder>,
    lem: &dyn Lemmatizer,
    cfg: &Config,
    question: &str,
    limit: usize,
) -> Answer {
    let rerank_on = dig(cfg, "rerank.enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let pool = if rerank_on { 20 } else { limit };
    let mut results = search(conn, emb, lem, cfg, question, None, pool);
    if results.is_empty() {
        return Answer {
            answer: "В индексе ничего не найдено. Проиндексируйте диски: hds index".to_string(),
            sources: Vec::new(),
        };
    }
    if rerank_on {
        if let Some(r) = rerank_results(cfg, question, &results, limit) {
            results = r;
        }
    }
    let max_chars = dig(cfg, "chat.max_context_chars")
        .and_then(|v| v.as_i64())
        .unwrap_or(14000) as usize;
    let (context, n_used) = build_context(&results, max_chars);
    let messages = json!([
        { "role": "system", "content": SYSTEM_PROMPT },
        { "role": "user", "content": format!(
            "Вопрос пользователя: {question}\n\nФрагменты из локальных файлов:\n\n{context}"
        ) },
    ]);
    let payload = chat_payload(cfg, messages.clone());
    let base = dig(cfg, "chat.base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://127.0.0.1:8010/v1")
        .trim_end_matches('/');
    let timeout = std::time::Duration::from_secs(
        dig(cfg, "chat.timeout")
            .and_then(|v| v.as_i64())
            .unwrap_or(240)
            .max(1) as u64,
    );

    let post = |body: &serde_json::Value| -> Result<serde_json::Value, String> {
        let (host, port, prefix) = hds_index::embed::split_base(base).map_err(|e| e.message())?;
        let path = format!("{prefix}/chat/completions");
        let resp = http::request(
            &host,
            port,
            "POST",
            &path,
            &[("Content-Type", "application/json")],
            Some(&body.to_string()),
            timeout,
        )
        .map_err(|e| e.message())?;
        if resp.status != 200 {
            return Err(format!(
                "HTTP {}: {}",
                resp.status,
                resp.body.chars().take(200).collect::<String>()
            ));
        }
        resp.json().map_err(|e| e.message())
    };

    match post(&payload) {
        Ok(v) => {
            let msg = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("message"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut content =
                strip_think(msg.get("content").and_then(|c| c.as_str()).unwrap_or(""));
            if content.is_empty() && msg.get("reasoning_content").is_some() {
                let mut nudge = payload.clone();
                let mut msgs = messages.as_array().cloned().unwrap_or_default();
                msgs.push(json!({ "role": "user", "content": CONTINUE_NUDGE }));
                nudge["messages"] = serde_json::Value::Array(msgs);
                if let Ok(v2) = post(&nudge) {
                    content = strip_think(
                        v2.get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("message"))
                            .and_then(|m| m.get("content"))
                            .and_then(|c| c.as_str())
                            .unwrap_or(""),
                    );
                }
            }
            Answer {
                answer: content,
                sources: results[..n_used].to_vec(),
            }
        }
        Err(e) => {
            let locs: Vec<String> = results[..n_used]
                .iter()
                .map(|r| format_location(&r.path, r.page, r.t_start))
                .collect();
            Answer {
                answer: format!(
                    "Не удалось получить ответ модели ({e}). Найденные файлы:\n{}",
                    locs.join("\n")
                ),
                sources: results[..n_used].to_vec(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn res(path: &str, page: Option<i64>, t: Option<f64>) -> SearchResult {
        SearchResult {
            path: path.into(),
            ext: ".txt".into(),
            kind: "text".into(),
            page,
            t_start: t,
            t_end: None,
            text: "текст".into(),
            snippet: String::new(),
            score: 0.0,
        }
    }

    #[test]
    fn strip_think_removes_paired_and_dangling() {
        let paired = format!("до {T_OPEN}секрет{T_CLOSE} после");
        assert_eq!(strip_think(&paired), "до  после");
        let dangling_open = format!("{T_OPEN}остаток");
        assert_eq!(strip_think(&dangling_open), "");
        let dangling_close = format!("{T_CLOSE} финал");
        // как Python `(T_CLOSE).*` с DOTALL: от закрывающего тега до конца — удаляется
        assert_eq!(strip_think(&dangling_close), "");
    }

    #[test]
    fn context_marks_page_and_timecode() {
        let rows = vec![
            res("D:\\a.pdf", Some(1), None),
            res("D:\\b.wav", None, Some(65.0)),
        ];
        let (ctx, n) = build_context(&rows, 10000);
        assert_eq!(n, 2);
        assert!(ctx.contains("[1] Файл: D:\\a.pdf, стр. 1"));
        assert!(ctx.contains("[2] Файл: D:\\b.wav, время 00:01:05–00:01:05"));
    }

    #[test]
    fn context_respects_budget() {
        let rows = vec![res("D:\\a.txt", None, None), res("D:\\b.txt", None, None)];
        let (ctx, n) = build_context(&rows, 5);
        assert_eq!(n, 0);
        assert!(ctx.is_empty());
    }

    #[test]
    fn answer_json_has_answer_and_sources() {
        let a = Answer {
            answer: "готово".into(),
            sources: vec![res("D:\\a.txt", None, None)],
        };
        let j = a.to_json();
        assert_eq!(j["answer"], json!("готово"));
        assert_eq!(j["sources"].as_array().unwrap().len(), 1);
        assert_eq!(j["sources"][0]["path"], json!("D:\\a.txt"));
    }
}
