# ПЛАН: переход hermes-disk-search с LM Studio на llama-server

**Статус:** черновик к согласованию.
**Прецедент:** `C:\Test\anonymizer_proxy` — модуль `anonymizer_proxy/llm_server.py`
(менеджер llama-server: probe → build_command → отвязанный запуск → PID-файл →
CLI check/start/stop/status/restart/run) и его установщики
(`install.ps1` шаг 3, `install.sh` шаг 3).

## 0. Мотивация

LM Studio не позволяет тонко управлять thinking-моделями (reasoning budget,
формат reasoning, `chat_template_kwargs`), молча усекает вход длиннее загруженного
контекста (замер: PLAN_INDEX_QUALITY.md §A2), требует «танцы» `lms load/unload`
для автозагрузки и плодит дубликаты инстансов, съедающие VRAM.

llama-server (llama.cpp) закрывает всё:

- **thinking-управление**: `--reasoning-budget 0` (сервер) и
  `"chat_template_kwargs": {"enable_thinking": false}` (per-request, Qwen3.x) —
  точное включение/выключение размышлений, `--reasoning-format` для парсинга;
- **явные ошибки контекста**: 400 `exceed_context_size` вместо тихого усечения
  (проверено tools/diag_a2_truncation.py);
- **полная автоматизация**: запуск/стоп/статус из кода проекта без LM Studio UI,
  без lms CLI, без зависимости от стороннего GUI-приложения;
- **одно соединение**: `--parallel 1` — ровно один запрос в работе, остальные
  в очереди сервера (как в anonymizer_proxy).

Качество векторов не меняется: A3/A3b замеры показали, что llama.cpp с тем же
GGUF bge-m3 (`--embedding --pooling cls`) даёт косинус ≥ 0.9987 с LM Studio и
≈ 0.999 с эталоном fp32. Уже доказано в проекте: rerank.py давно работает с
llama-server (`--reranking --pooling rank`, порт 8012), diag_a23_engine.py
запускал llama-server с bge-m3.

## 1. Целевая архитектура: «одна модель — один llama-server»

В anonymizer_proxy один llama-server обслуживает одну чат-модель. У hermes
три модели с разными режимами инференса — значит **три отдельных инстанса
llama-server** под управлением одного менеджера (один процесс llama.cpp не
умеет обслуживать чат- и эмбеддинг-модель одновременно):

| Роль       | Порт | Флаги режима                | Модель (GGUF)                     |
|------------|------|------------------------------|-----------------------------------|
| chat       | 8010 | (текст; БЕЗ tool-calling)    | qwen3.5-9b Q6_K (~7,5 ГБ)         |
| embedding  | 8011 | `--embedding --pooling cls`  | bge-m3 Q8_0 (~1,2 ГБ, как сейчас) |
| rerank     | 8012 | `--reranking --pooling rank` | bge-reranker-v2-m3 Q8_0 (~600 МБ) |

Порты — единый блок проекта **8010–8012**:

- rerank уже живёт на 8012 (config `rerank.url`) — не трогаем;
- 8080 занят llama-server'ом anonymizer_proxy: если оба проекта на одной
  машине разделят порт, наш probe() «переиспользовал» бы чужой сервер с
  чужой моделью. Отдельный блок портов снимает конфликт, LM Studio (:1234)
  тоже не задеваем.

GGUF-файлы хранятся в папке проекта `models/{chat,embedding,rerank}/`
(не в `~/.lmstudio` — это уже исключено из индексации: `exclude_dirs`
содержит `"models"`).

## 2. Размеры контекстных окон (по умолчанию)

llama.cpp принимает общий буфер `--ctx-size = parallel × ctx_per_role`;
при `--parallel 1` слот получает ровно свой контекст.

| Роль      | ctx по умолчанию | Обоснование |
|-----------|------------------|-------------|
| chat      | **16384**        | RAG-запрос: system (~120) + контекст ≤ 14 000 симв. ≈ 5К токенов + ответ 600 + запас на thinking ×2. 16384 = 3-кратный запас. 32К (стандарт anonymizer_proxy для целых файлов) здесь не нужен — rag.py отдаёт только фрагменты, а VRAM дороже: KV-кэш qwen3.5-9b на 32К ≈ 4–5 ГБ fp16. |
| embedding | **8192**         | Жёсткое требование `EMB_CONTEXT` (hds/config.py): медиана чанка 484 токена, p90 829, max 2114; при ctx < 8192 длинные чанки теряют хвост. llama-server вернёт ЯВНУЮ ошибку 400 — молчаливого усечения больше нет. |
| rerank    | **8192**         | Пул top-20 фрагментов (~500 симв. каждый) + запрос ≈ 2–3К токенов — с запасом. |

VRAM-бюджет на RTX 3060 12 ГБ (эталонная машина):

