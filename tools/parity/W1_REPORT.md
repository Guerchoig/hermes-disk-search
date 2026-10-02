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


## 5. Перевод портов 8010–8012 на `llm-host` (live, 01.10.2026)

**Задача.** Владельцем боевых портов 8010–8012 сделать Rust-`llm-host` (Python-роли —
только dev-инструмент, по решению заказчика их можно останавливать).

**Что сделано.**
1. Остановлены Python-роли: `.venv\Scripts\python.exe -m hds.llama_server stop all` →
   порты 8010/8011/8012 свободны, VRAM освобождена (11799 МиБ).
2. Поднят резидентный `llm_host run` (владелец 8010=chat, 8011=embedding, 8012=rerank);
   `/internal/status` → 200; `index.pause` заказчика переиспользована, **не снята**.
3. Проверено через фасад: embeddings (`dim=1024`), chat (`POST /v1/chat/completions`),
   поиск/RAG на **реальном индексе** (`D:\hermes-disk-search-db\index.db`):
   `hds search "1С:Документооборот пороговые суммы"` и
   `hds ask "Что говорится про пороговые суммы согласования договоров?"` → ответ со
   ссылками `[N]` и списком реальных источников (`D:\САША\…\Положение о договорной
   работе … .pdf`).

**Найденная особенность (и фикс).** У cluster API **нет ручки типа KV** (факт W2,
`W2_REPORT.md` §10.2a): `llm-host` грузит чат с **f16-KV** (≈8492 МиБ), тогда как Python
работал с `--cache-type-k/v q8_0`. На 12 ГБ «чат f16 + embedding(636) + резерв(1024)» не
уживались: диспетчер выгружал `chat` ради `embedding`, а назад чат не влезал (недостаток
774 МиБ), `hds ask` деградировал до списка источников. Фикс (обратимый):
* `llm_server.chat.ctx_per_slot: 32768 → 16384` — KV вдвое меньше (гибридная модель,
  8 слоёв), чат ≈7,9 ГБ, теперь уживается с embedding;
* `rerank.url: "http://localhost:8012/v1" → "http://127.0.0.1:8012/v1"` — `localhost`
  на этой машине резолвится в IPv6 `::1`, а `llm-host` слушает `127.0.0.1` (реранк падал
  `connect refused`).
* Бэкап конфига: `data/config.yaml.bak-20261001`.

**Итог.** Владелец 8010–8012 — `llm-host`; `hds search`/`hds ask`/`hds mcp` работают на
боевом индексе без Python-ролей. Для автозапуска ОС — `installers/install_llm_host_task.ps1`
(проверка переносится в W4). Python-watcher/MCP/UI продолжают работать через фасад.


## 6. Паритет поиска на БОЕВОЙ БД + запуск резидента из release (01.10.2026)

**Паритет поиска на реальном индексе.** Контрольные запросы заказчика
(`golden/real_db_queries.json`, W0 §12.3) прогнаны Rust-поиском по боевой БД
(`D:\hermes-disk-search-db\index.db`, ~61k чанков): против **свежего** Python-эталона
оба запроса совпали **точно** (топ-20, включая порядок).

* Тест: `crates/hds-search/tests/real_db_parity.rs` (`#[ignore]`;
  `.venv` + фасад `:8011` + боевая БД + golden).
* Важно: **золотой файл дрейфует** — БД меняется (watcher), поэтому прежний
  committed-golden давал расхождение (запрос 2: 12/20). После перегенерации
  (`tools/parity/golden_queries.py`) Rust совпал 20/20 по обоим запросам. Golden
  обновлён (коммит).

**Резидент `llm-host` — из release.** Живая грабля: `cargo test` не может
перезаписать `target\debug\llm_host.exe` (os error 5), пока резидент запущен из
**debug**-сборки. Владельцем портов 8010–8012 держим **release**-резидента
(`target\release\llm_host.exe run`) — debug-сборки/тесты разблокированы.


## 7. Веб-интерфейс `hds-ui` (перепроектированный, 01.10.2026)

