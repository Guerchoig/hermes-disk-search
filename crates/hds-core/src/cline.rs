//! Синхронизация настроек Cline (Desktop/CLI/IDE) с `config.yaml` проекта.
//!
//! Зачем: агент Cline ходит в наш MCP-сервер и в `llm-host` по настройкам, которые
//! лежат ВНЕ проекта (`~/.cline/...`), и их легко рассинхронизировать:
//! * `data/settings/models.json` объявляет окно контекста модели — если оно меньше
//!   реального `ctx_per_slot` слота, Cline сжимает историю раньше времени и агент
//!   теряет контекст поиска;
//! * `data/settings/cline_mcp_settings.json` и `mcp.json` — подключение MCP-сервера
//!   disk-search (URL общего инстанса `:8787`, `autoApprove`, `timeout`);
//! * `rules/disk-search.md` и `skills/disk-search/SKILL.md` — то, что доносит до
//!   модели «одна выдача — это фрагменты, делай несколько запросов» (правила Cline
//!   попадают в системный промпт всегда, скиллы грузятся лениво).
//!
//! Один и тот же код используют: UI (`POST /api/cline/sync`), CLI
//! (`hds cline-sync`) и инсталляторы Windows/macOS — расхождения между ними
//! невозможны по построению.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::{dig, replace_file, Config};

/// Инструменты MCP-сервера для `autoApprove` (имена — как в `hds_mcp::schema`).
pub const TOOLS: [&str; 6] = [
    "search_local_files",
    "ask_my_files",
    "index_status",
    "start_indexing",
    "stop_indexing",
    "reindex_path",
];

/// Таймаут MCP-сервера в настройках Cline, сек (`ask_my_files` — RAG до 1–3 мин).
pub const MCP_TIMEOUT_SECS: i64 = 300;

/// Контекст слота чата по умолчанию (как `hds-llama`), токены.
pub const DEFAULT_CHAT_CTX: i64 = 16384;

/// Что нужно сделать после синхронизации (текст для UI/CLI/инсталляторов).
pub const RESTART_NOTE: &str = "Перезапустите Cline (Desktop/CLI): настройки моделей и \
MCP-серверов читаются при его старте, правила и скиллы — при старте новой сессии.";

/// Шаг синхронизации: что делали и чем закончилось (`ok` | `skip` | `warn`).
#[derive(Debug, Clone)]
pub struct Step {
    pub title: String,
    pub status: &'static str,
    pub msg: String,
}

/// Итог синхронизации.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub cline_dir: Option<PathBuf>,
    pub steps: Vec<Step>,
    /// Нужен перезапуск Cline (меняли `models.json` или настройки MCP).
    pub restart_required: bool,
}

impl Report {
    fn push(&mut self, title: impl Into<String>, status: &'static str, msg: impl Into<String>) {
        self.steps.push(Step {
            title: title.into(),
            status,
            msg: msg.into(),
        });
    }

    /// Ни одного `warn` (не найденные файлы Cline — это `skip`, а не ошибка).
    pub fn ok(&self) -> bool {
        self.steps.iter().all(|s| s.status != "warn")
    }

    /// JSON для UI/CLI (`--json`) — тот же формат, что печатает `hds cline-sync`.
    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "cline_dir": self.cline_dir.as_ref().map(|p| p.display().to_string()),
            "restart_required": self.restart_required,
            "restart_note": if self.restart_required { RESTART_NOTE } else { "" },
            "steps": self.steps.iter().map(|s| json!({
                "title": s.title, "status": s.status, "msg": s.msg
            })).collect::<Vec<_>>(),
        })
    }
}

/// Домашний каталог пользователя (`USERPROFILE` → `HOME`).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// Каталог Cline (`~/.cline`). `CLINE_DATA_DIR` заменяет `~/.cline/data`,
/// поэтому корнем считается его родитель (документация Cline, «Config»).
pub fn cline_dir() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("CLINE_DATA_DIR") {
        if !v.is_empty() {
            let p = PathBuf::from(v);
            return Some(match p.file_name().and_then(|n| n.to_str()) {
                Some("data") => p.parent().unwrap_or(&p).to_path_buf(),
                _ => p,
            });
        }
    }
    home_dir().map(|h| h.join(".cline"))
}

