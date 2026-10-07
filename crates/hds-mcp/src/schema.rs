//! Описания MCP-инструментов (`tools/list`) и диспетчер вызовов (`tools/call`) —
//! имена, описания и схемы как в `hds/mcp_server.py`.

use serde_json::{json, Value};

use crate::auth::admin_required;
use crate::tools;
use hds_core::access::AuthCtx;

/// Список инструментов (для `tools/list`).
pub fn tool_list() -> Vec<Value> {
    vec![
        json!({
            "name": "search_local_files",
            "description": "ГЛАВНЫЙ инструмент для любых запросов «найди на этом компе/диске…» — файлы, документы, фильмы/видео, картинки, проекты MS Project, музыка. Гибридный (семантический + ключевой) поиск по индексу ВСЕХ локальных дисков: секунды даже на сотнях тысяч файлов. Возвращает фрагменты с путями, страницами и таймкодами. Это ОДНА выдача (limit фрагментов), а не список файлов: для «найди все/какие есть…» вызывайте инструмент несколько раз с разными формулировками и kinds. kinds — необязательный фильтр через запятую: text,pdf,docx,xlsx,pptx,mpp,image,media (фильмы/видео и музыка — \"media\", картинки — \"image\").",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "default": 8 },
                    "kinds": { "type": "string", "default": "" }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "ask_my_files",
            "description": "Ответ на свободный вопрос по СОДЕРЖИМОМУ локальных файлов («о чём этот документ», «в каких проектах упоминается 1С:Документооборот») с цитатами [N] и списком источников (путь, страница/таймкод). Используй для вопросов «что/где/в каких файлах…» по данным с этого компа. limit — сколько фрагментов идёт в контекст ответа (8 по умолчанию, до 30): для обзорных вопросов «какие есть ТЗ/проекты/документы» ставь 15–30, иначе выборка будет неполной.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "limit": { "type": "integer", "default": 8 }
                },
                "required": ["question"]
            }
        }),
        json!({
            "name": "index_status",
            "description": "Состояние индекса: сколько файлов проиндексировано, ошибки, идёт ли индексация.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "start_indexing",
            "description": "Запустить индексацию дисков из конфига в фоне (не блокирует чат). full=True — принудительная переобработка всех файлов.",
            "inputSchema": {
                "type": "object",
                "properties": { "full": { "type": "boolean", "default": false } }
            }
        }),
        json!({
            "name": "stop_indexing",
            "description": "Аккуратно остановить идущую индексацию: все уже обработанные файлы сохраняются, текущий файл будет дообработан при следующем запуске.",
            "inputSchema": { "type": "object", "properties": {} }
        }),
        json!({
            "name": "reindex_path",
            "description": "Переиндексировать один файл или папку (например, после массового изменения).",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }),
    ]
}

/// Диспетчер `tools/call`: имя + аргументы → текст результата.
///
/// `auth` — контекст токена (M1 T4): фильтр проектов для поиска/RAG,
/// admin-гейтинг для управляющих инструментов.
pub fn call(name: &str, args: &Value, auth: Option<&AuthCtx>) -> Result<String, String> {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).map(|x| x.to_string());
    match name {
        "search_local_files" => {
            let query = s("query").ok_or("нет аргумента query")?;
            let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(8);
            let kinds = s("kinds").unwrap_or_default();
            Ok(tools::search_local_files(auth, &query, limit, &kinds))
        }
        "ask_my_files" => {
            let q = s("question").ok_or("нет аргумента question")?;
            let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(8);
            Ok(tools::ask_my_files(auth, &q, limit))
        }
        "index_status" => Ok(tools::index_status()),
        "start_indexing" => {
            admin_required(auth)?; // M1 T4: индексацией занимается только админ
            let full = args.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(tools::start_indexing(full))
        }
        "stop_indexing" => {
            admin_required(auth)?;
            Ok(tools::stop_indexing())
        }
        "reindex_path" => {
            admin_required(auth)?; // M1 T4
            let p = s("path").ok_or("нет аргумента path")?;
            Ok(tools::reindex_path(&p))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}
