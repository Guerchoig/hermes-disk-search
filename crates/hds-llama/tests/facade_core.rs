//! A5 — ядро фасада `:8010–8012`: маршрутизация, сборка запроса, ответы, thinking.
//!
//! Паритет проверяется по контракту Python-версии (`hds/rag.py`, `hds/llama_server.py`)
//! и по фактам замера `bin/chat_probe` (движок сам применяет шаблон чата, `reasoning=off`
//! даёт ответ без размышлений, `on+format=none` — видимые размышления).

use serde_json::json;

use hds_llama::facade::{
    build_chat_request, build_prompt, chat_response_json, default_port, error_json, health_json,
    models_json, not_found_json, props_json, reasoning_flags, route, strip_think,
    validate_embeddings, validate_rerank, thinking_for, ChatRequest, Route, Thinking, Usage,
};

/// Порты совпадают с `llama-server` — клиенты (UI, MCP, внешние агенты) не меняются.
#[test]
fn default_ports_match_llama_server() {
    assert_eq!(default_port("chat"), 8010);
    assert_eq!(default_port("embedding"), 8011);
    assert_eq!(default_port("rerank"), 8012);
    assert_eq!(default_port("whisper"), 0, "роль вне фасада");
}

/// Маршруты: с `/v1` и без, query-строки, чужие пути и методы.
#[test]
fn routing_matches_llama_server_paths() {
    assert_eq!(route("GET", "/health"), Route::Health);
    assert_eq!(route("GET", "/v1/health"), Route::Health);
    assert_eq!(route("GET", "/props"), Route::Props);
    assert_eq!(route("GET", "/v1/models?x=1"), Route::Models);
    assert_eq!(route("POST", "/v1/chat/completions"), Route::Chat);
    assert_eq!(route("POST", "/chat/completions"), Route::Chat);
    assert_eq!(route("POST", "/v1/embeddings"), Route::Embeddings);
    assert_eq!(route("POST", "/v1/rerank"), Route::Rerank);
    assert_eq!(route("GET", "/v1/chat/completions"), Route::NotFound, "нужен POST");
    assert_eq!(route("POST", "/v1/unknown"), Route::NotFound);
    // W3: внутренняя транскрибация через владельца
    assert_eq!(route("POST", "/internal/transcribe"), Route::InternalTranscribe);
    assert_eq!(route("POST", "/v1/internal/transcribe"), Route::InternalTranscribe);
    assert!(Route::InternalTranscribe.is_internal());
    assert!(Route::InternalTranscribe.needs_body());
    assert_eq!(Route::Chat.role(), Some("chat"));
    assert_eq!(Route::Embeddings.role(), Some("embedding"));
    assert_eq!(Route::Rerank.role(), Some("rerank"));
    assert_eq!(Route::Health.role(), None);
    assert!(Route::Chat.needs_body() && !Route::Health.needs_body());
}

/// Режимы размышлений: приоритет тело → алиас → конфиг (план §11.4 и Python-версия).
#[test]
fn thinking_precedence_follows_body_then_alias_then_config() {
    // 1. `chat_template_kwargs.enable_thinking` — так выключает размышления Python-версия
    let body = json!({"chat_template_kwargs": {"enable_thinking": false}, "reasoning": "on"});
    assert_eq!(thinking_for("chat-think", &body, Thinking::On), Thinking::Off);
    let body = json!({"chat_template_kwargs": {"enable_thinking": true}});
    assert_eq!(thinking_for("chat", &body, Thinking::Off), Thinking::On);
    // 2. поле `reasoning` (llama-server-совместимое)
    let body = json!({"reasoning": "auto"});
    assert_eq!(thinking_for("chat", &body, Thinking::Off), Thinking::Auto);
    // 3. алиас `chat-think`
    assert_eq!(thinking_for("chat-think", &json!({}), Thinking::Off), Thinking::On);
    assert_eq!(thinking_for("chat", &json!({}), Thinking::Off), Thinking::Off);
    // 4. значение из конфига
    assert_eq!(thinking_for("chat", &json!({}), Thinking::On), Thinking::On);
}