/// Контекст слота роли в токенах — та же арифметика, что в `hds-llama::config`
/// (`llm.<role>.n_ctx`, иначе `parallel × llm_server.<role>.ctx_per_slot`).
pub fn role_ctx_tokens(cfg: &Config, role: &str, default_ctx: i64) -> i64 {
    let parallel = dig(cfg, "llm_server.parallel")
        .and_then(|v| v.as_i64())
        .unwrap_or(1)
        .max(1);
    let per_slot = dig(cfg, &format!("llm_server.{role}.ctx_per_slot"))
        .and_then(|v| v.as_i64())
        .unwrap_or(default_ctx);
    dig(cfg, &format!("llm.{role}.n_ctx"))
        .and_then(|v| v.as_i64())
        .unwrap_or(parallel * per_slot)
}

/// URL общего MCP-инстанса из `mcp_http.*` (как в инсталляторах).
pub fn mcp_url(cfg: &Config) -> String {
    let host = dig(cfg, "mcp_http.host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1");
    let port = dig(cfg, "mcp_http.port")
        .and_then(|v| v.as_i64())
        .unwrap_or(8787);
    let path = dig(cfg, "mcp_http.path")
        .and_then(|v| v.as_str())
        .unwrap_or("/mcp");
    format!("http://{host}:{port}{path}")
}

/// Запись `disk-search` для секции `mcpServers` (плоская форма Cline).
pub fn mcp_entry(cfg: &Config) -> Value {
    json!({
        "type": "streamableHttp",
        "url": mcp_url(cfg),
        "disabled": false,
        "autoApprove": TOOLS,
        "timeout": MCP_TIMEOUT_SECS,
    })
}

/// Вставить/обновить запись `disk-search` в объект настроек Cline.
///
/// `true` — данные изменились. Другие серверы сохраняются (как в
/// `installers/cline_mcp_merge.py`); нечитаемая секция `mcpServers` перезаписывается.
pub fn merge_mcp_entry(data: &mut Value, entry: &Value) -> bool {
    if !data.is_object() {
        *data = json!({});
    }
    let servers = match data.as_object_mut() {
        Some(o) => o.entry("mcpServers").or_insert_with(|| json!({})),
        None => return false,
    };
    if !servers.is_object() {
        *servers = json!({});
    }
    let map = match servers.as_object_mut() {
        Some(m) => m,
        None => return false,
    };
    if map.get("disk-search") == Some(entry) {
        return false;
    }
    map.insert("disk-search".to_string(), entry.clone());
    true
}

/// Контекст для модели по её идентификатору, если это алиас роли llm-host.
///
/// Фасад `:8010–8012` отдаёт `chat`, `chat-think` (роль chat), а также роли
/// `embedding`/`rerank`; у них РАЗНЫЕ слоты, поэтому окно подставляется по имени.
fn ctx_for_model(id: &str, chat: i64, embed: i64, rerank: i64) -> Option<i64> {
    let l = id.to_ascii_lowercase();
    if l.contains("embed") {
        Some(embed)
    } else if l.contains("rerank") {
        Some(rerank)
    } else if l.starts_with("chat") {
        Some(chat)
    } else {
        None
    }
}

