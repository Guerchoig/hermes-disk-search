//! A5 — фасад OpenAI-совместимого HTTP `:8010–8012` (ядро без сети: маршрутизация,
//! сборка запроса к cluster API, ответы и режимы размышлений).
//!
//! Зачем фасад: замер A3 показал, что **кросс-процессной адресации инстансов нет** —
//! другие процессы (`index`/`watch`, `mcp-http`, `ui`, внешние агенты) не видят
//! инстансы `llm-host`, поэтому единственный гарантированный способ обращаться к
//! чату/эмбеддингам — HTTP-фасад на тех же портах, что были у `llama-server`.
//!
//! Что здесь есть (проверяется тестами без движка — см. `tests/facade_core.rs`):
//! * маршрутизация как у `llama-server` (`/health`, `/props`, `/v1/models`,
//!   `/v1/chat/completions`, `/v1/embeddings`, `/v1/rerank`);
//! * сборка `prompt` из OpenAI-сообщений. **Факт замера (`bin/chat_probe`):**
//!   cluster `chat_complete` принимает готовый `prompt`, но движок **сам применяет
//!   шаблон чата модели** (плоский текст на ~100 символов дал 56 токенов входа,
//!   ChatML-маркеры добавили ещё 12 — значит шаблон накладывается поверх).
//!   Отсюда правило фасада: маркеры шаблона не ставим, история сводится в один
//!   текст (system → первым абзацем), как это делала Python-версия в RAG-запросах;
//! * режимы размышлений (`chat` → `reasoning=off`, `chat-think` → `on` + видимый
//!   формат, `chat_template_kwargs.enable_thinking`/`reasoning` в теле — приоритетнее);
//! * ответы в форме, которую ждут клиенты (в т.ч. Python-версия): `choices[0].message.content`,
//!   `usage`, а также `reasoning_content`, если размышления включены;
//! * `strip_think` — дословный порт `hds/rag.py::_strip_think` (бывает `thinking=auto`,
//!   когда сервер оставляет размышления прямо в `content`).
//!
//! Чего здесь ещё нет (следующий шаг A5): сам HTTP-сервер и бинарь `llm_host_facade`
//! (диспетчер `dispatch::apply` + инстансы + потоки на порты) — они добавляются поверх
//! этого ядра, чтобы решения проверялись тестами без сокетов.

use serde_json::{json, Value};

use crate::error::{EngineError, Result};

/// Порты по умолчанию — как у `llama-server` (совместимость клиентов сохраняем).
pub const PORTS: [(&str, u16); 3] = [("chat", 8010), ("embedding", 8011), ("rerank", 8012)];

/// Порт роли по умолчанию (`0` — роли нет в фасаде).
pub fn default_port(role: &str) -> u16 {
    PORTS
        .iter()
        .find(|(r, _)| *r == role)
        .map(|(_, p)| *p)
        .unwrap_or(0)
}

/// Маршрут запроса (по методу и пути без query-строки).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Health,
    Props,
    Models,
    Chat,
    Embeddings,
    Rerank,
    /// A6: внутренний API управления (`/internal/*`) — для CLI `llm-host`.
    InternalStatus,
    InternalLoad,
    InternalUnload,
    InternalDevices,
    InternalStop,
    /// W3: транскрибация через владельца GPU (роль whisper).
    InternalTranscribe,
    /// Не нашли — 404 с понятным текстом.
    NotFound,
}

impl Route {
    /// Роль инстанса, которого требует маршрут (`None` — общая информация).
    pub fn role(&self) -> Option<&'static str> {
        match self {
            Route::Chat => Some("chat"),
            Route::Embeddings => Some("embedding"),
            Route::Rerank => Some("rerank"),
            _ => None,
        }
    }

    /// Нужен ли телу запроса JSON.
    pub fn needs_body(&self) -> bool {
        matches!(
            self,
            Route::Chat
                | Route::Embeddings
                | Route::Rerank
                | Route::InternalLoad
                | Route::InternalUnload
                | Route::InternalTranscribe
        )
    }

    /// Внутренний маршрут управления (не для внешних клиентов).
    ///
    /// Их обслуживает **только** фасад на loopback: снаружи они позволяют
    /// выгрузить чужие роли и остановить хост, поэтому при
    /// `ServerConfig.internal = false` отвечаем `403`.
    pub fn is_internal(&self) -> bool {
        matches!(
            self,
            Route::InternalStatus
                | Route::InternalLoad
                | Route::InternalUnload
                | Route::InternalDevices
                | Route::InternalStop
                | Route::InternalTranscribe
        )
    }

    /// Путь апстрима для режима `llm_server.mode: facade` (без `/v1`).
    pub fn upstream_path(&self) -> Option<&'static str> {
        match self {
            Route::Chat => Some("chat/completions"),
            Route::Embeddings => Some("embeddings"),
            Route::Rerank => Some("rerank"),
            _ => None,
        }
    }
}

/// Разбор маршрута: пути с `/v1` и без него (клиенты ходят и так, и так).
pub fn route(method: &str, path: &str) -> Route {
    let p = path.split('?').next().unwrap_or("").trim_end_matches('/');
    let m = method.to_ascii_uppercase();
    match (m.as_str(), p) {
        (_, "/health") | (_, "/v1/health") => Route::Health,
        ("GET", "/props") | ("GET", "/v1/props") => Route::Props,
        ("GET", "/models") | ("GET", "/v1/models") => Route::Models,
        ("POST", "/chat/completions") | ("POST", "/v1/chat/completions") => Route::Chat,
        ("POST", "/embeddings") | ("POST", "/v1/embeddings") => Route::Embeddings,
        ("POST", "/rerank") | ("POST", "/v1/rerank") | ("POST", "/v1/rerank/rerank") => {
            Route::Rerank
        }
        ("GET", "/internal/status") | ("GET", "/v1/internal/status") => Route::InternalStatus,
        ("GET", "/internal/devices") | ("GET", "/v1/internal/devices") => Route::InternalDevices,
        ("POST", "/internal/load") | ("POST", "/v1/internal/load") => Route::InternalLoad,
        ("POST", "/internal/unload") | ("POST", "/v1/internal/unload") => Route::InternalUnload,
        ("POST", "/internal/stop") | ("POST", "/v1/internal/stop") => Route::InternalStop,
        ("POST", "/internal/transcribe") | ("POST", "/v1/internal/transcribe") => {
            Route::InternalTranscribe
        }
        _ => Route::NotFound,
    }
}