/// Сборка prompt: для пары system+user вход совпадает с тем, что Python-версия
/// отправляла в RAG; история сводится в один текст (движок шаблонит сам).
#[test]
fn prompt_rendering_matches_python_rag_input() {
    let messages = vec![
        json!({"role": "system", "content": "SYSTEM"}),
        json!({"role": "user", "content": "Вопрос пользователя: N\n\nФрагменты:\n[1] Файл"}),
    ];
    assert_eq!(
        build_prompt(&messages).unwrap(),
        "SYSTEM\n\nВопрос пользователя: N\n\nФрагменты:\n[1] Файл"
    );

    // многоходовой диалог: прошлые реплики помечаем, последняя — как есть
    let messages = vec![
        json!({"role": "user", "content": "вопрос 1"}),
        json!({"role": "assistant", "content": "ответ 1"}),
        json!({"role": "user", "content": "вопрос 2"}),
    ];
    assert_eq!(
        build_prompt(&messages).unwrap(),
        "Пользователь: вопрос 1\nАссистент: ответ 1\nвопрос 2"
    );

    // части контента (vision-стиль) склеиваются по `text`
    let messages = vec![json!({"role": "user",
        "content": [{"type": "text", "text": "а"}, {"type": "text", "text": "б"}]})];
    assert_eq!(build_prompt(&messages).unwrap(), "а\nб");

    // без пользовательских сообщений — понятная ошибка
    let err = build_prompt(&[json!({"role": "system", "content": "s"})]).unwrap_err();
    assert!(err.to_string().contains("messages"), "{err}");
}

/// Параметры запроса: `max_tokens`/`temperature` с дефолтами из конфига, stream запрещён.
#[test]
fn chat_request_parses_options_with_config_defaults() {
    let body = json!({"messages": [{"role": "user", "content": "привет"}]});
    let req: ChatRequest = build_chat_request(&body, Thinking::Off, 600, 0.2).unwrap();
    assert_eq!(req.prompt, "привет");
    assert_eq!(req.n_predict, 600);
    assert!((req.temperature - 0.2).abs() < 1e-6);
    assert_eq!(req.thinking, Thinking::Off);
    assert_eq!(req.reasoning(), Some(("off", 0, None)));

    let body = json!({
        "messages": [{"role": "user", "content": "привет"}],
        "max_tokens": 64, "temperature": 0.7, "model": "chat-think",
    });
    let req = build_chat_request(&body, Thinking::Off, 600, 0.2).unwrap();
    assert_eq!(req.n_predict, 64);
    assert!((req.temperature - 0.7).abs() < 1e-6);
    assert_eq!(req.thinking, Thinking::On, "алиас chat-think → размышления");
    assert_eq!(req.reasoning(), Some(("on", -1, Some("none"))));

    let body = json!({"messages": [], "stream": true});
    let err = build_chat_request(&body, Thinking::Off, 600, 0.2).unwrap_err();
    assert!(err.to_string().contains("stream"), "{err}");
    let err = build_chat_request(&json!({}), Thinking::Off, 600, 0.2).unwrap_err();
    assert!(err.to_string().contains("messages"), "{err}");
}

/// `strip_think` — порт `hds/rag.py::_strip_think`.
#[test]
fn strip_think_parity_with_python() {
    let lt = '\u{3c}';
    let gt = '\u{3e}';
    let open = format!("{lt}think{gt}");
    let close = format!("{lt}/think{gt}");

    let text = format!("{open}размышления{close}\n\nОтвет: 4");
    assert_eq!(strip_think(&text), "Ответ: 4");
    // блок без закрытия — режем до конца
    let text = format!("Ответ{open}обрыв");
    assert_eq!(strip_think(&text), "Ответ");
    // одинокий закрывающий маркер (модель начала с середины размышлений)
    let text = format!("середина{close}\n\nОтвет: 5");
    assert_eq!(strip_think(&text), "середина");
    // без маркеров — только trim
    assert_eq!(strip_think("  Ответ: 6  "), "Ответ: 6");
    // два блока подряд
    let text = format!("{open}а{close}{open}б{close}итог");
    assert_eq!(strip_think(&text), "итог");
}

