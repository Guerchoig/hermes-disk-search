# W1_REPORT.md — журнал волны W1 (резидентный слой: поиск/MCP/UI)

> Ветка `w2-llm-host` (W1 продолжаем в ней), начато **01.10.2026**. План —
> `MIGRATION_PLAN_RUST.md` §4.1 (W1). Правило волны: Python-версия — источник
> истины, паритет проверяется golden/тестами.

## 1. Порт поиска `hds/search.py` → `crates/hds-search` (01.10.2026)

**Что сделано.** Гибридный поиск (FTS5 BM25 + `chunks_vec` + CLIP, слияние RRF)
перенесён в Rust с сохранением поведения Python:

| Файл | Что внутри |
|---|---|
| `crates/hds-search/src/fts.rs` | `find_tokens` (`[\w]{2,}`), `lemmatize_token` (через воркер), `fts_tokens` (≤12), `quoted`, `fts_query`, `fts_search_ids` (AND→OR, префикс хвоста, pushdown `kinds`) |
| `crates/hds-search/src/snippet.rs` | `make_snippet` (окно по первому токену, границы предложений), `format_location` |
| `crates/hds-search/src/lib.rs` | `search()` — RRF по FTS+vec+CLIP, `SearchResult`/`to_json` |
| `crates/hds-cli/src/cmd/search.rs` | `hds search <запрос> [--kinds] [--limit] [--json]` |

**Ключевые решения.**
* Лемматизация токенов запроса — **через Python-воркер** (`hds_index::Lemmatizer`),
  тем же `pymorphy3`, что и `chunks_fts` (иначе FTS не совпадёт) — §6 плана.
* Эмбеддинги — фасад (`hds_index::Embedder`), CLIP — `hds_clip::shared`.
  Ошибки ветвей глушатся (как Python): поиск не падает из-за недоступной роли.
* Порядок результатов — стабильная сортировка по score (порядок первого появления
  = порядок dict в Python), `round(score, 5)`.

**Паритет (golden).** Против БД фикстур `tools/parity/out/index.db` (16 файлов,
6362 чанка, векторы от W0) и golden `search_*.json`: **все 10 контрольных запросов
— топ-20 совпал точно** (`path|page|t_start`, включая порядок).
```powershell
cargo test -p hds-search --test search_parity -- --ignored --nocapture   # Q01..Q10 ok
```
Живой CLI (та же БД, роль embedding `:8011`): `hds search 'Накладная дата склад'`
→ накладная.jpg (OCR), инструкция.md, скан.pdf — с локацией и сниппетом.

**Тесты:** `search_core` (5, чистые: токены, `fts_query`, сниппет, локация) +
`search_parity` (1, `#[ignore]`, golden). Итого `cargo test --workspace` —
**150 passed / 0 failed (+8 ignored)**.

**Артефакты:** `out/w1_search.yaml` (конфиг для CLI против БД фикстур).

**Далее по W1:** `hds-mcp`/`hds-ui` (поиск/`ask` через тот же крейт: `Clip::embed_text`
в поисковую ветку уже подключён), RAG (`rag.ask`).

## 2. RAG-ответ `hds/rag.py` → `hds-search::rag` + реранк (01.10.2026)

**Что сделано.** `ask` (ответ по локальным файлам) перенесён в Rust:

| Файл | Что внутри |
|---|---|
| `crates/hds-search/src/rag.rs` | `ask()` (поиск → контекст → чат-роль `/chat/completions`), `build_context`, `strip_think`, `Answer`/`to_json`; `SYSTEM_PROMPT`/nudge дословно |
| `crates/hds-search/src/rerank.rs` | `rerank_results` (роль rerank `/v1/rerank`, авто-отключение по латентности) |
| `crates/hds-cli/src/cmd/ask.rs` | `hds ask <вопрос> [--limit] [--json]` |

**Поведение сохранено:** чат-роль — только генератор (инструменты не передаются),
`thinking=off` → `chat_template_kwargs.enable_thinking=false`; один nudge-дозапрос при
пустом `content` (размышления в `reasoning_content`); при недоступности модели —
ответ со списком найденных файлов; `rerank.enabled: false` по умолчанию.

**Живой прогон** (чат-роль `:8010`, БД фикстур): вопрос «Где задаются пороговые суммы
согласования договоров?» → корректный ответ со ссылками `[N]` и списком источников
(`инструкция_документооборот.md`, `акты_сверки_многостраничный.pdf`).