/// Режим размышлений (план §11.4): `off` → движок форсирует `reasoning_budget = 0`,
/// `on` → `reasoning_budget = -1` и видимый формат (`reasoning_format = none`),
/// `auto` → решение за шаблоном модели (флаги не отправляем).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    Off,
    On,
    Auto,
}

impl Thinking {
    pub fn as_str(&self) -> &'static str {
        match self {
            Thinking::Off => "off",
            Thinking::On => "on",
            Thinking::Auto => "auto",
        }
    }

    pub fn parse(s: &str) -> Thinking {
        match s.trim().to_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => Thinking::On,
            "auto" => Thinking::Auto,
            _ => Thinking::Off,
        }
    }
}

impl Default for Thinking {
    /// Как в `chat.thinking` Python-версии: по умолчанию размышления выключены
    /// (быстрые RAG-ответы в таймаут клиента).
    fn default() -> Self {
        Thinking::Off
    }
}

/// Маркеры размышлений собираем из кодов: литеральные угловые скобки с `think`
/// внутри «съедаются» частью инструментов (в Python то же решение — `hds/rag.py`).
fn think_markers() -> (String, String) {
    let lt = '\u{3c}';
    let gt = '\u{3e}';
    (format!("{lt}think{gt}"), format!("{lt}/think{gt}"))
}

/// Разделить вывод модели на размышления и ответ по маркерам ``…`` .
///
/// Движок при `reasoning=on` отдаёт закрывающий маркер `</think>` даже **без**
/// открывающего (открывающий добавляет шаблон модели): поэтому всё до последнего
/// `</think>` — размышления, а после — ответ (порт `hds/rag.py::_strip_think`;
/// его regex тоже резал вывод до закрывающего маркера).
pub fn split_reasoning(text: &str) -> (Option<String>, String) {
    let (open, close) = think_markers();
    if let Some(j) = text.rfind(&close) {
        let before = &text[..j];
        let start = before.rfind(&open).map(|i| i + open.len()).unwrap_or(0);
        let reasoning = before[start..].trim().to_string();
        let answer = text[j + close.len()..].trim().to_string();
        return (Some(reasoning), answer);
    }
    if let Some(i) = text.find(&open) {
        // открыли, не закрыли: до маркера — ответ, после — обрыв размышлений
        return (
            Some(text[i + open.len()..].trim().to_string()),
            text[..i].trim().to_string(),
        );
    }
    (None, text.trim().to_string())
}

/// Порт `hds/rag.py::_strip_think`: убрать inline-размышления из `content`
/// (бывает при `thinking = auto`/`on`, когда сервер оставил их прямо в тексте).
pub fn strip_think(text: &str) -> String {
    split_reasoning(text).1
}

/// Решение о режиме размышлений по телу запроса и алиасу модели.
///
/// Приоритет (от старшего к младшему):
/// 1. `chat_template_kwargs.enable_thinking` — так отключает размышления Python-версия
///    (`hds/rag.py`, `thinking: off`);
/// 2. `reasoning` (`off`/`on`/`auto`) — llama-server-совместимое поле;
/// 3. алиас модели: `chat-think` → `on`, иначе значение по умолчанию из конфига.
pub fn thinking_for(model_alias: &str, body: &Value, default: Thinking) -> Thinking {
    if let Some(v) = body
        .get("chat_template_kwargs")
        .and_then(|k| k.get("enable_thinking"))
        .and_then(|v| v.as_bool())
    {
        return if v { Thinking::On } else { Thinking::Off };
    }
    if let Some(s) = body.get("reasoning").and_then(|v| v.as_str()) {
        return Thinking::parse(s);
    }
    // `reasoning_effort` (OpenAI-совместимые агенты, в т.ч. Cline) и `reasoning: {effort|enabled}` —
    // включённые размышления. Явное `none`/`off` — выключаем.
    if let Some(e) = body.get("reasoning_effort").and_then(|v| v.as_str()) {
        let e = e.trim().to_lowercase();
        return if !e.is_empty() && e != "none" && e != "off" {
            Thinking::On
        } else {
            Thinking::Off
        };
    }
    if let Some(obj) = body.get("reasoning").and_then(|v| v.as_object()) {
        if let Some(e) = obj.get("effort").and_then(|v| v.as_str()) {
            let e = e.trim().to_lowercase();
            return if !e.is_empty() && e != "none" && e != "off" {
                Thinking::On
            } else {
                Thinking::Off
            };
        }
        if obj.get("enabled").and_then(|v| v.as_bool()) == Some(true) {
            return Thinking::On;
        }
    }
    if model_alias.trim().to_lowercase().ends_with("-think") {
        return Thinking::On;
    }
    default
}

/// Флаги `(reasoning, reasoning_budget, reasoning_format)` для cluster API.
pub fn reasoning_flags(t: Thinking) -> Option<(&'static str, i32, Option<&'static str>)> {
    match t {
        Thinking::Off => Some(("off", 0, None)),
        Thinking::On => Some(("on", -1, Some("none"))),
        Thinking::Auto => None,
    }
}

/// Готовый запрос к cluster `chat_complete`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    /// Текст запроса **без** маркеров шаблона: движок применяет шаблон чата сам
    /// (факт замера `bin/chat_probe`: +20 токенов шаблона к плоскому тексту).
    pub prompt: String,
    pub n_predict: i32,
    pub temperature: f32,
    pub thinking: Thinking,
    /// Алиас модели из запроса (для ответа) — фасад роль не различает, она одна.
    pub model: String,
    /// В запросе были `tools` (агентный вызов) → ответ собираем с `tool_calls`.
    pub tools_enabled: bool,
}

impl ChatRequest {
    /// Флаги размышлений для движка.
    pub fn reasoning(&self) -> Option<(&'static str, i32, Option<&'static str>)> {
        reasoning_flags(self.thinking)
    }
}