/// Синхронизировать окна контекста в `models.json` Cline.
///
/// Правятся только модели провайдера, чей `baseUrl` совпадает с `chat.base_url`
/// (наш `llm-host`): чужие провайдеры и модели не трогаем. Возвращает список
/// изменённых пар «провайдер/модель» (для отчёта) — `contextWindow` приравнивается
/// контексту слота, `maxInputTokens` — тому же значению.
///
/// Дополнительно (важно для поставки): у провайдера `openai-compatible` в Cline
/// каталог моделей **не динамический** — `modelsSourceUrl` у него не задан, кнопка
/// «обновить список» молча ничего не делает, и модели появляются в `models.json`
/// только после ручного ввода id в селекторе (проверено по исходникам
/// cline/cline 0.0.43 и живьём 05.10.2026). Поэтому здесь мы **создаём
/// отсутствующие** записи `chat`/`chat-think` сами, а фантомный дефолт
/// каталога (`defaultModelId: "gpt-4o"` — заглушка Cline, не наша модель)
/// заменяем на `chat`: после синхронизации модели видны в селекторе без ручного
/// ввода.
pub fn sync_models(
    data: &mut Value,
    chat_base_url: &str,
    chat_ctx: i64,
    embed_ctx: i64,
    rerank_ctx: i64,
) -> Vec<String> {
    let want = chat_base_url.trim_end_matches('/').to_ascii_lowercase();
    let mut changed = Vec::new();
    let providers = match data.get_mut("providers").and_then(|p| p.as_object_mut()) {
        Some(p) => p,
        None => return changed,
    };
    for (pname, pval) in providers.iter_mut() {
        let base = pval
            .get("provider")
            .and_then(|p| p.get("baseUrl"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('/').to_ascii_lowercase());
        if base.as_deref() != Some(want.as_str()) {
            continue;
        }
        // Запись `models` может отсутствовать целиком (каталог ещё пуст) — создаём.
        if let Some(obj) = pval.as_object_mut() {
            if !obj.contains_key("models") {
                obj.insert("models".to_string(), json!({}));
            }
            // Фантомный дефолт каталога Cline (например `gpt-4o`) → наш `chat`.
            let default_id = obj
                .get("defaultModelId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !default_id.is_empty() && !default_id.starts_with("chat") {
                obj.insert("defaultModelId".to_string(), json!("chat"));
                changed.push(format!("{pname}/defaultModelId → chat"));
            }
        }
        let models = match pval.get_mut("models").and_then(|m| m.as_object_mut()) {
            Some(m) => m,
            None => continue,
        };
        // Окна контекста существующих записей (по id, как у ролей llm-host).
        for (key, mval) in models.iter_mut() {
            let id = mval
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(key)
                .to_string();
            let ctx = match ctx_for_model(&id, chat_ctx, embed_ctx, rerank_ctx) {
                Some(c) if c > 0 => c,
                _ => continue,
            };
            let same = mval.get("contextWindow").and_then(|v| v.as_i64()) == Some(ctx)
                && mval.get("maxInputTokens").and_then(|v| v.as_i64()) == Some(ctx);
            if same {
                continue;
            }
            if let Some(obj) = mval.as_object_mut() {
                obj.insert("contextWindow".to_string(), json!(ctx));
                obj.insert("maxInputTokens".to_string(), json!(ctx));
                changed.push(format!("{pname}/{id} → {ctx}"));
            }
        }
        // Отсутствующие роли чата — создаём (см. док: каталог не динамический).
        for (id, name) in [
            ("chat", "chat (thinking off)"),
            ("chat-think", "chat-think (thinking on)"),
        ] {
            if models.contains_key(id) {
                continue;
            }
            models.insert(
                id.to_string(),
                json!({
                    "id": id,
                    "name": name,
                    "contextWindow": chat_ctx,
                    "maxInputTokens": chat_ctx,
                    "capabilities": ["streaming", "tools"],
                }),
            );
            changed.push(format!("{pname}/{id} → создана ({chat_ctx})"));
        }
    }
    changed
}

/// Прочитать JSON-файл (`Err` — текст для отчёта, а не паника).
fn load_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = text.trim_start_matches('\u{feff}');
    serde_json::from_str(text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Записать JSON атомарно (tmp рядом + [`replace_file`] с ретраями на Windows:
/// файл настроек Cline может держать открытым живой клиент).
fn write_json(path: &Path, v: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{}: нет каталога", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let tmp = path.with_extension("json.tmp");
    let mut body = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    body.push('\n');
    std::fs::write(&tmp, body).map_err(|e| format!("{}: {e}", tmp.display()))?;
    replace_file(&tmp, path).map_err(|e| format!("{}: {}", path.display(), e.message()))
}

/// Скопировать файл, только если содержимое отличается (`true` — обновили).
/// Отсутствие источника — `Ok(None)` (не ошибка: в поставке может не быть правил).
fn copy_if_changed(src: &Path, dst: &Path) -> Result<Option<bool>, String> {
    let bytes = match std::fs::read(src) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    if std::fs::read(dst).map(|cur| cur == bytes).unwrap_or(false) {
        return Ok(Some(false));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(dst, bytes).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok(Some(true))
}

/// Синхронизировать все настройки Cline с конфигом проекта.
///
/// `root` — корень проекта (оттуда берутся источники правила/скилла), `dry_run` —
/// только отчёт без записи. Ошибки ввода-вывода не роняют синхронизацию: шаг
/// помечается `warn`, остальные выполняются.
pub fn sync(cfg: &Config, root: &Path, dry_run: bool) -> Report {
    let mut rep = Report::default();
    let dir = match cline_dir() {
        Some(d) if d.is_dir() => d,
        _ => {
            rep.push(
                "Cline",
                "skip",
                "каталог ~/.cline не найден — Cline не установлен?",
            );
            return rep;
        }
    };
    rep.cline_dir = Some(dir.clone());

    // 1. Окна контекста моделей: иначе Cline сжимает историю раньше реального слота.
    let chat_base = dig(cfg, "chat.base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("http://127.0.0.1:8010/v1");
    let chat_ctx = role_ctx_tokens(cfg, "chat", DEFAULT_CHAT_CTX);
    let embed_ctx = role_ctx_tokens(cfg, "embedding", crate::config::EMB_CONTEXT);
    let rerank_ctx = role_ctx_tokens(cfg, "rerank", crate::config::EMB_CONTEXT);
    let models_path = dir.join("data").join("settings").join("models.json");
    if !models_path.exists() {
        rep.push(
            "models.json",
            "skip",
            format!(
                "не найден ({}) — откройте Cline и выберите модель",
                models_path.display()
            ),
        );
    } else {
        match load_json(&models_path) {
            Err(e) => rep.push(
                "models.json",
                "warn",
                format!("не читается ({e}) — файл не тронут"),
            ),
            Ok(mut data) => {
                let changed = sync_models(&mut data, chat_base, chat_ctx, embed_ctx, rerank_ctx);
                if changed.is_empty() {
                    rep.push(
                        "models.json",
                        "ok",
                        format!("окна контекста уже совпадают (слот чата {chat_ctx})"),
                    );
                } else if dry_run {
                    rep.push(
                        "models.json",
                        "ok",
                        format!("(dry-run) будет: {}", changed.join(", ")),
                    );
                } else {
                    match write_json(&models_path, &data) {
                        Ok(()) => {
                            rep.restart_required = true;
                            rep.push(
                                "models.json",
                                "ok",
                                format!(
                                    "обновлены окна контекста: {} (слоты: чат {chat_ctx}, embedding {embed_ctx}, rerank {rerank_ctx})",
                                    changed.join(", ")
                                ),
                            );
                        }
                        Err(e) => rep.push("models.json", "warn", format!("не записан: {e}")),
                    }
                }
            }
        }
    }

    // 2. MCP-сервер: оба файла настроек Cline (Desktop/CLI и IDE-вариант CLI).
    let entry = mcp_entry(cfg);
    let mcp_files = [
        dir.join("data")
            .join("settings")
            .join("cline_mcp_settings.json"),
        dir.join("mcp.json"),
    ];
    for path in mcp_files {
        let title = format!(
            "MCP {}",
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        );
        let mut data = load_json(&path).unwrap_or_else(|_| json!({}));
        if !merge_mcp_entry(&mut data, &entry) {
            rep.push(title, "ok", "запись disk-search уже актуальна");
        } else if dry_run {
            rep.push(
                title,
                "ok",
                format!("(dry-run) будет записан disk-search → {}", mcp_url(cfg)),
            );
        } else if let Err(e) = write_json(&path, &data) {
            rep.push(title, "warn", format!("не записан: {e}"));
        } else {
            rep.restart_required = true;
            rep.push(
                title,
                "ok",
                format!("сервер disk-search → {}", mcp_url(cfg)),
            );
        }
    }

    // 3. Правило (always-on) и скилл (ленивый) — источники в корне проекта.
    let pairs = [
        (
            "правило disk-search",
            root.join("cline-rules").join("disk-search.md"),
            dir.join("rules").join("disk-search.md"),
        ),
        (
            "правило dev-машина",
            root.join("cline-rules").join("dev-machine.md"),
            dir.join("rules").join("dev-machine.md"),
        ),
        (
            "скилл disk-search",
            root.join("hermes-skill").join("disk-search.md"),
            dir.join("skills").join("disk-search").join("SKILL.md"),
        ),
    ];
    for (title, src, dst) in pairs {
        match copy_if_changed(&src, &dst) {
            Ok(None) => rep.push(
                title,
                "skip",
                format!("нет источника {} — пропущено", src.display()),
            ),
            Ok(Some(false)) => rep.push(title, "ok", "уже актуален"),
            Ok(Some(true)) if dry_run => rep.push(
                title,
                "ok",
                format!("(dry-run) будет обновлён {}", dst.display()),
            ),
            Ok(Some(true)) => rep.push(title, "ok", format!("обновлён {}", dst.display())),
            Err(e) => rep.push(title, "warn", format!("не установлен: {e}")),
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    fn yaml(s: &str) -> Config {
        serde_yaml::from_str(s).unwrap()
    }

    #[test]
    fn merge_keeps_foreign_servers_and_is_idempotent() {
        let mut data = json!({ "mcpServers": { "tavily": { "command": "npx", "autoApprove": ["tavily_search"] } } });
        let entry = json!({ "type": "streamableHttp", "url": "http://127.0.0.1:8787/mcp" });
        assert!(
            merge_mcp_entry(&mut data, &entry),
            "первая запись меняет файл"
        );
        assert_eq!(data["mcpServers"]["tavily"]["command"], "npx");
        assert_eq!(
            data["mcpServers"]["disk-search"]["url"],
            "http://127.0.0.1:8787/mcp"
        );
        assert!(
            !merge_mcp_entry(&mut data, &entry),
            "повторный вызов ничего не меняет"
        );
    }

    #[test]
    fn merge_repairs_broken_mcp_section() {
        let mut data = json!({ "mcpServers": [] });
        assert!(merge_mcp_entry(&mut data, &json!({ "url": "x" })));
        assert!(data["mcpServers"].is_object());
        let mut empty = json!(null);
        assert!(merge_mcp_entry(&mut empty, &json!({ "url": "x" })));
        assert_eq!(empty["mcpServers"]["disk-search"]["url"], "x");
    }

    #[test]
    fn sync_models_touches_only_our_provider() {
        let mut data = json!({ "providers": {
            "openai-compatible": {
                "provider": { "baseUrl": "http://127.0.0.1:8010/v1" },
                "models": {
                    "chat": { "id": "chat", "contextWindow": 16384, "maxInputTokens": 16384 },
                    "chat-think": { "id": "chat-think", "contextWindow": 16384, "maxInputTokens": 16384 },
                    "embedding": { "id": "embedding", "contextWindow": 8192, "maxInputTokens": 8192 }
                }
            },
            "openrouter": {
                "provider": { "baseUrl": "https://openrouter.ai/api/v1" },
                "models": { "~x": { "id": "~x", "contextWindow": 128000, "maxInputTokens": 128000 } }
            }
        }});
        let changed = sync_models(&mut data, "http://127.0.0.1:8010/v1", 65536, 8192, 8192);
        assert_eq!(
            changed,
            vec![
                "openai-compatible/chat → 65536",
                "openai-compatible/chat-think → 65536"
            ]
        );
        let m = &data["providers"]["openai-compatible"]["models"];
        assert_eq!(m["chat-think"]["contextWindow"], 65536);
        assert_eq!(m["chat-think"]["maxInputTokens"], 65536);
        assert_eq!(
            m["embedding"]["contextWindow"], 8192,
            "окно embedding не тронуто"
        );
        assert_eq!(
            data["providers"]["openrouter"]["models"]["~x"]["contextWindow"],
            128000
        );
        // повторный прогон — изменений нет
        assert!(sync_models(&mut data, "http://127.0.0.1:8010/v1", 65536, 8192, 8192).is_empty());
    }

    /// Провайдер настроен, но каталог пуст (модель руками не вводили): sync сам
    /// создаёт `chat`/`chat-think` с окнами слотов и чинит фантомный дефолт Cline.
    #[test]
    fn sync_models_creates_missing_role_models() {
        let mut data = json!({ "providers": {
            "openai-compatible": {
                "provider": { "baseUrl": "http://127.0.0.1:8010/v1" },
                "defaultModelId": "gpt-4o",
                "models": {}
            },
            "openrouter": {
                "provider": { "baseUrl": "https://openrouter.ai/api/v1" },
                "defaultModelId": "~x",
                "models": {}
            }
        }});
        let changed = sync_models(&mut data, "http://127.0.0.1:8010/v1", 32768, 8192, 8192);
        assert_eq!(
            changed,
            vec![
                "openai-compatible/defaultModelId → chat",
                "openai-compatible/chat → создана (32768)",
                "openai-compatible/chat-think → создана (32768)"
            ],
            "чужой провайдер не тронут"
        );
        let m = &data["providers"]["openai-compatible"]["models"];
        assert_eq!(m["chat"]["contextWindow"], 32768);
        assert_eq!(m["chat"]["maxInputTokens"], 32768);
        assert_eq!(m["chat"]["name"], "chat (thinking off)");
        assert_eq!(m["chat-think"]["capabilities"][0], "streaming");
        assert_eq!(
            data["providers"]["openai-compatible"]["defaultModelId"], "chat",
            "фантомный gpt-4o заменён на chat"
        );
        assert_eq!(
            data["providers"]["openrouter"]["defaultModelId"], "~x",
            "чужой defaultModelId не тронут"
        );
        // повторный прогон — изменений нет
        assert!(sync_models(&mut data, "http://127.0.0.1:8010/v1", 32768, 8192, 8192).is_empty());
    }

    #[test]
    fn role_ctx_matches_llm_host_arithmetic() {
        let explicit = yaml("llm_server:\n  parallel: 2\n  chat:\n    ctx_per_slot: 8192\nllm:\n  chat:\n    n_ctx: 4096\n");
        assert_eq!(role_ctx_tokens(&explicit, "chat", DEFAULT_CHAT_CTX), 4096);
        let per_slot = yaml("llm_server:\n  parallel: 2\n  chat:\n    ctx_per_slot: 8192\n");
        assert_eq!(role_ctx_tokens(&per_slot, "chat", DEFAULT_CHAT_CTX), 16384);
        let nothing = yaml("chat: {}\n");
        assert_eq!(
            role_ctx_tokens(&nothing, "chat", DEFAULT_CHAT_CTX),
            DEFAULT_CHAT_CTX
        );
        assert_eq!(
            role_ctx_tokens(&nothing, "embedding", crate::config::EMB_CONTEXT),
            crate::config::EMB_CONTEXT
        );
    }

    #[test]
    fn mcp_entry_reads_mcp_http_section() {
        let cfg = yaml("mcp_http:\n  host: 127.0.0.1\n  port: 8799\n  path: /mcp\n");
        assert_eq!(mcp_url(&cfg), "http://127.0.0.1:8799/mcp");
        let e = mcp_entry(&cfg);
        assert_eq!(e["type"], "streamableHttp");
        assert_eq!(e["disabled"], false);
        assert_eq!(e["timeout"], 300);
        assert_eq!(e["autoApprove"].as_array().unwrap().len(), TOOLS.len());
        // дефолты, если секции нет
        assert_eq!(mcp_url(&yaml("chat: {}\n")), "http://127.0.0.1:8787/mcp");
    }
}