/// Ответ чата: форма OpenAI + отсутствие размышлений при `thinking=off`.
#[test]
fn chat_response_shape_is_openai_compatible() {
    let lt = '\u{3c}';
    let gt = '\u{3e}';
    let with_think = format!("{lt}think{gt}секрет{lt}/think{gt}Ответ: 4");
    let v = chat_response_json(
        "chat",
        &with_think,
        Thinking::Off,
        Usage {
            prompt_tokens: 56,
            completion_tokens: 2,
        },
    );
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["model"], "chat");
    assert_eq!(v["choices"][0]["message"]["role"], "assistant");
    assert_eq!(v["choices"][0]["message"]["content"], "Ответ: 4");
    assert_eq!(v["choices"][0]["finish_reason"], "stop");
    assert_eq!(v["usage"]["prompt_tokens"], 56);
    assert_eq!(v["usage"]["completion_tokens"], 2);
    assert_eq!(v["usage"]["total_tokens"], 58);
    assert!(v["choices"][0]["message"].get("reasoning_content").is_none());

    // chat-think: размышления видны и в content, и в reasoning_content
    let v = chat_response_json("chat-think", &with_think, Thinking::On, Usage::default());
    assert!(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("секрет"));
    assert!(v["choices"][0]["message"]["reasoning_content"]
        .as_str()
        .unwrap()
        .contains("секрет"));
}

/// Флаги для движка — по §11.4: `off` форсирует `reasoning_budget = 0`,
/// `on` — `-1` и видимый формат, `auto` — ничего не отправляем.
#[test]
fn reasoning_flags_map_to_engine_contract() {
    assert_eq!(reasoning_flags(Thinking::Off), Some(("off", 0, None)));
    assert_eq!(reasoning_flags(Thinking::On), Some(("on", -1, Some("none"))));
    assert_eq!(reasoning_flags(Thinking::Auto), None);
}

/// `/props` — по этим полям Python-версия (`probe`) считает инстанс «своим»,
/// а UI показывает фактический контекст (`props_context`).
#[test]
fn props_shape_is_what_python_probe_expects() {
    let v = props_json("chat", "C:\\llama-runtime\\models\\chat\\qwen.gguf", 32768, 1, "chat");
    assert_eq!(v["total_slots"], 1, "иначе probe вернёт FOREIGN");
    assert_eq!(v["model_path"], "C:\\llama-runtime\\models\\chat\\qwen.gguf");
    assert_eq!(v["default_generation_settings"]["n_ctx"], 32768);
    assert_eq!(v["n_ctx"], 32768);
    assert_eq!(v["alias"], "chat");

    assert_eq!(health_json()["status"], "ok");

    let m = models_json(&["chat".to_string(), "chat-think".to_string()], 1_700_000_000);
    assert_eq!(m["object"], "list");
    assert_eq!(m["data"][0]["id"], "chat");
    assert_eq!(m["data"][1]["id"], "chat-think");

    let e = error_json("нет роли", "invalid_request_error");
    assert_eq!(e["error"]["message"], "нет роли");
    assert!(not_found_json("/x")["error"]["message"]
        .as_str()
        .unwrap()
        .contains("/v1/chat/completions"));
}

/// Проверка ответов движка: «пустые» эмбеддинги/реранк — ошибка, а не тишина.
#[test]
fn engine_responses_are_validated() {
    let ok = r#"{"data":[{"embedding":[0.1,0.2],"index":0}],"model":"embedding"}"#;
    assert!(validate_embeddings(ok).is_ok());
    let bad = r#"{"data":[{"index":0}],"model":"embedding"}"#;
    assert!(validate_embeddings(bad)
        .unwrap_err()
        .to_string()
        .contains("embedding"));
    assert!(validate_embeddings("не json").is_err());

    let ok = r#"{"results":[{"index":0,"relevance_score":0.9}]}"#;
    assert!(validate_rerank(ok).is_ok());
    let bad = r#"{"data":[]}"#;
    assert!(validate_rerank(bad).unwrap_err().to_string().contains("results"));
}

