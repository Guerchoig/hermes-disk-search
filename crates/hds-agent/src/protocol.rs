//! NDJSON-протокол §4.3 между Rust-стороной (`hds`) и Node-воркером
//! `sidecar/hds_agent/dist/worker.mjs`.
//!
//! Канал двунаправленный и живёт поверх stdio: одна строка — один JSON-объект.
//! Идентификаторы строковые: у команд Rust — `"1"`, `"2"`, …; у встречных
//! запросов воркера (`host_call`) — с префиксом `h`, чтобы Rust не спутал их
//! со своими (TR-4.3).
//!
//! Вниз (Rust → воркер): `hello`, `start`, `send`, `abort`, `stop`,
//! `shutdown`, `host_result`.
//! Вверх (воркер → Rust): `ready`, `event`, `host_call`, `done`, `error`.
//!
//! Значения PII в протокол не попадают: `start` несёт только ключ провайдера
//! (он не пишется на диск), `event` — уже деанонимизированный Rust-стороной
//! поток, `host_call` — только тела, которые Rust обязан прогнать через
//! словарный фильтр до отправки наружу.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Версия протокола §4.3. Меняется только несовместимо.
pub const PROTOCOL_VERSION: u32 = 1;

/// Провайдер и модель сессии (TR-3.3). Ключ приходит из
/// `data/agent/secrets.json` и живёт только в памяти процесса.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// `openrouter`, `anthropic` или `openai-compatible`.
    pub kind: String,
    /// База API. Для OpenAI-совместимых — с `/v1`, для Anthropic — без.
    pub base_url: String,
    /// Идентификатор модели.
    pub model: String,
    /// Ключ провайдера. В журналы и события не попадает.
    pub api_key: String,
}

impl Provider {
    /// Признак Anthropic-совместимого API (другой путь и заголовки).
    pub fn is_anthropic(&self) -> bool {
        self.kind.eq_ignore_ascii_case("anthropic")
    }
}

/// Сообщение вниз (Rust → воркер).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Down {
    /// Рукопожатие: версия протокола.
    Hello { id: String, protocol: u32 },
    /// Открыть сессию (или продолжить по `session_id`).
    Start {
        id: String,
        project: String,
        provider: Provider,
        /// История прошлой сессии (`initialMessages`, TR-3.9).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        history: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default = "default_max_iterations")]
        max_iterations: u32,
        #[serde(default)]
        subagents: bool,
    },
    /// Ход пользователя.
    Send { id: String, text: String },
    /// Прервать текущий ход.
    Abort { id: String },
    /// Мягкая остановка (сессия сохраняется).
    Stop { id: String },
    /// Завершить процесс.
    Shutdown { id: String },
    /// Ответ на встречный `host_call`.
    HostResult {
        id: String,
        ok: bool,
        #[serde(default)]
        result: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

fn default_max_iterations() -> u32 {
    12
}

impl Down {
    /// Идентификатор команды.
    pub fn id(&self) -> &str {
        match self {
            Down::Hello { id, .. }
            | Down::Start { id, .. }
            | Down::Send { id, .. }
            | Down::Abort { id }
            | Down::Stop { id }
            | Down::Shutdown { id }
            | Down::HostResult { id, .. } => id,
        }
    }

    /// Тип сообщения (для журналов и ошибок).
    pub fn kind(&self) -> &'static str {
        match self {
            Down::Hello { .. } => "hello",
            Down::Start { .. } => "start",
            Down::Send { .. } => "send",
            Down::Abort { .. } => "abort",
            Down::Stop { .. } => "stop",
            Down::Shutdown { .. } => "shutdown",
            Down::HostResult { .. } => "host_result",
        }
    }
}

