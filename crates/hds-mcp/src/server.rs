//! MCP-транспорт stdio: NDJSON JSON-RPC 2.0 (`initialize`, `tools/list`,
//! `tools/call`, `ping`). Порт поведения `hds/mcp_server.py` (`mcp.run(stdio)`).

use serde_json::{json, Value};

use crate::schema;
use hds_core::access::AuthCtx;

/// Версия протокола MCP, которую объявляем при `initialize`.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Область инструментов маршрута (TR-4.4).
///
/// Обычный `/mcp` — [`Scope::global`]: все инструменты как раньше. Маршрут
/// агента `/mcp/agent/<P>` — [`Scope::agent`]: только поиск фрагментов и статус
/// индекса **своего** проекта, без `ask_my_files` и управления индексацией.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Проект анонимной зоны (`P0001`); `None` — обычный маршрут.
    pub agent_project: Option<String>,
}

impl Scope {
    /// Обычный маршрут (desktop/stdio/`/mcp`).
    pub fn global() -> Scope {
        Scope::default()
    }

    /// Маршрут агента проекта `P` (TR-4.3).
    pub fn agent(project: &str) -> Scope {
        Scope {
            agent_project: Some(project.to_string()),
        }
    }

    /// Это маршрут агента?
    pub fn is_agent(&self) -> bool {
        self.agent_project.is_some()
    }
}

/// «Системный промпт» сервера для обычного маршрута (поле `instructions`).
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

/// «Системный промпт» маршрута агента (TR-4.4): только поиск фрагментов.
///
/// Локальной генерации (`ask_my_files`) здесь нет: ответ синтезирует облачная
/// модель агента по фрагментам. Инструкция прямо говорит, что выдача — по
/// одному проекту и что при предупреждении о неиндексированных файлах поиск
/// неполон.
pub const AGENT_INSTRUCTIONS: &str = "\
Поиск по документам проекта в анонимной зоне (тексты анонимизированы: вместо ФИО, \
организаций и реквизитов стоят метки вида [PERSON_3], [ORG_7]).

Как пользоваться:
- `search_local_files` — поиск фрагментов по словам и смыслу: `query`, \
необязательные `kinds` (text,pdf,docx,xlsx,pptx,image) и `limit` (до 30). \
Выдача — фрагменты с абсолютными путями внутри рабочей папки проекта и номерами \
страниц; файл по этому пути можно открыть и дочитать.
- `index_status` — сколько файлов проекта проиндексировано, сколько ещё в очереди.

Главное:
- Одна выдача — это фрагменты, а не список файлов: на вопросы «найди все…», \
«какие есть…» делайте несколько поисков разными формулировками и синонимами.
- Метки — это обычные слова: `[PERSON_3]` ищется буквально и находит ровно того \
человека, `[PERSON_30]` — другого.
- Если в выдаче есть предупреждение о неиндексированных файлах, поиск неполон: \
эти файлы можно найти инструментом поиска по файлам или прочитать напрямую.
- Видны только документы этого проекта; документы других проектов недоступны.
- Ответ синтезируйте сами по фрагментам и всегда указывайте путь файла и страницу.";

/// Обработать один JSON-RPC объект; `None` — ответ не нужен (notification).
///
/// `auth` — контекст токена (M1 T4); `None` — без авторизации (desktop/stdio):
/// инструменты работают как раньше, без фильтра проектов. `scope` — область
/// маршрута (TR-4.4): на маршруте агента публикуется только поиск.
pub fn handle_scoped(msg: &Value, auth: Option<&AuthCtx>, scope: &Scope) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let is_notification = id.is_none();

    let response = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "disk-search", "version": env!("CARGO_PKG_VERSION") },
            "instructions": if scope.is_agent() { AGENT_INSTRUCTIONS } else { INSTRUCTIONS }
        }),
        "tools/list" => json!({ "tools": schema::tool_list_scoped(scope) }),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = msg
                .pointer("/params/arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match schema::call_scoped(name, &args, auth, scope) {
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

/// Обработать один JSON-RPC объект на обычном маршруте (все инструменты).
pub fn handle(msg: &Value, auth: Option<&AuthCtx>) -> Option<Value> {
    handle_scoped(msg, auth, &Scope::global())
}

/// Обработать строку (для тестов): `None` — ответа нет; `Some` — JSON-строка ответа.
pub fn handle_line(line: &str) -> Option<String> {
    let msg: Value = serde_json::from_str(line.trim()).ok()?;
    handle(&msg, None).map(|v| v.to_string())
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
            Ok(msg) => handle(&msg, None).map(|v| v.to_string()),
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