**Тесты:** 4 (lib, чистые: `strip_think`, `build_context`/бюджет, `Answer::to_json`).
Итого `cargo test --workspace` — **154 passed / 0 failed (+8 ignored)**.

**Далее по W1:** `hds-mcp`/`hds-ui` (MCP/UI через `hds-search` + `rag::ask`).


## 3. MCP-сервер (stdio) — `crates/hds-mcp` (01.10.2026)

**Что сделано.** Инструменты `hds/mcp_server.py` и stdio-транспорт перенесены в Rust:

| Файл | Что внутри |
|---|---|
| `crates/hds-mcp/src/tools.rs` | `search_local_files`, `ask_my_files`, `index_status`, `start_indexing`, `stop_indexing`, `reindex_path` — те же строки результата, что в Python |
| `crates/hds-mcp/src/schema.rs` | `tools/list` (те же имена/описания/`inputSchema`) и диспетчер `tools/call` |
| `crates/hds-mcp/src/server.rs` | stdio NDJSON JSON-RPC 2.0: `initialize`, `tools/list`, `tools/call`, `ping`, notifications |
| `crates/hds-mcp/src/bin/hds_mcp.rs`, `crates/hds-cli/src/cmd/mcp.rs` | запуск: `hds_mcp` или `hds mcp` (эквивалент `python -m hds.mcp_server`) |

**Ключевые решения.**
* Инструменты — тонкие обёртки: поиск/`ask` через `hds-search`, индексация через
  `hds_index::pipeline::run_index` (+ `MediaRouter`), лемматизация — Python-воркер.
* `index_status` — из `db::stats` + `index.heartbeat.json` (в Python прогресс шёл из
  in-process reporter; отдельный процесс читает heartbeat — семантика та же, R30).
* `initialize` → `serverInfo.name = "disk-search"`, `protocolVersion = 2024-11-05`.

**Живой прогон** (stdio): `initialize` + `tools/list` (6 инструментов, описания/схемы
корректны) + `tools/call search_local_files` → «Найдено 2 фрагментов…» с источниками
и сниппетами.

**Тесты:** `tests/protocol.rs` — 6 (initialize, tools/list, валидация аргумента,
неизвестный метод -32601, ping, notification). Итого `cargo test --workspace` —
**160 passed / 0 failed (+8 ignored)**.

**Далее по W1:** `mcp_http`-менеджер (streamable-http, ОДИН инстанс на машину) и `hds-ui`.


## 4. streamable-http + менеджер (W1, 01.10.2026)

**Что сделано.** Добавлен HTTP-транспорт MCP (ОДИН инстанс на машину) и менеджер —
порт `hds/mcp_http.py`:

| Файл | Что внутри |
|---|---|
| `crates/hds-mcp/src/http.rs` | мини HTTP/1.1-сервер: `GET /health` (`{"app":"disk-search","transport":"streamable-http"}`), `POST <path>` (JSON-RPC → `application/json`, notification → 202), 404; чистая `route()` для тестов |
| `crates/hds-cli/src/cmd/mcp.rs` | `hds mcp [--http --host --port --path]` — stdio или HTTP |
| `crates/hds-cli/src/cmd/mcp_http.rs` | `hds mcp-http check\|start\|stop\|status\|restart\|run`: проба `/health` (mcp/foreign/down), PID-файл `data/mcp_http.pid`, запуск detached (`<exe> mcp --http …`) |

**Ключевые решения.**
* Опознание «наш» инстанс — по `GET /health` (`{"app":"disk-search"}`); чужой сервис
  на порту **не** переиспользуется (`foreign`).
* Свой мини-сервер (не `hds-llama::http`): `hds-mcp` не тянет движок/NVML.
* Ответ POST — `application/json` (одиночный JSON-RPC), notification → 202 (без тела).
* Запуск detached переживает перезапуск UI/агентов (флаги `DETACHED_PROCESS |
  CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` на Windows).

**Живой прогон:** `hds mcp-http check --port 8791` → `down` (1); `start` → `{state:mcp,
pid, version}`; `GET /health` → `{"app":"disk-search",…}`; `POST /mcp initialize` →
JSON-RPC-результат; `status` → mcp; `stop` → порт освобождён, `check` → `down`.

**Тесты:** `tests/http_route.rs` — 5 (/health, /mcp result, notification 202, 404,
parse-error). Итого `cargo test --workspace` — **165 passed / 0 failed (+8 ignored)**.

**Далее по W1:** `hds-ui` (веб-интерфейс). **W4** — упаковка/CI.

