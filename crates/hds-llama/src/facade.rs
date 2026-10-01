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

/// Порт `hds/rag.py::_strip_think`: убрать inline-размышления из `content`
/// (бывает при `thinking = auto`, когда сервер не вынес их в `reasoning_content`).
pub fn strip_think(text: &str) -> String {
    let (open, close) = think_markers();
    let mut out = String::new();
    let mut rest = text;
    loop {
        match rest.find(&open) {
            Some(i) => {
                out.push_str(&rest[..i]);
                let after = &rest[i + open.len()..];
                match after.find(&close) {
                    // блок без закрытия — режем до конца (модель не дописала)
                    None => return out.trim().to_string(),
                    Some(j) => rest = &after[j + close.len()..],
                }
            }
            None => {
                // одинокий закрывающий маркер: режем от него до конца (как `_THINK_RX`)
                match rest.find(&close) {
                    Some(j) => out.push_str(&rest[..j]),
                    None => out.push_str(rest),
                }
                return out.trim().to_string();
            }
        }
    }
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
    if body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(EngineError::Other(
            "stream=true не поддерживается фасадом: cluster chat API отдаёт ответ целиком, \
             клиент получит обычный JSON"
                .to_string(),
        ));
    }
    let messages = body
        .get("messages")
        .and_then(|m| m.as_array())
        .ok_or_else(|| EngineError::Other("в теле нет массива messages".to_string()))?;
    let prompt = build_prompt(messages)?;
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
    let content = match thinking {
        Thinking::Off => strip_think(text),
        _ => text.trim().to_string(),
    };
    let mut message = json!({ "role": "assistant", "content": content });
    if thinking == Thinking::On {
        message["reasoning_content"] = json!(text.trim());
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

/// Проверить и вернуть JSON эмбеддингов от движка.
///
/// Контракт (как у `llama-server`, и что читает `hds/embedder.py`): `data[i].embedding`
/// — массив чисел; при отсутствии — понятная ошибка вместо тихого «пустого» ответа.
pub fn validate_embeddings(engine_json: &str) -> Result<Value> {
    let v: Value = serde_json::from_str(engine_json).map_err(|e| {
        EngineError::Other(format!("ответ эмбеддингов не JSON ({e}): {engine_json}"))
    })?;
    let data = v.get("data").and_then(|d| d.as_array()).ok_or_else(|| {
        EngineError::Other(format!("в ответе эмбеддингов нет массива data: {engine_json}"))
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
    let v: Value = serde_json::from_str(engine_json).map_err(|e| {
        EngineError::Other(format!("ответ реранка не JSON ({e}): {engine_json}"))
    })?;
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
        _ => Response::error(400, "маршрут не является внутренним", "invalid_request_error"),
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
        .ok_or_else(|| {
            crate::error::EngineError::Other("в теле запроса нет поля role".to_string())
        })
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
        Ok((status, json)) if (200..300).contains(&status) => Response { status: 200, json },
        Ok((status, json)) => {
            let msg = json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("upstream {url} ответил статусом {status}"));
            Response::error(if status == 404 { 502 } else { status }, &msg, "server_error")
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
            Some((role, props)) => Response::ok(props_json(
                &role,
                props.get("model_path").and_then(|v| v.as_str()).unwrap_or(""),
                props.get("n_ctx").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                props.get("total_slots").and_then(|v| v.as_i64()).unwrap_or(1) as i32,
                &cfg.aliases_of(&role)
                    .first()
                    .cloned()
                    .unwrap_or_else(|| role.clone()),
            )),
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
            let cr =
                match build_chat_request(&body, cfg.thinking, cfg.max_tokens, cfg.temperature) {
                    Ok(r) => r,
                    Err(e) => return Response::error(400, &e.to_string(), "invalid_request_error"),
                };
            match backend.chat(&cr) {
                Ok((text, usage)) => {
                    Response::ok(chat_response_json(&cr.model, &text, cr.thinking, usage))
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