/// Текст одного сообщения: строка или массив частей (vision-стиль OpenAI).
fn message_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Собрать `prompt` из OpenAI-сообщений.
///
/// Правило фасада (см. модуль): маркеры шаблона не ставим, а сводим диалог в один
/// текст — system первым абзацем, предыдущие реплики как «Пользователь:/Ассистент:»,
/// последняя реплика как есть. Для пары system+user это ровно тот же вход, что
/// Python-версия отправляла в RAG (`SYSTEM\n\nВопрос…`).
pub fn build_prompt(messages: &[Value]) -> Result<String> {
    let mut system_parts: Vec<String> = Vec::new();
    let mut turns: Vec<(String, String)> = Vec::new(); // (роль, текст)
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let text = message_text(m.get("content").unwrap_or(&Value::Null));
        if text.trim().is_empty() {
            continue;
        }
        match role {
            "system" | "developer" => system_parts.push(text.trim().to_string()),
            "assistant" => turns.push(("assistant".to_string(), text)),
            _ => turns.push(("user".to_string(), text)),
        }
    }
    if turns.is_empty() {
        return Err(EngineError::Other(
            "в запросе нет ни одного пользовательского сообщения (messages[].content)".to_string(),
        ));
    }
    let mut prompt = String::new();
    if !system_parts.is_empty() {
        prompt.push_str(&system_parts.join("\n\n"));
        prompt.push_str("\n\n");
    }
    let last = turns.len() - 1;
    for (i, (role, text)) in turns.iter().enumerate() {
        if i == last && role == "user" {
            prompt.push_str(text);
        } else if role == "assistant" {
            prompt.push_str("Ассистент: ");
            prompt.push_str(text.trim());
            prompt.push('\n');
        } else {
            prompt.push_str("Пользователь: ");
            prompt.push_str(text.trim());
            prompt.push('\n');
        }
    }
    Ok(prompt)
}

/// Угловая обёртка тега без литеральных `<>` (как `think_markers` — чтобы маркеры
/// не «съедал» инструментарий).
fn angle(tag: &str) -> String {
    format!("{}{}{}", '\u{3c}', tag, '\u{3e}')
}

/// Маркеры блока вызова инструмента.
fn tool_markers() -> (String, String) {
    (angle("tool_call"), angle("/tool_call"))
}

/// Блок с описанием доступных функций (формат Qwen `<tool_call>`). Добавляется в
/// промпт ТОЛЬКО когда в запросе есть `tools` (агентный вызов): MCP/RAG `tools`
/// не передают, поэтому обычный чат остаётся прежним текстовым.
fn tools_section(tools: &[Value]) -> String {
    let open_tools = angle("tools");
    let close_tools = angle("/tools");
    let (tc_open, tc_close) = tool_markers();
    let mut s = String::new();
    s.push_str("# Tools\n\n");
    s.push_str("You may call one or more functions to assist with the user query.\n\n");
    s.push_str(&format!(
        "You are provided with function signatures within {open_tools}{close_tools} XML tags:\n{open_tools}\n"
    ));
    for t in tools {
        let f = t.get("function").unwrap_or(t);
        s.push_str(&serde_json::to_string(f).unwrap_or_default());
        s.push('\n');
    }
    s.push_str(&format!("{close_tools}\n\n"));
    s.push_str(&format!(
        "For each function call, return a json object with the function name and arguments within {tc_open}{tc_close} XML tags:\n"
    ));
    s.push_str(&format!(
        "{tc_open}\n{{\"name\": <function-name>, \"arguments\": <args-json-object>}}\n{tc_close}\n"
    ));
    s
}

/// Собрать `prompt` из сообщений, добавив описание инструментов и свернув историю
/// (в т.ч. прошлые `tool_calls` и результаты роли `tool`) в тот же плоский текст.
pub fn build_prompt_tools(messages: &[Value], tools: &[Value]) -> Result<String> {
    let (tc_open, tc_close) = tool_markers();
    let tr_open = angle("tool_response");
    let tr_close = angle("/tool_response");
    let mut system_parts: Vec<String> = Vec::new();
    let mut turns: Vec<(String, String)> = Vec::new(); // (роль, текст)
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let text = message_text(m.get("content").unwrap_or(&Value::Null));
        match role {
            "system" | "developer" => {
                if !text.trim().is_empty() {
                    system_parts.push(text.trim().to_string());
                }
            }
            "assistant" => {
                let mut t = text.trim().to_string();
                if let Some(calls) = m.get("tool_calls").and_then(|v| v.as_array()) {
                    for c in calls {
                        if let Some(f) = c.get("function") {
                            let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let args = f.get("arguments").cloned().unwrap_or_else(|| json!({}));
                            let args = match args {
                                Value::String(s) => {
                                    serde_json::from_str::<Value>(&s).unwrap_or_else(|_| json!({}))
                                }
                                other => other,
                            };
                            if !name.is_empty() {
                                t.push_str(&format!(
                                    "\n{tc_open}\n{{\"name\": \"{name}\", \"arguments\": {args}}}\n{tc_close}"
                                ));
                            }
                        }
                    }
                }
                turns.push(("assistant".to_string(), t));
            }
            "tool" => turns.push(("tool".to_string(), text.trim().to_string())),
            _ => turns.push(("user".to_string(), text)),
        }
    }
    if !turns.iter().any(|(r, _)| r == "user") {
        return Err(EngineError::Other(
            "в запросе нет ни одного пользовательского сообщения (messages[].content)".to_string(),
        ));
    }
    let mut prompt = String::new();
    if !system_parts.is_empty() {
        prompt.push_str(&system_parts.join("\n\n"));
        prompt.push_str("\n\n");
    }
    prompt.push_str(&tools_section(tools));
    prompt.push('\n');
    let last = turns.len() - 1;
    for (i, (role, text)) in turns.iter().enumerate() {
        match role.as_str() {
            "tool" => {
                prompt.push_str(&format!("\n{tr_open}\n{}\n{tr_close}\n", text.trim()));
            }
            "assistant" => {
                prompt.push_str("Ассистент: ");
                prompt.push_str(text.trim());
                prompt.push('\n');
            }
            _ => {
                if i == last {
                    prompt.push_str(text);
                } else {
                    prompt.push_str("Пользователь: ");
                    prompt.push_str(text.trim());
                    prompt.push('\n');
                }
            }
        }
    }
    Ok(prompt)
}