/// Сообщение вверх (воркер → Rust).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Up {
    /// Ответ на `hello`.
    Ready {
        id: String,
        protocol: u32,
        #[serde(default)]
        node: String,
        #[serde(default)]
        sdk: String,
        /// Инструменты, доступные модели (имена `<server>__<tool>`).
        #[serde(default)]
        tools: Vec<String>,
    },
    /// Событие сессии. Структура — свободный JSON: Rust деанонимизирует
    /// значения рекурсивно и добавляет `raw` рядом с `text` (TR-5.3).
    Event { id: String, event: Value },
    /// Встречный запрос к Rust: `egress_check`, `office_*`, `ask_user`.
    HostCall {
        id: String,
        call: String,
        #[serde(default)]
        payload: Value,
    },
    /// Команда выполнена (или завершилась ошибкой).
    Done {
        id: String,
        ok: bool,
        #[serde(default)]
        result: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Непротокольная ошибка воркера (stderr-подобная строка).
    Error {
        #[serde(default)]
        message: String,
        #[serde(default)]
        stack: Option<String>,
    },
}

impl Up {
    /// Тип сообщения.
    pub fn kind(&self) -> &'static str {
        match self {
            Up::Ready { .. } => "ready",
            Up::Event { .. } => "event",
            Up::HostCall { .. } => "host_call",
            Up::Done { .. } => "done",
            Up::Error { .. } => "error",
        }
    }
}

/// Точки применения словарного фильтра (TR-6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPoint {
    /// Тело запроса к провайдеру.
    Model,
    /// Аргументы инструмента MCP перед вызовом.
    McpOut,
    /// Результат инструмента MCP после вызова.
    McpIn,
}

impl EgressPoint {
    pub fn as_str(self) -> &'static str {
        match self {
            EgressPoint::Model => "model",
            EgressPoint::McpOut => "mcp_out",
            EgressPoint::McpIn => "mcp_in",
        }
    }

    pub fn parse(s: &str) -> Option<EgressPoint> {
        match s.trim().to_ascii_lowercase().as_str() {
            "model" => Some(EgressPoint::Model),
            "mcp_out" => Some(EgressPoint::McpOut),
            "mcp_in" => Some(EgressPoint::McpIn),
            _ => None,
        }
    }
}

/// Разобранный `host_call` воркера.
#[derive(Debug, Clone, PartialEq)]
pub enum HostCall {
    /// Прогнать тело через фильтр. `payload` — `{"kind": "...", "body": ...}`.
    EgressCheck { point: EgressPoint, body: Value },
    /// Прочитать документ Office.
    OfficeRead { path: String },
    /// Изменить документ Office.
    OfficeEdit { path: String, ops: Vec<Value> },
    /// Создать документ Office.
    OfficeCreate {
        path: String,
        kind: String,
        content: Value,
    },
    /// Спросить пользователя (единственный канал вопросов, TR-3.2).
    AskUser { question: String },
    /// Неизвестный вызов — Rust отвечает ошибкой, воркер решает, что делать.
    Unknown { name: String, payload: Value },
}

impl HostCall {
    /// Разобрать сообщение `Up::HostCall`.
    pub fn parse(name: &str, payload: &Value) -> HostCall {
        match name {
            "egress_check" => {
                let point = payload
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .and_then(EgressPoint::parse)
                    .unwrap_or(EgressPoint::Model);
                let body = payload.get("body").cloned().unwrap_or(Value::Null);
                HostCall::EgressCheck { point, body }
            }
            "office_read" => HostCall::OfficeRead {
                path: str_field(payload, "path"),
            },
            "office_edit" => HostCall::OfficeEdit {
                path: str_field(payload, "path"),
                ops: payload
                    .get("ops")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default(),
            },
            "office_create" => HostCall::OfficeCreate {
                path: str_field(payload, "path"),
                kind: str_field(payload, "kind"),
                content: payload.get("content").cloned().unwrap_or(Value::Null),
            },
            "ask_user" => HostCall::AskUser {
                question: str_field(payload, "question"),
            },
            other => HostCall::Unknown {
                name: other.to_string(),
                payload: payload.clone(),
            },
        }
    }
}

