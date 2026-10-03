//! MCP-транспорт stdio: NDJSON JSON-RPC 2.0 (`initialize`, `tools/list`,
//! `tools/call`, `ping`). Порт поведения `hds/mcp_server.py` (`mcp.run(stdio)`).

use serde_json::{json, Value};

use crate::schema;

/// Версия протокола MCP, которую объявляем при `initialize`.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// «Системный промпт» сервера — поле `instructions` ответа на `initialize`.
///
/// Спека MCP: эти инструкции клиент «должен» подмешивать в системный промпт модели
/// (так делает, например, Goose). У нас его раньше не было вовсе: модель видела
/// только описания отдельных инструментов и потому на «найди технические задания»
/// делала ОДИН запрос, получала первые 8 фрагментов и объявляла это полным ответом —
/// отсюда «неполные результаты» и уточняющие вопросы вместо выборки.
///
/// Поле отдаётся всегда; клиент вправе его игнорировать (Cline 0.0.43 игнорирует —
/// для него та же суть продублирована правилом `cline-rules/disk-search.md`).
pub const INSTRUCTIONS: &str = "\
Локальный поиск по файлам этого компьютера: проиндексированы все диски (документы, PDF, \
офисные файлы, проекты MS Project, картинки с OCR, видео/музыка с транскрипциями речи).

Как пользоваться:
- `search_local_files` — поиск по имени/теме/содержимому («найди на этом компе…»): \
`query`, необязательные `kinds` (text,pdf,docx,xlsx,pptx,mpp,image,media) и `limit`.
- `ask_my_files` — готовый ответ по содержимому файлов со ссылками [N]; его генерирует \
локальная LLM (1–3 минуты). `limit` задаёт глубину выборки: 8 по умолчанию, до 30.
- `index_status` — когда результат пуст или индекс мог устареть.

Главное: одна выдача инструмента — это 8–30 ФРАГМЕНТОВ, а не список файлов. Поэтому на \
запросы «найди все…», «есть ли ещё…», «какие есть…» делай НЕСКОЛЬКО поисков разными \
формулировками и синонимами (русский/английский/транслит), при необходимости с разными \
`kinds` и `limit`, и только потом объединяй найденное в ответ. Не отчитывайся о \
неполном результате как о полном: если выборка заведомо шире — так и скажи и продолжи \
поиск другими запросами. Не заменяй поиск обходом диска через терминал (dir /s, find, \
grep) — индекс отвечает за секунды, обход дерева идёт минуты и падает на недоступных \
каталогах. При цитировании всегда приводи путь файла и страницу/таймкод.";

/// Обработать один JSON-RPC объект; `None` — ответ не нужен (notification).
pub fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let is_notification = id.is_none();

    let response = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "disk-search", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS
        }),
        "tools/list" => json!({ "tools": schema::tool_list() }),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match schema::call(name, &args) {
                Ok(text) => json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }),
                Err(e) => json!({
                    "content": [{ "type": "text", "text": e }],
                    "isError": true
                }),
            }
        }
        "ping" => json!({}),
        "notifications/initialized" | "notifications/cancelled" => return None,
        _ => {
            if is_notification {
                return None;
            }
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }));
        }
    };

    if is_notification {
        return None;
    }
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": response }))
}

/// Обработать строку (для тестов): `None` — ответа нет; `Some` — JSON-строка ответа.
pub fn handle_line(line: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(line.trim()).ok()?;
    handle(&msg).map(|v| v.to_string())
}

/// Запустить MCP-сервер на stdio (блокирующе, до EOF на stdin).
pub fn run_stdio() -> i32 {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(line.trim()) {
            Ok(msg) => handle(&msg).map(|v| v.to_string()),
            Err(e) => Some(
                json!({ "jsonrpc": "2.0", "id": Value::Null,
                        "error": { "code": -32700, "message": format!("Parse error: {e}") } })
                .to_string(),
            ),
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
    0
}