/// Извлечь из ответа модели блоки `<tool_call>{json}</tool_call>` (формат Qwen) и
/// вернуть `(текст без блоков, tool_calls в форме OpenAI)`.
///
/// Разбор ТЕРПИМ к «почти-JSON» локальной модели (см. [`parse_tool_payload`]):
/// раньше строгий `serde_json` на таком блоке падал, блок оставался ТЕКСТОМ, агент
/// не получал `tool_calls` и молча завершал ход — наблюдалось 03.10.2026:
/// `{"name": disk-search__ask_my_files", "arguments": {...}}` (потеряна открывающая
/// кавычка значения имени). Закрывающий тег тоже может отсутствовать — тогда блок
/// читаем до следующего `<tool_call>` или до конца ответа.
pub fn parse_tool_calls(text: &str) -> (String, Vec<Value>) {
    let (open, close) = tool_markers();
    let mut content = String::new();
    let mut calls: Vec<Value> = Vec::new();
    let mut rest = text;
    loop {
        let i = match rest.find(&open) {
            Some(i) => i,
            None => {
                content.push_str(rest);
                break;
            }
        };
        content.push_str(&rest[..i]);
        let after = &rest[i + open.len()..];
        let (inner_end, consumed) = match after.find(&close) {
            Some(j) => (j, j + close.len()),
            None => match after.find(&open) {
                Some(j) => (j, j), // модель забыла закрыть тег
                None => (after.len(), after.len()),
            },
        };
        match parse_tool_payload(after[..inner_end].trim()) {
            Some((name, args)) => calls.push(json!({
                "id": format!("call_{}", calls.len() + 1),
                "type": "function",
                "function": { "name": name, "arguments": args }
            })),
            // не разобрали — оставляем как есть (не глотаем текст молча)
            None => content.push_str(&after[..consumed]),
        }
        rest = &after[consumed..];
    }
    (content.trim().to_string(), calls)
}

/// Разобрать тело блока вызова: `{"name":…,"arguments":…}` (или `{"function":{…}}`).
///
/// Порядок попыток: строгий JSON → починка типовых сбоев → ручное извлечение имени
/// и аргументов (даже если объект целиком битый). `arguments` возвращаем СТРОКОЙ —
/// так требует форма OpenAI (`function.arguments` — JSON-текст).
fn parse_tool_payload(inner: &str) -> Option<(String, String)> {
    if let Ok(v) = serde_json::from_str::<Value>(inner) {
        if let Some(p) = payload_from_value(&v) {
            return Some(p);
        }
    }
    let fixed = repair_json(inner);
    if let Ok(v) = serde_json::from_str::<Value>(&fixed) {
        if let Some(p) = payload_from_value(&v) {
            return Some(p);
        }
    }
    let name = extract_value_after(inner, "\"name\"")?;
    if is_placeholder(&name) {
        return None;
    }
    let args = extract_object_after(inner, "\"arguments\"")
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| match v {
            Value::String(s) => s,
            other => other.to_string(),
        })
        .unwrap_or_else(|| "{}".to_string());
    Some((name, args))
}

/// `(имя, аргументы-строкой)` из разобранного значения (принимаем и обёртку
/// `{"function": {...}}`, как в OpenAI-форме).
fn payload_from_value(v: &Value) -> Option<(String, String)> {
    let f = v.get("function").unwrap_or(v);
    let name = f.get("name")?.as_str()?.trim().to_string();
    if name.is_empty() || is_placeholder(&name) {
        return None;
    }
    let args = match f.get("arguments") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => "{}".to_string(),
    };
    Some((name, args))
}

/// Модель может скопировать плейсхолдеры из промпта (`<function-name>`) — не вызов.
fn is_placeholder(name: &str) -> bool {
    name.contains('<') || name.contains('>')
}

/// Починить типовые сбои JSON от локальной модели: висячие запятые и «голые»
/// строковые значения (`"name": foo"` → `"name": "foo"`).
fn repair_json(s: &str) -> String {
    let mut out = s.to_string();
    for (a, b) in [(", }", "}"), (",}", "}"), (", ]", "]"), (",]", "]")] {
        out = out.replace(a, b);
    }
    quote_bare_values(&out)
}

/// Обернуть «голые» строковые значения в кавычки. Действуем только на `:` ВНЕ строк
/// (иначе `:` в тексте вопроса испортил бы значение) и не трогаем числа и литералы.
fn quote_bare_values(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len() + 16);
    let mut i = 0usize;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        out.push(c);
        i += 1;
        if c == b'\\' && in_str {
            if i < b.len() {
                out.push(b[i]);
                i += 1;
            }
            continue;
        }
        if c == b'"' {
            in_str = !in_str;
            continue;
        }
        if in_str || c != b':' {
            continue;
        }
        let mut j = i;
        while j < b.len() && (b[j] as char).is_whitespace() {
            j += 1;
        }
        if j >= b.len() {
            break;
        }
        let first = b[j];
        if first == b'"' || first == b'{' || first == b'[' {
            continue;
        }
        let rest = &s[j..];
        if first.is_ascii_digit()
            || first == b'-'
            || rest.starts_with("true")
            || rest.starts_with("false")
            || rest.starts_with("null")
        {
            continue;
        }
        let end = rest.find([',', '}', ']', '\n']).unwrap_or(rest.len());
        let mut val = rest[..end].trim();
        if let Some(v) = val.strip_suffix('"') {
            val = v.trim_end();
        }
        if val.is_empty() {
            continue;
        }
        out.extend_from_slice(&b[i..j]);
        out.push(b'"');
        out.extend_from_slice(val.as_bytes());
        out.push(b'"');
        i = j + end;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Значение-слово после ключа (`"name": disk-search__ask_my_files"` → `disk-search__ask_my_files`).