**Почему не 1:1-порт.** `hds/ui_server.py` (1201 стр.) — это **операционный слой Python**
(`llama_server` start/stop, скачивание/смена моделей, `chat-model/set`, правка
`config.yaml`, watch-autostart). После перехода на `llm-host` (роли держит Rust) этот
слой неактуален, поэтому UI **перепроектирован** под Rust-стек, а не скопирован.

**Что сделано** — крейт `crates/hds-ui` (свой лёгкий HTTP-сервер + встроенная страница):

| Эндпоинт | Назначение |
|---|---|
| `GET /` | страница (статус/поиск/ask/управление индексацией), `text/html` |
| `GET /api/status` | индекс (`db.stats` + heartbeat) + роли `llm-host` (HTTP `/internal/status`) |
| `GET /api/search?q=&limit=&kinds=` | результаты поиска (`hds-search`) |
| `GET /api/ask?q=` | RAG-ответ (`hds-search::rag`) |
| `POST /api/index/start?full=` / `stop` / `pause` / `resume` | управление индексацией через файлы `index.stop`/`index.pause` |

Запуск: `hds ui [--host H] [--port N]` (default `127.0.0.1:8765`) или бинарь `hds_ui`.

**Живой прогон** (боевой `config.yaml`, `llm-host` владеет 8010–8012):
`GET /` → 200 (страница); `GET /api/status` → реальные данные (612 220 чанков; `llm-host`
up, chat `LOADED`, `n_ctx=16384`); `GET /api/search?q=Технические задания` → 2 результата
с реальными путями `D:\…`.

**Тесты:** `tests/ui_core.rs` — 4 (страница/404/валидация/декодирование query).
Итого `cargo test --workspace` — **169 passed / 0 failed (+9 ignored)**.

**Дополнено (01.10.2026): дерево, диагностика, CSRF.**
* `GET /api/tree` — дерево папок индексации (порт `_build_trees`): статус каталога
  `done`/`partial`/`none` со свёрткой по потомкам, глубина ≤ 4, лимит детей 40.
  По умолчанию строится **по БД** (0 с на боевой БД, 7797 каталогов); обход диска —
  опционально `?walk=1` (бюджет 20 с, флаг `truncated`).
* `GET /api/diagnostics` — облегчённая проверка: config, БД (`db.stats`), корни,
  роли `embedding`/`chat` (`/props`), `llm-host` (`/internal/status`); `ok` =
  отсутствие `fail`.
* **CSRF** для POST (порт `_csrf_ok`): `Origin` пуст или loopback **и**
  (`Content-Type: application/json` **или** заголовок `X-HDS-UI: 1`; страница его шлёт).
  Прочее → `403`.
* Страница: секции «Проверка компонентов» и «Дерево индекса».

**Найденный баг (стоил времени).** Первая версия дерева «съедала» CPU: в графе
`children` один и тот же предок добавлялся **на каждого потомка** → дубли поддеревьев
и экспоненциальный рост. Исправлено: набор нужных путей (`HashSet`) связывается
**по одному разу**. Плюс убран `canonicalize` корня (Windows даёт verbatim `\\?\D:\`,
из-за чего префикс не совпадал и деревья были пустыми).

**Правка настроек (01.10.2026).** Добавлено редактирование `config.yaml` из UI с
**сохранением комментариев** (`crates/hds-ui/src/config_edit.rs`, порт
`_set_roots`/`_set_exclude_paths`): строчный редактор заменяет список под ключом
`  <key>:` (блочный или inline), проверяет итоговый YAML и пишет атомарно
(`replace_file`). Эндпоинты: `GET /api/config` (roots/exclude_paths/exclude_dirs),
`POST /api/roots/save {roots}` и `POST /api/config/excludes {paths}`.
Живьём (временный конфиг с комментариями): смена корней и исключений проходит,
**шапка-комментарий и соседние ключи сохранены**.

**Далее:** автозапуск UI, при желании — полный `hds check` в диагностике, кэш дерева,
`index.pause`-переключатель (W4).