- chat Q6_K: ~7,2 ГБ веса + KV 16К в q8_0 (~1,2 ГБ) + буферы ≈ **9,0 ГБ**
- embedding Q8_0 на GPU: ~1,5 ГБ
- rerank по умолчанию `-ngl 0` (CPU) — 0 ГБ GPU
- итого ~10,5 ГБ из 12 ГБ — помещается; при нехватке (8 ГБ-машины) —
  `llm_server.chat.extra_args` с меньшим `-ngl` или CPU-путь.

KV-кэш чат-сервера квантуем флагами
`--cache-type-k q8_0 --cache-type-v q8_0` (в 2 раза меньше памяти,
качество не страдает — общепринятая практика llama.cpp).

## 3. Очередь: один запрос на сервер, остальные ждут

Требование «с llama-server одновременно работает только один запрос»
закрывается **на стороне llama-server**, без клиентских семафоров:

- все три инстанса запускаются с `--parallel 1` — один слот; второй запрос
  ложится в очередь llama-server (FCFS), память на очередь не тратится
  (тот же подход, что `LLM_SERVER_PARALLEL=1` в anonymizer_proxy);
- батчинг эмбеддингов остаётся на стороне клиента (`Embedder.batch=64`
  текстов в одном HTTP-запросе) — это ОДИН запрос к серверу;
- конкуренция процессов (indexer/watcher/UI/MCP-server/Cline) вырождается
  в очередь llama-server; клиенты уже имеют таймаут 600 с (embedder, rag) —
  покрывает ожидание за длинной генерацией;
- индексация эмбеддингами и RAG-генерация не блокируют друг друга — они на
  разных серверах (8011 и 8010), GPU делят, но в очереди не пересекаются.

## 4. Инструменты вызывает агент, а не llama

Инвариант проекта: MCP-инструменты (search_local_files, ask_my_files,
index_status…) вызываются агентом (Cline/Hermes). llama-server в цепочке —
только генератор текста:

1. **чат-сервер запускается БЕЗ `--jinja`** (нет нативного tool-calling на
   стороне сервера) и с `--no-webui` — как в anonymizer_proxy;
2. `rag.py` НИКОГДА не передаёт поле `tools` в /chat/completions — RAG-вызов
   листовой: модель не может «позвать» инструмент, дэдлок (модель ждёт
   результат инструмента, агент ждёт ответ модели) невозможен по построению;
3. агент → MCP-инструмент `ask_my_files` → search + rerank + chat-генерация
   → текст. Никаких вложенных LLM-вызовов из RAG.

Дополнительно в rag.py — «nudge»-повтор как в anonymizer_proxy (llm_router):
thinking-модель может завершить генерацию ВНУТРИ блока размышлений
(content пуст) — один прозрачный дозапрос «дай финальный ответ» вместо
пустого ответа клиенту.

## 5. Thinking-модели (главная причина миграции)

Конфиг `chat.thinking`:

- `"off"` (по умолчанию для RAG — быстрые компактные ответы, таймаут Cline 300 с):
  rag.py добавляет в запрос `"chat_template_kwargs": {"enable_thinking": false}`
  (Qwen3.x; для других моделей — серверный флаг `--reasoning-budget 0`
  в `llm_server.chat.extra_args`); `reasoning_content` игнорируется;
- `"auto"`: thinking разрешён, `reasoning_content` отбрасывается (в ответ
  уходит только `content`), в лог пишется длина размышлений для диагностики;
- скорость/лимиты: `max_tokens 600` остаётся; при off thinking не съедает
  бюджет генерации — ответы стабильнее по времени.

## 6. Конфигурация (config.yaml)

Новая секция (плюс правки дефолтов в `hds/config.py::_default_config_yaml`):

```yaml
llm_server:
  bin: ""                     # путь к llama-server; пусто: tools/llama.cpp/ > PATH
  host: "127.0.0.1"
  autostart: true             # ensure() при старте UI/MCP/cli (неблокирующе)
  start_timeout: 300
  chat:
    port: 8010
    model: "models/chat/qwen3.5-9b-Q6_K.gguf"
    ctx_per_slot: 16384
    extra_args: "--cache-type-k q8_0 --cache-type-v q8_0 -ngl 99"
  embedding:
    port: 8011
    model: "models/embedding/bge-m3-Q8_0.gguf"
    ctx_per_slot: 8192
    extra_args: "--batch-size 8192 --ubatch-size 8192 -ngl 99"
  rerank:
    port: 8012
    model: "models/rerank/bge-reranker-v2-m3-Q8_0.gguf"
    ctx_per_slot: 8192
    extra_args: "-ngl 0"
```