fn extract_value_after(s: &str, key: &str) -> Option<String> {
    let at = s.find(key)? + key.len();
    let after = s[at..].trim_start().strip_prefix(':')?.trim_start();
    let val: String = after
        .trim_start_matches('"')
        .chars()
        .take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/'))
        .collect();
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

/// Первый JSON-объект после ключа (баланс скобок с учётом строк).
fn extract_object_after(s: &str, key: &str) -> Option<String> {
    let at = s.find(key)? + key.len();
    let after = s[at..].trim_start().strip_prefix(':')?.trim_start();
    let start = after.find('{')?;
    let b = after.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(after[start..=i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Собрать запрос чата из тела `/v1/chat/completions`.
///
/// `default_thinking` — из конфига (`chat.thinking`), `default_max_tokens` — из
/// `chat.max_tokens` (в Python-версии 600: длинные ответы не влезают в таймаут клиента),
/// `default_temperature` — из `chat.temperature`.
pub fn build_chat_request(
    body: &Value,
    default_thinking: Thinking,
    default_max_tokens: i32,
    default_temperature: f32,
) -> Result<ChatRequest> {
    let messages = body
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or_else(|| EngineError::Other("в теле нет массива messages".to_string()))?;
    let prompt = build_prompt(messages)?;
    // `tools` есть → агентный вызов: собираем промпт с описанием функций и ждём
    // `<tool_call>`. MCP/RAG `tools` не передают — обычный текстовый промпт.
    let tools = body
        .get("tools")
        .and_then(|t| t.as_array())
        .filter(|a| !a.is_empty());
    let (prompt, tools_enabled) = match tools {
        Some(tools) => (build_prompt_tools(messages, tools)?, true),
        None => (prompt, false),
    };
    let n_predict = body
        .get("max_tokens")
        .or_else(|| body.get("n_predict"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(default_max_tokens)
        .max(1);
    let temperature = body
        .get("temperature")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(default_temperature);
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("chat")
        .to_string();
    let thinking = thinking_for(&model, body, default_thinking);
    Ok(ChatRequest {
        prompt,
        n_predict,
        temperature,
        thinking,
        model,
        tools_enabled,
    })
}

/// Токены для блока `usage` (из метрик движка).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
}

impl Usage {
    pub fn total(&self) -> i32 {
        self.prompt_tokens + self.completion_tokens
    }

    pub fn to_json(&self) -> Value {
        json!({
            "prompt_tokens": self.prompt_tokens,
            "completion_tokens": self.completion_tokens,
            "total_tokens": self.total(),
        })
    }
}

/// `GET /health` — как у `llama-server`: `{"status":"ok"}` (Python-версия проверяет
/// только 2xx-код, но формат полезен и UI).
pub fn health_json() -> Value {
    json!({ "status": "ok" })
}

/// `GET /props` — то, по чему Python-версия (`hds/llama_server.py::probe`) узнаёт
/// «свой» инстанс: `total_slots >= 1`, `model_path` (сверяется с ожидаемым GGUF) и
/// `n_ctx`/`default_generation_settings.n_ctx` (для `ctx_actual` в UI).
pub fn props_json(role: &str, model_path: &str, n_ctx: i32, n_parallel: i32, alias: &str) -> Value {
    json!({
        "role": role,
        "model_path": model_path,
        "model": model_path,
        "alias": alias,
        "total_slots": n_parallel.max(1),
        "n_ctx": n_ctx,
        "default_generation_settings": { "n_ctx": n_ctx, "n_parallel": n_parallel.max(1) },
        "facade": "hds-llm-host",
    })
}

/// `GET /v1/models` — алиасы ролей (`chat`, `chat-think`, `embedding`, `rerank`).
pub fn models_json(aliases: &[String], created: i64) -> Value {
    json!({
        "object": "list",
        "data": aliases.iter().map(|a| json!({
            "id": a, "object": "model", "created": created, "owned_by": "hds",
        })).collect::<Vec<_>>(),
    })
}

/// Ошибка в форме OpenAI (её же отдаёт `llama-server`).
pub fn error_json(message: &str, kind: &str) -> Value {
    json!({ "error": { "message": message, "type": kind, "code": Value::Null } })
}

/// 404 с подсказкой, какие пути поддержаны.
pub fn not_found_json(path: &str) -> Value {
    error_json(
        &format!(
            "неизвестный путь {path}; поддержаны /health, /props, /v1/models, \
             /v1/chat/completions, /v1/embeddings, /v1/rerank"
        ),
        "invalid_request_error",
    )
}

/// Ответ `/v1/chat/completions`.
///
/// * `thinking = off` → из `content` вырезаются inline-размышления (`strip_think`,
///   как в Python-версии: при `auto` сервер иногда оставляет их в тексте);
/// * `thinking = on` → текст отдаём как есть (размышления видны — это и есть
///   `chat-think`), в `reasoning_content` дублируем их отдельно, чтобы клиенты,
///   умеющие показывать размышления, могли это делать.
pub fn chat_response_json(model: &str, text: &str, thinking: Thinking, usage: Usage) -> Value {
    let (reasoning, content) = split_reasoning(text);
    let mut message = json!({ "role": "assistant", "content": content });
    if thinking == Thinking::On {
        if let Some(r) = reasoning {
            if !r.is_empty() {
                message["reasoning_content"] = json!(r);
            }
        }
    }
    json!({
        "id": "chatcmpl-hds",
        "object": "chat.completion",
        "created": unix_now(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": "stop",
        }],
        "usage": usage.to_json(),
    })
}

/// Ответ `/v1/chat/completions` с поддержкой `tool_calls` (агентный вызов).
///
/// Если в запросе были `tools` и модель выдала блоки `<tool_call>`, возвращаем
/// `message.tool_calls` и `finish_reason = "tool_calls"` (размышления из `content`
/// вырезаются). Иначе — обычный ответ (`chat_response_json`).
pub fn chat_response_json_tools(
    model: &str,
    text: &str,
    thinking: Thinking,
    usage: Usage,
    tools_enabled: bool,
) -> Value {
    if !tools_enabled {
        return chat_response_json(model, text, thinking, usage);
    }
    let (content, calls) = parse_tool_calls(&strip_think(text));
    if calls.is_empty() {
        return chat_response_json(model, text, thinking, usage);
    }
    let content = if content.is_empty() {
        Value::Null
    } else {
        json!(content)
    };
    let message = json!({ "role": "assistant", "content": content, "tool_calls": calls });
    json!({
        "id": "chatcmpl-hds",
        "object": "chat.completion",
        "created": unix_now(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": "tool_calls",
        }],
        "usage": usage.to_json(),
    })
}

/// SSE-поток `/v1/chat/completions` в формате OpenAI.
///
/// Движок отдаёт ответ целиком (стриминга в C-API нет), поэтому «стримим» его
/// несколькими чанками: роль+контент → `tool_calls` (если есть) → финальный
/// `finish_reason` → `usage` → `[DONE]`. Клиенту (Cline и др.) важен формат SSE,
/// а не покадровая генерация.
pub fn chat_sse(
    model: &str,
    text: &str,
    thinking: Thinking,
    usage: Usage,
    tools_enabled: bool,
) -> String {
    fn chunk(out: &mut String, model: &str, delta: Value, finish: Value) {
        let v = json!({
            "id": "chatcmpl-hds",
            "object": "chat.completion.chunk",
            "created": unix_now(),
            "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        });
        out.push_str("data: ");
        out.push_str(&v.to_string());
        out.push_str("\n\n");
    }

    let (reasoning, answer) = split_reasoning(text);
    let (content, calls) = if tools_enabled {
        let (c, calls) = parse_tool_calls(&answer);
        if calls.is_empty() {
            (answer, Vec::new())
        } else {
            (c, calls)
        }
    } else {
        (answer, Vec::new())
    };

    let mut out = String::new();
    if thinking == Thinking::On {
        if let Some(r) = &reasoning {
            if !r.is_empty() {
                chunk(
                    &mut out,
                    model,
                    json!({ "reasoning_content": r }),
                    Value::Null,
                );
            }
        }
    }
    chunk(
        &mut out,
        model,
        json!({ "role": "assistant", "content": content }),
        Value::Null,
    );
    if !calls.is_empty() {
        let tc: Vec<Value> = calls
            .iter()
            .enumerate()
            .map(|(i, c)| {
                json!({
                    "index": i,
                    "id": c.get("id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": c["function"]["name"].clone(),
                        "arguments": c["function"]["arguments"].clone(),
                    }
                })
            })
            .collect();
        chunk(&mut out, model, json!({ "tool_calls": tc }), Value::Null);
    }
    let finish = if calls.is_empty() {
        "stop"
    } else {
        "tool_calls"
    };
    chunk(&mut out, model, json!({}), json!(finish));
    // usage-чанк (клиенты с stream_options.include_usage его читают)
    let u = json!({
        "id": "chatcmpl-hds",
        "object": "chat.completion.chunk",
        "created": unix_now(),
        "model": model,
        "choices": [],
        "usage": usage.to_json(),
    });
    out.push_str("data: ");
    out.push_str(&u.to_string());
    out.push_str("\n\n");
    out.push_str("data: [DONE]\n\n");
    out
}

/// Проверить и вернуть JSON эмбеддингов от движка.
///
/// Контракт (как у `llama-server`, и что читает `hds/embedder.py`): `data[i].embedding`
/// — массив чисел; при отсутствии — понятная ошибка вместо тихого «пустого» ответа.
pub fn validate_embeddings(engine_json: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(engine_json).map_err(|e| {
        EngineError::Other(format!("ответ эмбеддингов не JSON ({e}): {engine_json}"))
    })?;
    let data = v.get("data").and_then(|d| d.as_array()).ok_or_else(|| {
        EngineError::Other(format!(
            "в ответе эмбеддингов нет массива data: {engine_json}"
        ))
    })?;
    let with_vec = data
        .iter()
        .filter(|d| d.get("embedding").map(|e| e.is_array()).unwrap_or(false))
        .count();
    if with_vec == 0 {
        return Err(EngineError::Other(format!(
            "в ответе эмбеддингов нет ни одного data[].embedding: {engine_json}"
        )));
    }
    Ok(v)
}

/// Проверить и вернуть JSON реранка (`results[]`, как у `llama-server`).
pub fn validate_rerank(engine_json: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(engine_json)
        .map_err(|e| EngineError::Other(format!("ответ реранка не JSON ({e}): {engine_json}")))?;
    if v.get("results").and_then(|r| r.as_array()).is_none() {
        return Err(EngineError::Other(format!(
            "в ответе реранка нет массива results: {engine_json}"
        )));
    }
    Ok(v)
}

/// Секунды от Unix epoch (для `created` в ответах).
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Источник инференса для фасада: реальный кластер (`llm-host`) или подделка в тестах.
///
/// Фасад знает про роли, а не про инстансы: он лишь превращает HTTP-запрос в вызов
/// роли и обратно — это и позволяет проверять формат ответов без движка.
pub trait Backend: Send + Sync {
    /// Чат: `(текст, usage)`.
    fn chat(&self, req: &ChatRequest) -> Result<(String, Usage)>;
    /// Эмбеддинги: JSON движка (фасад проверяет `data[].embedding`).
    fn embeddings(&self, body_json: &str) -> Result<String>;
    /// Реранк: JSON движка (фасад проверяет `results[]`).
    fn rerank(&self, body_json: &str) -> Result<String>;
    /// Факты об инстансе роли для `/props` (`None` — роль не поднята).
    fn props(&self, role: &str) -> Option<Value>;

    // --- A6: внутренний API управления (`/internal/*`) ---
    //
    // Методы с реализацией по умолчанию: подделки в тестах и будущие чужие
    // бэкенды не обязаны их поддерживать — тогда фасад честно отвечает 501.

    /// Готовый `StatusReport` (роли/бюджет/пауза/прогноз) + человеческие строки.
    fn internal_status(&self) -> Result<Value> {
        Err(crate::error::EngineError::Other(
            "внутренний status недоступен: backend не умеет отчёт (режим без кластера)".to_string(),
        ))
    }

    /// Загрузить роль (`/internal/load`, тело `{"role": "chat"}`).
    fn internal_load(&self, role: &str) -> Result<Value> {
        Err(crate::error::EngineError::Other(format!(
            "внутренний load недоступен для роли '{role}'"
        )))
    }

    /// Выгрузить роль (`/internal/unload`).
    fn internal_unload(&self, role: &str) -> Result<Value> {
        Err(crate::error::EngineError::Other(format!(
            "внутренний unload недоступен для роли '{role}'"
        )))
    }

    /// Устройства движка (`/internal/devices`).
    fn internal_devices(&self) -> Result<Value> {
        Err(crate::error::EngineError::Other(
            "внутренний devices недоступен: движок не загружен".to_string(),
        ))
    }

    /// Попросить хост завершиться (`/internal/stop`).
    fn internal_stop(&self) -> Result<Value> {
        Err(crate::error::EngineError::Other(
            "внутренний stop недоступен".to_string(),
        ))
    }

    /// W3: транскрибация аудио/видео через владельца GPU (`/internal/transcribe`).
    ///
    /// Тело: `{"path": "...", "mode": "subtitle|speech", "custom": "4.5", "gpu": 0}`.
    /// Ответ: `{"segments":[{"text,t_start,t_end}], "stats": …}`.
    fn internal_transcribe(&self, body: &Value) -> Result<Value> {
        let _ = body;
        Err(crate::error::EngineError::Other(
            "внутренняя транскрибация недоступна: владелец не умеет whisper".to_string(),
        ))
    }

    /// Базовый URL внешнего владельца для режима `llm_server.mode: facade`
    /// (`http://host:port/v1`). `None` — режим `embedded` (работаем через кластер).
    ///
    /// Если URL задан, фасад **проксирует** запрос роли как есть: это позволяет
    /// держать на боевых портах OpenAI-совместимый поверх без собственных
    /// инстансов (GPU принадлежит другому процессу).
    fn upstream(&self, role: &str) -> Option<String> {
        let _ = role;
        None
    }
}

/// Настройки фасада (из конфига: порты ролей, `chat.thinking`, `chat.max_tokens`,
/// `chat.temperature`, `llm_server.host`).
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    /// Порты по ролям (`chat`/`embedding`/`rerank`).
    pub ports: Vec<(String, u16)>,
    /// Алиасы для `/v1/models` (например `chat`, `chat-think`).
    pub aliases: Vec<String>,
    pub thinking: Thinking,
    pub max_tokens: i32,
    pub temperature: f32,
    /// Обслуживать `/internal/*` (CLI `llm-host`). `false` — отвечаем `403`:
    /// эти маршруты управляют ролями и остановкой процесса, поэтому их нельзя
    /// выставлять наружу (фасад слушает loopback, но лишняя страховка бесплатна).
    pub internal: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            host: "127.0.0.1".to_string(),
            ports: PORTS.iter().map(|(r, p)| (r.to_string(), *p)).collect(),
            aliases: vec![
                "chat".to_string(),
                "chat-think".to_string(),
                "embedding".to_string(),
                "rerank".to_string(),
            ],
            thinking: Thinking::Off,
            max_tokens: 600,
            temperature: 0.2,
            internal: true,
        }
    }
}

impl ServerConfig {
    pub fn port_of(&self, role: &str) -> Option<u16> {
        self.ports.iter().find(|(r, _)| r == role).map(|(_, p)| *p)
    }

    /// Алиасы роли (у чата их два: `chat` и `chat-think`).
    pub fn aliases_of(&self, role: &str) -> Vec<String> {
        self.aliases
            .iter()
            .filter(|a| a.as_str() == role || a.starts_with(&format!("{role}-")))
            .cloned()
            .collect()
    }
}

/// Роль для `/props`: первая поднятая (обычно чат).
fn props_role(cfg: &ServerConfig, backend: &dyn Backend) -> Option<(String, Value)> {
    for (role, _) in &cfg.ports {
        if let Some(props) = backend.props(role) {
            return Some((role.clone(), props));
        }
    }
    None
}

/// Внутренний API управления (`/internal/*`) — для CLI `llm-host` (A6).
///
/// Отдельная ветка, потому что это не инференс: ответы — машинные отчёты
/// (готовый `StatusReport`, список устройств, результат load/unload), а не
/// OpenAI-совместимые формы, и доступны они только при `cfg.internal`.
fn handle_internal(
    req: &crate::http::Request,
    cfg: &ServerConfig,
    backend: &dyn Backend,
    r: Route,
) -> crate::http::Response {
    use crate::http::Response;
    if !cfg.internal {
        return Response::error(
            403,
            "внутренний API выключен (ServerConfig.internal = false): он управляет \
             ролями и остановкой llm-host",
            "forbidden",
        );
    }
    match r {
        Route::InternalStatus => match backend.internal_status() {
            Ok(v) => Response::ok(v),
            Err(e) => Response::error(501, &e.to_string(), "not_implemented"),
        },
        Route::InternalDevices => match backend.internal_devices() {
            Ok(v) => Response::ok(v),
            Err(e) => Response::error(501, &e.to_string(), "not_implemented"),
        },
        Route::InternalStop => match backend.internal_stop() {
            Ok(v) => Response::ok(v),
            Err(e) => Response::error(501, &e.to_string(), "not_implemented"),
        },
        Route::InternalLoad | Route::InternalUnload => {
            let role = match internal_role(req) {
                Ok(r) => r,
                Err(e) => return Response::error(400, &e.to_string(), "invalid_request_error"),
            };
            let res = if r == Route::InternalLoad {
                backend.internal_load(&role)
            } else {
                backend.internal_unload(&role)
            };
            match res {
                Ok(v) => Response::ok(v),
                // роль не найдена / состояние не позволяет — это не «не реализовано»
                Err(e) => Response::error(409, &e.to_string(), "server_error"),
            }
        }
        Route::InternalTranscribe => {
            let body = match crate::http::parse_body(req) {
                Ok(v) => v,
                Err(e) => return Response::error(400, &e.to_string(), "invalid_request_error"),
            };
            match backend.internal_transcribe(&body) {
                Ok(v) => Response::ok(v),
                Err(e) => Response::error(501, &e.to_string(), "not_implemented"),
            }
        }
        _ => Response::error(
            400,
            "маршрут не является внутренним",
            "invalid_request_error",
        ),
    }
}

/// Роль из тела внутреннего запроса (`{"role": "chat"}`); пустое тело — `chat`
/// (так `llm-host load` без аргумента работает с чатом, как `llama_server status`).
fn internal_role(req: &crate::http::Request) -> Result<String> {
    if req.body.trim().is_empty() {
        return Ok("chat".to_string());
    }
    let v = crate::http::parse_body(req)?;
    v.get("role")
        .and_then(|r| r.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| crate::error::EngineError::Other("в теле запроса нет поля role".to_string()))
}

/// Ответ внешнего владельца **как есть** (режим `llm_server.mode: facade`).
///
/// Ничего не пересобираем: апстрим — уже OpenAI-совместимый сервер, а любая
/// нормализация потеряла бы его поля (`reasoning_content`, `usage` и пр.).
fn proxy(req: &crate::http::Request, base: &str, r: Route) -> crate::http::Response {
    use crate::http::Response;
    let Some(path) = r.upstream_path() else {
        return Response::error(400, "маршрут не проксируется", "invalid_request_error");
    };
    let url = format!("{}/{}", base.trim_end_matches('/'), path);
    // чат отвечает минутами (RAG-контекст), остальные роли — заметно быстрее
    let timeout = if r == Route::Chat {
        std::time::Duration::from_secs(600)
    } else {
        std::time::Duration::from_secs(120)
    };
    match crate::http::client_json(&req.method, &url, Some(&req.body), timeout) {
        Ok((status, json)) if (200..300).contains(&status) => Response {
            status: 200,
            json,
            sse: None,
        },
        Ok((status, json)) => {
            let msg = json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("upstream {url} ответил статусом {status}"));
            Response::error(
                if status == 404 { 502 } else { status },
                &msg,
                "server_error",
            )
        }
        Err(e) => Response::error(
            503,
            &format!("upstream {url} недоступен: {e}"),
            "server_error",
        ),
    }
}

/// Обработать HTTP-запрос фасада (без сокетов — тестируется напрямую с подделкой backend).
pub fn handle(
    req: &crate::http::Request,
    cfg: &ServerConfig,
    backend: &dyn Backend,
) -> crate::http::Response {
    use crate::http::Response;
    let r = route(&req.method, &req.path);
    if r.is_internal() {
        return handle_internal(req, cfg, backend, r);
    }
    // режим `llm_server.mode: facade`: роли держит внешний процесс — проксируем как есть
    if let Some(base) = r.role().and_then(|role| backend.upstream(role)) {
        return proxy(req, &base, r);
    }
    match r {
        Route::NotFound => Response {
            status: 404,
            json: not_found_json(&req.path),
            sse: None,
        },
        // внутренние маршруты разобраны выше (`handle_internal`); эта ветка —
        // страховка на случай, если `Route` расширят и забудут обработку
        Route::InternalStatus
        | Route::InternalLoad
        | Route::InternalUnload
        | Route::InternalDevices
        | Route::InternalStop
        | Route::InternalTranscribe => Response::error(
            400,
            "внутренний маршрут не обслуживается здесь",
            "invalid_request_error",
        ),
        Route::Health => Response::ok(health_json()),
        Route::Models => Response::ok(models_json(&cfg.aliases, unix_now() as i64)),
        Route::Props => match props_role(cfg, backend) {
            Some((role, props)) => {
                let mut body = props_json(
                    &role,
                    props
                        .get("model_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                    props.get("n_ctx").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    props
                        .get("total_slots")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(1) as i32,
                    &cfg.aliases_of(&role)
                        .first()
                        .cloned()
                        .unwrap_or_else(|| role.clone()),
                );
                // L1 (`W4_REPORT.md` §15): наблюдаемость роли сквозь штатный `/props` —
                // состояние инстанса и занятость движка («кто держит и сколько»), если
                // бэкенд их знает (резидентный `llm-host` знает; режим `facade` — нет).
                if let Value::Object(map) = &mut body {
                    for key in ["state", "busy"] {
                        if let Some(v) = props.get(key) {
                            map.insert(key.to_string(), v.clone());
                        }
                    }
                }
                Response::ok(body)
            }
            None => Response::error(
                503,
                "ни один инстанс фасада не поднят (инстансы стартуют вместе с llm-host)",
                "server_error",
            ),
        },
        Route::Chat => {
            let body = match crate::http::parse_body(req) {
                Ok(v) => v,
                Err(e) => return Response::error(400, &e.to_string(), "invalid_request_error"),
            };
            let want_stream = body
                .get("stream")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let cr = match build_chat_request(&body, cfg.thinking, cfg.max_tokens, cfg.temperature)
            {
                Ok(r) => r,
                Err(e) => return Response::error(400, &e.to_string(), "invalid_request_error"),
            };
            match backend.chat(&cr) {
                Ok((text, usage)) => {
                    if want_stream {
                        Response::sse(chat_sse(
                            &cr.model,
                            &text,
                            cr.thinking,
                            usage,
                            cr.tools_enabled,
                        ))
                    } else {
                        Response::ok(chat_response_json_tools(
                            &cr.model,
                            &text,
                            cr.thinking,
                            usage,
                            cr.tools_enabled,
                        ))
                    }
                }
                Err(e) => Response::error(503, &e.to_string(), "server_error"),
            }
        }
        Route::Embeddings => {
            let body = match crate::http::parse_body(req) {
                Ok(v) => v,
                Err(e) => return Response::error(400, &e.to_string(), "invalid_request_error"),
            };
            match backend.embeddings(&body.to_string()) {
                Ok(engine_json) => match validate_embeddings(&engine_json) {
                    Ok(v) => Response::ok(v),
                    Err(e) => Response::error(502, &e.to_string(), "server_error"),
                },
                Err(e) => Response::error(503, &e.to_string(), "server_error"),
            }
        }
        Route::Rerank => {
            let body = match crate::http::parse_body(req) {
                Ok(v) => v,
                Err(e) => return Response::error(400, &e.to_string(), "invalid_request_error"),
            };
            match backend.rerank(&body.to_string()) {
                Ok(engine_json) => match validate_rerank(&engine_json) {
                    Ok(v) => Response::ok(v),
                    Err(e) => Response::error(502, &e.to_string(), "server_error"),
                },
                Err(e) => Response::error(503, &e.to_string(), "server_error"),
            }
        }
    }
}

/// Поднять фасад на портах ролей: поток на порт, общий обработчик.
///
/// `stop` создаёт **владелец** (резидентный `llm-host`): флаг нужен не только
/// `Host::stop`, но и маршруту `/internal/stop` — CLI просит хост завершиться
/// через тот же фасад, без сигналов и отдельного канала.
pub fn serve(
    cfg: &ServerConfig,
    backend: std::sync::Arc<dyn Backend>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<Vec<std::thread::JoinHandle<()>>> {
    use std::sync::Arc;
    let mut handles = Vec::new();
    // порты — своя копия: поток должен владеть и конфигом, и строкой роли
    for (role, port) in cfg.ports.clone() {
        let addr = format!("{}:{}", cfg.host, port);
        let listener = std::net::TcpListener::bind(&addr).map_err(|e| {
            EngineError::Other(format!(
                "порт {port} ({role}) не занялся: {e} — возможно, там ещё слушает llama-server \
                 (остановите старые роли или смените порт)"
            ))
        })?;
        let cfg_thread = ServerConfig {
            ports: vec![(role.clone(), port)],
            ..ServerConfig::clone(cfg)
        };
        let backend_thread = Arc::clone(&backend);
        let stop_thread = Arc::clone(&stop);
        println!("[facade] {role}: http://{addr} (эндпоинты /v1/... как у llama-server)");
        handles.push(std::thread::spawn(move || {
            let handler =
                move |req: &crate::http::Request| handle(req, &cfg_thread, &*backend_thread);
            if let Err(e) = crate::http::serve(listener, stop_thread, handler) {
                eprintln!("[facade] сервер роли '{role}' остановлен: {e}");
            }
        }));
    }
    Ok(handles)
}