fn str_field(payload: &Value, name: &str) -> String {
    payload
        .get(name)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Разобрать строку NDJSON. Пустые строки и строки, не являющиеся объектом,
/// возвращают `None` (воркер может писать диагностику в stdout).
pub fn parse_line(line: &str) -> Option<Up> {
    let text = line.trim();
    if text.is_empty() || !text.starts_with('{') {
        return None;
    }
    serde_json::from_str::<Up>(text).ok()
}

/// Сериализовать сообщение вниз в одну строку NDJSON.
pub fn encode_down(down: &Down) -> String {
    serde_json::to_string(down).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn down_round_trip() {
        let hello = Down::Hello {
            id: "1".to_string(),
            protocol: PROTOCOL_VERSION,
        };
        let text = encode_down(&hello);
        assert!(text.contains("\"type\":\"hello\""));
        let back: Down = serde_json::from_str(&text).unwrap();
        assert_eq!(back, hello);
    }

    #[test]
    fn start_carries_provider_and_optional_history() {
        let start = Down::Start {
            id: "2".to_string(),
            project: "P0001".to_string(),
            provider: Provider {
                kind: "openrouter".to_string(),
                base_url: "https://openrouter.ai/api/v1".to_string(),
                model: "anthropic/claude-sonnet-4".to_string(),
                api_key: "sk-test".to_string(),
            },
            history: vec![],
            session_id: None,
            max_iterations: 12,
            subagents: false,
        };
        let text = encode_down(&start);
        assert!(!text.contains("history"), "{text}");
        assert!(text.contains("\"kind\":\"openrouter\""));
        let back: Down = serde_json::from_str(&text).unwrap();
        assert_eq!(back, start);
    }

    #[test]
    fn host_result_round_trip() {
        let reply = Down::HostResult {
            id: "h1".to_string(),
            ok: false,
            result: Value::Null,
            error: Some("privacy_filter_unavailable".to_string()),
        };
        let text = encode_down(&reply);
        let back: Down = serde_json::from_str(&text).unwrap();
        assert_eq!(back, reply);
    }

    #[test]
    fn up_events_and_host_calls_parse() {
        let ready = parse_line(
            r#"{"type":"ready","id":"1","protocol":1,"node":"v24","sdk":"0.2.0","tools":["disk-search__search_local_files"]}"#,
        )
        .unwrap();
        match ready {
            Up::Ready { tools, .. } => assert_eq!(tools.len(), 1),
            other => panic!("ожидался ready, получено {other:?}"),
        }

        let call = parse_line(
            r#"{"type":"host_call","id":"h7","call":"egress_check","payload":{"kind":"model","body":{"messages":[]}}}"#,
        )
        .unwrap();
        match call {
            Up::HostCall { id, call, payload } => {
                assert_eq!(id, "h7");
                match HostCall::parse(&call, &payload) {
                    HostCall::EgressCheck { point, body } => {
                        assert_eq!(point, EgressPoint::Model);
                        assert_eq!(body, json!({"messages": []}));
                    }
                    other => panic!("ожидался egress_check, получено {other:?}"),
                }
            }
            other => panic!("ожидался host_call, получено {other:?}"),
        }
    }

    #[test]
    fn unknown_host_call_is_kept_as_is() {
        let payload = json!({"x": 1});
        match HostCall::parse("no_such_tool", &payload) {
            HostCall::Unknown { name, payload: p } => {
                assert_eq!(name, "no_such_tool");
                assert_eq!(p, payload);
            }
            other => panic!("ожидался Unknown, получено {other:?}"),
        }
    }

    #[test]
    fn non_json_lines_are_ignored() {
        assert!(parse_line("").is_none());
        assert!(parse_line("warning: something").is_none());
        assert!(parse_line("[1,2]").is_none());
    }

    #[test]
    fn error_message_parses_without_stack() {
        let up = parse_line(r#"{"type":"error","message":"boom"}"#).unwrap();
        match up {
            Up::Error { message, stack } => {
                assert_eq!(message, "boom");
                assert!(stack.is_none());
            }
            other => panic!("ожидался error, получено {other:?}"),
        }
    }
}