Сопутствующие правки: `chat.base_url` → `http://127.0.0.1:8010/v1`,
`embedding.base_url` → `http://127.0.0.1:8011/v1`, `rerank.url` — без
изменений (8012). Каждому инстансу задаётся `--alias chat|embedding|rerank`,
чтобы имя модели в запросах не зависело от имени GGUF-файла
(сейчас это источник рассинхрона `chat.model` с идентификатором LM Studio
`qwen3.5-9b@q6_k`).

**Совместимость**: клиенты (embedder/rag/rerank) остаются generic-HTTP;
существующий config.yaml пользователя с `:1234` (LM Studio) продолжит
работать, если LM Studio запущен. Установщики же настраивают llama-server.

## 7. Менеджер: hds/llama_server.py (порт из anonymizer_proxy)

Перенос `anonymizer_proxy/llm_server.py` с обобщением на N ролей:

- `ROLES = {chat, embedding, rerank}` — у каждой свой PID-файл
  (`data/llama_chat.pid` и т.д.), лог (`data/logs/llama_<role>.log`),
  команда запуска (build_command по секции конфига);
- `probe(role)`: `/health` → `/props` (признак llama — целое `total_slots`);
  **отличие от anonymizer_proxy**: сверяем `/props.model_path` с ожидаемым
  GGUF роли — чужой llama на порту не переиспользуем (state=foreign);
- запуск: отвязанный `Popen` (Windows: `CREATE_NO_WINDOW|DETACHED_PROCESS`,
  unix: `start_new_session=True`) — переживает перезапуск UI/MCP;
  `--host --port` всегда явно; ожидание готовности поллингом `/health`
  (start_timeout 300);
- `stop()`: `taskkill /PID <pid> /T /F` (win) / SIGTERM; инстансы без
  PID-файла (запущенные вручную) не трогаем;
- `ensure(role)`: probe → переиспользовать живой инстанс с той же моделью,
  иначе запустить; вызывается из ui_server, mcp_server (mcp_start.py), cli
  (`index`/`check`) в фоновом потоке — ошибки не роняют процесс;
- CLI: `python -m hds.llama_server start|stop|status|check|restart|run [role]`
  (`run` — foreground для автозапуска ОС, `os.execvp` на unix);
- автопоиск бинаря: `llm_server.bin` → `tools/llama.cpp/llama-server(.exe)` → PATH.

## 8. Правки модулей проекта

| Модуль | Правка |
|--------|--------|
| hds/embedder.py | дефолт URL 8011; хинты ошибок переписать под llama-server («python -m hds.llama_server start embedding»); 400 exceed_context — явный понятный текст ошибки |
| hds/rag.py | URL 8010; `chat_template_kwargs` по `chat.thinking`; отбрасывать reasoning_content; nudge на пустой content; по-прежнему без tools |
| hds/diag.py | проверки chat/embedding через probe()+/props (фактический ctx из props); убрать lms-блоки; подсказки fix — команда llama_server start |
| hds/ui_server.py | группа «Модель эмбеддингов» → «LLM-серверы»: статус ролей (pid, модель, ctx, слоты), кнопки скачать GGUF/запустить/остановить; скачивание в models/…; удалить lms-код (_loaded_instances и пр.) |
| hds/cli.py | `check` обновить; подсказки в ошибках эмбеддингов |
| hds/config.py | дефолты секции llm_server + новые base_url |
| README, hermes-skill, STATUS | заменить упоминания LM Studio/lms на llama-server |

## 9. Установщики Windows и macOS

**Windows** (`setup.ps1`, `installers/install_windows.ps1`) — по образцу
anonymizer_proxy/install.ps1 шаг 3:

- детект железа уже есть (CUDA при nvidia-smi, иначе Vulkan) — то же
  разделение: `*bin-win-cuda*x64*.zip` или `*bin-win-vulkan-x64*.zip`;
- пре-билд llama.cpp с GitHub Releases (ggml-org/llama.cpp, последний релиз)
  распаковывается в `tools/llama.cpp/`, exe поднимается из вложенной папки
  (`llama-<tag>-bin-win-…/`), проверка `llama-server --version`;
- `ensure_embedding_model.ps1` → `ensure_models` (ps1 + sh): скачивание
  bge-m3-Q8_0.gguf (~1,2 ГБ) и qwen3.5-9b Q6_K (~7,5 ГБ) в
  `models/{embedding,chat}/` — идемпотентно (`.part`, повторный запуск не
  перекачивает), поддержкой HF_TOKEN; **убрать весь lms-код** (lms load
  --context-length 8192 больше не нужен: контекст задаёт менеджер);
- self-test: `llama_server status` + ping-эмбеддинг + мини-чат-запрос.

**macOS** (`installers/install_hermes_macos.sh`, `install_macos.command`) —
по образцу anonymizer_proxy/install.sh:

- `brew install llama.cpp` (Metal включён автоматически на Apple Silicon),
  проверка `/opt/homebrew/bin/llama-server` / PATH;
- те же загрузки GGUF в `models/…`;
- `-ngl 99` работает на Metal; на Intel-маках менеджер стартует с `-ngl 0`.

**Автозапуск**: отдельный демон не обязателен — `ensure()` поднимает
нужные роли при старте UI/watcher/MCP. Для работы «постоянно в памяти»
(install_autostart.ps1 / LaunchAgent) — режим `llama_server run` foreground,
как `run`-команда anonymizer_proxy для launchd.

**release.yml / RELEASE_NOTES**: включить новые файлы (llama_server.py,
ensure_models.*, tests) в сборку релиза; release-заметка с описанием
breaking-change и миграцией.

## 10. Миграция существующих установок

- старый config.yaml продолжает работать (клиенты generic; LM Studio, если
  запущен, отвечает на :1234);
- diag.py при `chat.base_url`/`embedding.base_url` на :1234 даёт warn с
  fix-подсказкой «переход на llama-server: python -m hds.llama_server start»;
- разовая миграция моделей: если `~/.lmstudio/.../bge-m3-Q8_0.gguf` уже есть,
  инсталлятор не перекачивает, а копирует файл в `models/embedding/`
  (быстрый путь; при отсутствии — скачивание);
- в UI-группе «LLM-серверы» — кнопка «Перенести модели из LM Studio»
  (копирование уже скачанных GGUF вместо повторной загрузки 9 ГБ).

## 11. Этапы работ (инкрементально, каждый этап с тестами)

1. **Менеджер** — `hds/llama_server.py` (probe/build_command/start/stop/
   status/ensure/CLI, 3 роли) + `tests/test_llama_server.py`
   (build_command: ctx = parallel × ctx_per_role, `--embedding --pooling cls`,
   `--reranking`, `--no-webui`, отсутствие `--jinja`; probe-состояния llama/
   foreign/down; PID-логика; конфиг-дефолты). Сеть/subprocess — моки,
   изоляция через HDS_CONFIG (как в остальных тестах).
2. **Конфиг** — секция llm_server в `_default_config_yaml()`, новые base_url,
   `ensure_config`-тесты.
3. **Клиенты** — embedder/rag/diag/ui_server/cli по таблице §8 + правки
   tests/test_ui_server.py, test_diag.py, test_rerank.py (фикстуры без lms).
4. **Установщики** — ensure_models (win/mac), llama.cpp-шаг в setup.ps1 и
   install_hermes_macos.sh, self-test, release.yml.
5. **Документация** — README (раздел «LM Studio» → «llama.cpp»),
   hermes-skill/disk-search.md, RELEASE_NOTES.
6. **Живая проверка на эталонной машине (RTX 3060 12 ГБ)**:
   - `llama_server start all`, VRAM-мониторинг (nvidia-smi) с chat Q6_K +
     emb на GPU;
   - индексация ~100 файлов (в т.ч. длинный PDF): батчи 64 текста по ctx 8192 —
     проверить, что llama-server обрабатывает батч в пределах контекста
     (если нет — снизить embedding.batch_size до 16-32 или поднять ctx);
   - поиск, ask_my_files (thinking off → латентность до/после миграции),
     реранкер на 8012;
   - одновременные запросы UI + MCP: убедиться в очереди (лог llama-server);
   - Windows-машина без NVIDIA (Vulkan-сборка) и macOS (brew, Metal).

## 12. Риски и известные развилки

| Риск | Митигация |
|------|-----------|
| VRAM: Q6_K chat + emb на GPU — ~10,5 ГБ из 12 | KV q8_0 по умолчанию; extra_args `-ngl` на роль; при нехватке emb → CPU (-ngl 0) |
| Батч 64 эмбеддингов против ctx 8192 | проверить на живой машине; llama-server может требовать суммарно ≤ ctx на батч — тогда batch_size ↓ (16–32) или ctx ↑; замер в Э6 |
| Пользовательский config.yaml на :1234 | клиенты generic — работает; diag подсказывает миграцию |
| Чужой llama на наших портах | probe сверяет /props.model_path → foreign, не переиспользуем |
| Первая генерация после старта медленная (прогрев) | ensure() при старте UI/MCP поднимает серверы заранее, прогрев-запрос после старта |
| Порты заняты другим софтом | probe → state=foreign → понятная ошибка с fix («смените llm_server.<role>.port») |

## 13. Открытые вопросы (не блокируют этап 1–2)

- Точный репозиторий Q6_K GGUF qwen3.5-9b для скачивания инсталлятором
  (уточнить источник текущего `qwen3.5-9b@q6_k`; кандидат — unsloth/*-GGUF).
- Нужен ли прогрев-запрос chat-модели при старте (тратит VRAM/секунды, но
  убирает «первый запрос медленный»).