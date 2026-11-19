# AGENT_A3_HANDOFF_PROMPT.md — контекст и промт для фазы A3 (агент `hds_agent`)

> **Как использовать:** открыть новый чат в режиме **Act**, скопировать **§9 целиком** (он
> самодостаточен) и отправить первым сообщением. Разделы §0–§8 — справочная «карта»: промт
> в §9 ссылается на них, исполнитель читает этот файл целиком.
>
> **Дата среза:** 18.11.2026. **Ветка:** `agent-privacy-zone`, HEAD `3bfd1089`
> (`origin/agent-privacy-zone` отстаёт на 18 коммитов — push не делался), дерево чистое.
> **План:** `PLAN_AGENT_PRIVACY_ZONE.md`, редакция 3.1. Правки плана по итогам A0
> (П-1…П-14) действуют; владельцем в план ещё не внесены — они в
> `tools/parity/AGENT_SPIKES.md` §3.11.
> **Отчёты:** A0 — `tools/parity/AGENT_SPIKES.md`; A1/A2 — `RELEASE_NOTES_0.24.0.md`
> («Фаза A1», «Фаза A2»), карта A2 — `AGENT_A2_HANDOFF_PROMPT.md`.

---

## 0. Что происходит (коротко)

`hermes-disk-search` (**hds**) — локальный поиск и ИИ-инструменты для аналитиков и менеджеров
проектов внедрения 1С (проектные документы, не код). Функция по плану: **ИИ-агент в анонимной
зоне** — headless-ядро Cline (`@cline/sdk`) в процессе `hds_agent`, который работает только с
анонимизированными копиями документов (метки `[PERSON_3]` вместо ФИО), а всё исходящее проходит
фильтр hds.

| Фаза | Состояние |
|---|---|
| **A0 — спайки** | закрыт, отчёт `AGENT_SPIKES.md` |
| **A1 — база меток v2** | закрыт: `vault.py`, морфология, `keep_terms`, режим agent, образы, office |
| **A2 — индекс и `disk-search`** | закрыт: корни по `agent.root`, `hds agent init/token`, маршрут `/mcp/agent/<P>`, метки в FTS, отставание индекса (`615be549`…`3bfd1089`) |
| **A3 — агент (`hds_agent`)** | **эта фаза** |
| A4 — демон `hds agent-sync` | дальше |
| A5 — вкладка «Агент» в UI | дальше |
| A6 — выпуск | дальше |

**Фаза A3 (эта карта)** — сам агент и его граница снаружи (§6 плана, строка A3):

1. `sidecar/hds_agent` (Node/TypeScript): протокол §4.3, окружение TR-3.2, провайдеры TR-3.3,
   хук `hds-guard` TR-3.4, office-инструменты TR-3.5, MCP TR-3.6, правила TR-3.8/3.8a,
   жизненный цикл TR-3.9, тесты изоляции TR-3.10;
2. crate `hds-agent` (Rust, внутри процесса `hds ui`): запуск и надзор за процессом агента,
   цикл `host_call`, **шлюз реплик и событий** (TR-5.1…5.3), **словарный фильтр исходящего**
   (TR-6.1…6.7) и деанонимизация событий;
3. `installers/build_agent_sidecar.ps1` — портативный Node + `npm ci` по lock;
4. приёмка: TR-3.10 зелёный + e2e на mock-провайдере (агент читает образ, ищет через
   `disk-search`, вызывает заглушку `tavily`, пишет файл); инварианты I2/I7 на заглушках.

**Что НЕ входит в A3:** демон зеркалирования `hds agent-sync`, реестр артефактов, снимок
основания, «Принять» (A4); вкладка «Агент», SSE-страница, панели «Файлы/Метки/Настройки»
(A5); `hds agent check`, пункт `agent` в `hds check`, CI-сборка Node-воркера, `NOTICE.md`,
`README_AGENT.md` (A6); контрольная точка K1 («с RAG / без RAG») — **после** A3.

## 1. Обязательное чтение (по порядку)

1. **`PLAN_AGENT_PRIVACY_ZONE.md`**: §3 (D1, D8, D10, D11, D12, D16, D17, D20, D22, D23),
   §4.2 (потоки), **§4.3 (канал `hds ui` ↔ `hds_agent`)**, §4.5 (два Cline), §4.6 (почему нет
   прокси), **§5.3 (TR-3.1…TR-3.10)**, §5.5 (шлюз, TR-5.1…5.3), **§5.6 (фильтр, TR-6.1…6.7)**,
   §5.8 (TR-8.1, TR-8.3, TR-8.5), §5.9 (I1–I8), §6 (строка A3), §9 (риски).
2. **`RELEASE_NOTES_0.24.0.md`** — разделы «Фаза A1» и «Фаза A2»: что уже есть в продукте и
   какие решения владельца действуют (В-1…В-11).
3. **`tools/parity/AGENT_SPIKES.md`**: §3.1–3.8 (факты SDK 0.2.0), **§3.11 (П-1…П-14 — правки
   плана)**, **§4 (точные имена API)**, §5 (риски и решения В-1…В-6).
4. **Код до правок:**
   * `crates/hds-anonymize/src/{client,protocol,settings}.rs` — `anonymize_text`,
     `vault_snapshot`/`vault_rev`, `label_for_value`, `merge_entities`, `text_image`,
     `office_read/edit/create`; `Settings::from_config` (GPU-аренда `anon` у `llm_host`);
   * `sidecar/hds_anonymize/{vault,handlers,file_parser}.py` — единственный доступ к базе
     меток (TR-1.1), RPC-контракт и capability-список;
   * `crates/hds-mcp/src/{agent,http,server,schema,tools}.rs` — **маршрут A2**
     `/mcp/agent/<P>` (токен проекта, `projects=[P]`, только `search_local_files` и
     `index_status`);
   * `crates/hds-core/src/{labels,roots,access}.rs` — метки (TR-4.6), корни зоны (TR-4.1),
     хэши/токены (`token_matches`);
   * `crates/hds-extract/src/worker.rs` — шаблон NDJSON-JSON-RPC транспорта к дочернему
     процессу (A3 пишет такой же для Node, но двунаправленный: `host_call`);
   * `crates/hds-ui/src/lib.rs` — **стиль**: потоки и блокирующий I/O, `tokio`/async в
     воркспейсе **нет**;
   * `crates/hds-cli/src/{main,support}.rs` — как устроены подкоманды и общие помощники.
5. **Стенд A0 (переиспользовать, не переписывать):** в git —
   `tools/parity/agent_a0_sdk/{mock_provider.mjs,stand.mjs,s1_…s7_*.mjs,package.json,
   package-lock.json}`; рабочая копия с `node_modules` — `tools/parity/out/agent_a0/sdk/`
   (не в git).
6. **`.gitignore`** — `dist/` и `sidecar/venv-*` уже игнорируются, `node_modules` — **нет**
   (в A3 добавить).

## 2. Состояние репозитория после A2

### 2.1 git

| Что | Значение |
|---|---|
| Ветка | `agent-privacy-zone` |
| HEAD | `3bfd1089` (docs: В-9/В-10) — **18 коммитов впереди `origin`**, push не делался |
| Дерево | чистое; чужих незакоммиченных правок нет |
| Коммиты A2 | `615be549` корни → `d9d5d9fc` метки в FTS → `0a3e5fe9` отставание → `a6a91fa0` маршрут → `52879fb2` init/token → `d4eff475`, `4c53ed87`, `3fe48095`, `dfc58ab4`, `3bfd1089` (docs/В-10) |

### 2.2 Что появилось в A2 (можно и нужно опираться)

| Файл / символ | Что даёт | Как использовать в A3 |
|---|---|---|
| `crates/hds-mcp/src/agent.rs` | `secrets_path_from(cfg)` (env `HDS_AGENT_SECRETS`), `load_at`/`save_at`, `token_ok`, `project_ctx`, `project_route`, `agent_mode` | маршрут `disk-search` для агента: URL `<mcp_http.path>/agent/<P>`, токен — из `secrets.json` проекта |
| `crates/hds-mcp/src/http.rs::handle_agent` | `POST <mcp>/agent/<P>`: 401 на чужой/просроченный токен, `projects=[P]` всегда, только `search_local_files` + `index_status` | агент подключает его как streamableHttp MCP (TR-3.6); `ask_my_files` там нет — правило TR-3.8a это учитывает |
| `crates/hds-mcp/src/server.rs::Scope` | «область маршрута» (global/agent) + `AGENT_INSTRUCTIONS` | образец текста инструкций агента для правил |
| `crates/hds-core/src/labels.rs` | `label_parts`, `fts_token` (`[PERSON_3|дат]` → `xperson3x`), `split_labels`, `normalize_labels`, `LABEL_PATTERN` | разбор меток в событиях/фильтре — **не писать третий разбор** |
| `crates/hds-core/src/roots.rs` | `agent_enabled`, `agent_root`, `agent_roots`, `effective_roots`, `is_agent_project` (`P`+4 цифры) | проверки путей проекта и жизни зоны |
| `crates/hds-core/src/access.rs` | `token_matches(plain, stored_sha256_hex)` (постоянное время) | проверка токена `/mcp/agent` при старте сессии |
| `crates/hds-index/src/lag.rs` | `project_lag` (total/done/pending/errors/building) | правило TR-3.8a: предупреждение о неиндексированных файлах приходит в выдаче поиска |
| `hds agent init` | сброс индекса зоны; предохранитель В-10 (`--force` для «чужого» `anonymize.output_dir`) | предполётная проверка стенда A3 |
| `hds agent token <P>` | токен проекта (в файл — только хэш) | выдать токен для `cline_mcp_settings.json` агента |

### 2.3 Контракты, которые нельзя ломать

* **A1:** `vault.py` — единственный доступ к `anonymize.db`; RPC-набор и `hello.capabilities`
  (`anonymize`, `anonymize_text`, `vault_snapshot`, `vault_rev`, `label_for_value`,
  `merge_entities`, `vault_drop_project`, `keep_terms`, `text_image`, `office_*`) аддитивен;
  `[IMAGE_n]` — тип `IMAGE`; `mode: "agent"` и маркер `hds-agent/<P>/<artifact>`.
* **A2:** `agent.enabled` переключает корень; маршрут `/mcp/agent/<P>` и его 401-семантика;
  `projects=[P]` независимо от `access.enabled` (I5); метка — одно слово в FTS; `pending`
  по ФС+`index.db`.
* **Общие:** `cargo fmt/clippy/test` и python-тесты зелёные; боевые `config.yaml`, `index.db`,
  `data/anonymize.db` не трогать; артефакты — в `tools/parity/out/` (гитигнорится).

### 2.4 Окружение машины (проверено 18.11.2026)

| Что | Значение |
|---|---|
| Node / npm | **v24.18.0** / 11.16.0 (системные; портативного `sidecar/node/` **нет**) |
| SDK | `@cline/sdk` **0.2.0** (закреплён в A0; `@cline/core`, `@cline/shared`, `@cline/llms` — транзитивно) |
| `tavily-mcp` | **0.2.22** (закреплён; `npx @latest` не использовать) |
| Python-воркер анонимизации | `sidecar/venv-anonymize` (3.12.14, onnxruntime-gpu, natasha, pymorphy2) |
| Модели анонимизации | `C:\ProgramData\hermes-disk-search\models\anonymize` |
| GPU | RTX 3060 12 ГБ; резидент `llm_host.exe` — владелец GPU (аренда `anon` через `llm_host`) |
| `1c-buddy` | `%USERPROFILE%\.cline\mcp\1c-buddy\` (+`.venv\Scripts\1c-buddy-mcp.exe`), старт 5–15 с |
| Rust | без `tokio`/async: только потоки и блокирующий I/O |
| `aho-corasick` | **есть в `Cargo.lock` (1.1.5)**, но не в `[workspace.dependencies]` — добавить при реализации фильтра |

### 2.5 Чего ещё нет (не искать, создавать)

* `sidecar/hds_agent/` (нет каталога), `sidecar/node/`;
* crate `hds-agent` (нет);
* `installers/build_agent_sidecar.ps1`;
* ключей A3 в `config.example.yaml` (есть только `enabled`, `root` из A2);
* `data\agent\projects.json`, `registry\P0001.db` (A4), `<P>\agent-config`, `<P>\agent-state`
  (создаёт A3 при старте проекта);
* `README_AGENT.md`, `NOTICE.md`, пункт `agent` в `hds check` (A6).

## 3. Задачи с методикой (TR-3.1…3.10, TR-5.1…5.3, TR-6.1…6.7)

### 3.1. TR-3.1 — рантайм и поставка Node-воркера

**Что сделать.**

1. `sidecar/hds_agent/`: `package.json` (TypeScript + esbuild, **точные** версии
   `@cline/sdk@0.2.0`, `tavily-mcp@0.2.22`; Node ≥ 22 — в `engines`),
   `package-lock.json` (коммитить!), `tsconfig.json`, `src/worker.ts`, `npm run build`
   → `dist/worker.mjs` (esbuild, ESM, `platform=node`, без минификации — нужны читаемые
   стектрейсы).
2. Запуск: `hds-agent` (Rust) ищет Node в порядке `agent.node.path` (ключ конфига) →
   `sidecar/node/node(.exe)` → системный `node`. Портативный Node ставит
   `installers/build_agent_sidecar.ps1` (скачивание zip, распаковка в `sidecar/node/`,
   `npm ci --omit=dev` по lock) — образец: `installers/build_anonymize_sidecar.ps1`.
3. `.gitignore`: добавить `sidecar/hds_agent/node_modules/` (сейчас `node_modules` **не**
   игнорируется, а `dist/` уже игнорируется).
4. Тесты воркера — встроенный `node --test` (Node 24), без новых зависимостей:
   `sidecar/hds_agent/test/*.test.mjs`; `npm test` → `node --test`, чтобы CI A6 мог просто
   вызвать `npm test`.

**Критерий.** `npm ci && npm run build` на чистой копии даёт `dist/worker.mjs`;
`node dist/worker.mjs` отвечает на `hello` (см. §3.13); `npm test` зелёный.

### 3.2. TR-3.2 — окружение процесса агента (D15)

**Что сделать.**

1. `hds-agent` запускает Node **с явным окружением**: `HOME`, `USERPROFILE`, `APPDATA`,
   `LOCALAPPDATA`, `TEMP` → подкаталоги `<P>\agent-config` (создать заранее); ключи
   провайдеров и MCP в env **не** передаются, кроме разрешённых (`PATH`, `SystemRoot`,
   `ComSpec`, `NUMBER_OF_PROCESSORS`, `PROCESSOR_ARCHITECTURE`); `HTTP_PROXY`/`HTTPS_PROXY`/
   `NO_PROXY` — **только** из `agent.network.proxy` (пусто — переменные не ставятся).
2. `cwd` процесса = `<P>\workspace` (иначе встроенные инструменты резолвят относительные пути
   от каталога SDK — П-3); `config.cwd`/`config.workspaceRoot` = тот же путь.
3. `ClineCore.create({ backendMode: "local", clientName: "hds-agent", fetch: egressFetch,
   toolPolicies, telemetry: undefined, featureFlags: undefined })`; каталог данных SDK —
   `<P>\agent-state` (через `HOME`).
4. Плагины: `AgentRuntimePlugin` только из списка hds (`guard`); каталог
   `%USERPROFILE%\.cline\plugins` не читается — проверить тестом TR-3.10 (г).

**Критерий (TR-3.10 в, г, е).** В окружении процесса нет профиля Windows; плагин в
`%USERPROFILE%\.cline\plugins` не загружен; MCP владельца в сессии нет.

### 3.3. TR-3.3 — провайдер, модель, ключи

**Что сделать.**

1. Провайдер/модель — из `projects.json` (TR-8.1: `{id, name, path, provider, model}`).
   A3 читает `projects.json`; минимальный `hds agent project add|list` — для стенда (полный
   CRUD и UI — A5).
2. Ключ — из `data\agent\secrets.json` (`secrets_path_from`), поля `providers.<name>.api_key`,
   `mcp.<name>.*`; в конфиг сессии уходит `config.apiKey`.
3. `baseUrl`: `openrouter` и `openai-compatible` — **с** `/v1`, `anthropic` — **без** `/v1`
   (SDK зовёт `/messages`); повторы SDK идут через тот же `fetch`.
4. Модель по умолчанию для нового проекта — вопрос В-12 (предложение: OpenRouter
   `~deepseek/deepseek-flash-latest`, как в дымовом прогоне A0).

**Критерий.** Прогон на mock-провайдере: 1 вызов `send` → ровно 1 запрос через `egressFetch`
для каждого из трёх провайдеров; ключ не попадает ни в один файл (кроме `secrets.json`),
ни в лог, ни в события.

### 3.4. TR-3.4 — файловые инструменты и хук `hds-guard`

**Что сделать.**

1. `toolPolicies`: включены `read_files`, `search_codebase`, `editor`; **выключены**
   `run_commands` (D16) и `fetch_web_content` (интернет — через `tavily`).
   `apply_patch`, `ask_question`, `submit_and_exit` в headless-наборе **недоступны** —
   политикой не включаются (П-4); вопросы пользователю — только через `host_call ask_user`.
2. `hds-guard` (`beforeTool`): для `read_files`, `search_codebase`, `editor` (и office-)
   каждый путь в аргументах канонизируется: абсолютизация от `cwd`, разрешение `..`,
   junction/symlink через `realpath`, снятие `\\?\`; **отказ** на UNC, `\\?\`-префиксы,
   альтернативные потоки (`file:stream`), имена устройств (`CON`, `NUL`, `COM1`…); результат
   обязан лежать внутри `<P>\workspace`. Нет пути у `search_codebase` → подставить `cwd`.
3. Хук **никогда не бросает**: все ошибки → `{skip: true, reason: "..."}`. Исключение в хуке
   валит весь прогон (в обоих `hookErrorMode` — A0 §3.3), таймаута хука в 0.2.0 **нет** —
   если guard делает `realpath` и может зависнуть, таймаут ставим сами (например 2 с, по
   превышении — `skip`).
4. Отказ доходит до модели текстом причины (на этом держится TR-7.3a: модель понимает,
   почему правку надо оформить новым документом).

**Критерий (TR-3.10 а, б).** `read_files` с `..\..`, абсолютным путём вне workspace, junction
наружу, UNC, `\\?\`, `allowed.txt:hidden`, `NUL` → отказ; абсолютный путь **внутри**
workspace разрешён; в событиях нет путей вне workspace.

### 3.5. TR-3.5 — документы Office через `host_call`

**Что сделать.**

1. `extraTools` воркера: `office_read`, `office_edit`, `office_create` (схемы — в `src/tools.ts`,
   описания по-русски, как у MCP hds). Выполнение — `host_call {method: "office_*", …}` →
   Rust → `hds_anonymize::Client::{office_read, office_edit, office_create}` (RPC уже есть,
   A1): `office_read(path)` → абзацы с номерами + таблицы в markdown + листы/ячейки с адресами;
   `office_edit(path, ops[])` — `replace_text`, `set_cell`, `add_row`, `add_column`,
   `insert_paragraph`, `delete_paragraph`; `office_create(path, kind, content)` — docx/xlsx/pptx.
2. `read_files` для `docx`/`xlsx` — `beforeTool` возвращает `{skip: true, reason: "для
   docx/xlsx используй office_read"}` (подменой `input` инструмент не заменить).
3. `editor` для `docx`/`xlsx` — запрещён хуком (бинарные форматы портит).
4. Текстовые образы `*.pdf.md`, `*.pptx.md`, `*.mpp.md` — **только чтение** (TR-7.3a):
   `editor` по ним запрещён, правки оформляются новым документом (правило в `privacy.md`).
5. `[IMAGE_n]` — метка типа `IMAGE`; office-инструменты её не восстанавливают (в A4 —
   восстановление байтов при «Принять»).

**Критерий.** `office_read` на образе docx из стенда A2 возвращает абзацы и таблицы;
`office_edit` меняет текст абзаца, форматирование/формулы/даты сохраняются; `office_create`
создаёт docx/xlsx/pptx; `editor` по `docx` → отказ с понятной причиной.

### 3.6. TR-3.6 — MCP агента (disk-search, 1c-buddy, tavily)

**Что сделать.**

1. hds **пишет** `<P>\agent-config\.cline\data\settings\cline_mcp_settings.json` при каждом
   старте проекта (формат — A0 §3.7; программной опции «вот серверы» в 0.2.0 нет):
   ```json
   { "mcpServers": {
       "disk-search": { "transport": { "type": "streamableHttp",
                        "url": "http://127.0.0.1:8787/mcp/agent/P0001",
                        "headers": { "Authorization": "Bearer <токен проекта>" } },
                        "timeoutSeconds": 60 },
       "tavily":      { "transport": { "type": "stdio", "command": "<node>",
                        "args": ["<sidecar>/hds_agent/node_modules/tavily-mcp/…"],
                        "env": { "TAVILY_API_KEY": "<из secrets.json>" } } },
       "1c-buddy":    { "transport": { "type": "stdio",
                        "command": "…\\1c-buddy-mcp.exe",
                        "args": ["--env-file", "…\\.env"] } }
   } }
   ```
2. **Токен проекта** — из `data\agent\secrets.json` (`hds-mcp::agent`), маршрут — из
   `mcp_http.path` (по умолчанию `/mcp`) + `/agent/<P>`; проверить, что MCP отвечает и
   `tools/list` = `search_local_files`, `index_status`.
3. `command` с пробелом **не квотируется** SDK (A0: «C:\Program Files\…» разваливается) —
   путь без пробелов, кавычки или `node` из PATH.
4. Имя инструмента для модели — `<server>__<tool>` (`disk-search__search_local_files`) —
   повторить в правилах (`privacy.md`/`1c-domain.md`).
5. Старт долгий: `1c-buddy` 5–15 с, `tavily` 1–3 с — `start` не должен блокироваться дольше
   `agent.request_timeout_sec`.

**Критерий.** На стенде A3: агент вызывает `disk-search__search_local_files` и получает
фрагменты своего проекта; `ask_my_files` в списке инструментов **отсутствует**; вызов
`tavily`-заглушки проходит фильтр `mcp_out` (§3.12).

### 3.7. TR-3.7 — суб-агенты

`agent.subagents` (по умолчанию **false**) → `config.enableSpawnAgent`. A0 §3.5 подтвердил:
суб-агенты наследуют `fetch`, хуки, `toolPolicies`, `hookErrorMode`, `cwd`, их события видны в
`subscribe`, инструменты проходят `beforeTool`. Включать только настройкой; в тестах A3 — два
прогона (выключено/включено), чтобы регресс был виден.

### 3.8. TR-3.8 / TR-3.8a — правила агента

hds кладёт в `<P>\agent-config\rules\` два файла **без путей пользователя и имён проектов**:

* `privacy.md`: синтаксис меток и суффикса (`[PERSON_3|дат]`), «не угадывать значения»,
  «docx/xlsx — только office-инструментами», «изменить документ = править его образ в
  workspace», «`*.pdf.md`/`*.pptx.md`/`*.mpp.md` — только чтение; правки — новым документом
  (TR-7.3a)», «выбор инструмента поиска — по таблице TR-3.8a»;
* `1c-domain.md`: предметная область (§0.1), типовые артефакты проекта внедрения, когда идти
  в `1c-buddy` (функциональность типовых конфигураций, стандарты), когда — в `tavily`,
  и **таблица выбора инструмента поиска** (TR-3.8a): точная строка/метка → `search_codebase`;
  «где говорится о…» → `search_local_files` 2–3 формулировки, `limit` 10–30; найден документ →
  дочитать `office_read`/`read_files` с диапазоном; мало документов → читать целиком;
  предупреждение о `pending` → `search_codebase` + чтение.

**Критерий.** В первом запросе к модели (mock) `system_prompt` содержит правила; в правилах нет
ни одного пути пользователя/имени проекта (проверить поиском строк стенда).

### 3.9. TR-3.9 — жизненный цикл

* Один процесс на проект; живут `agent.max_live_projects` последних проектов (2), выгрузка по
  простою `agent.idle_unload_min` (30 мин). Реестр живых процессов — в `hds-agent` (Rust),
  ключ — `P0001`; выгрузка = `stop` + `dispose` + ожидание выхода, затем удаление записи.
* `start {project, session_id?}` на живом процессе не перезапускает его, а переиспользует
  сессию (одна активная сессия на проект; параллельные ходы — `abort`).
* Возобновление: `start` с **тем же** `sessionId` **и** `initialMessages` (только `sessionId`
  историю не грузит — A0 §3.6); сообщения — из
  `<P>\agent-state\.cline\data\sessions\<id>\<id>.messages.json`.
* `stop` — мягкая остановка (`cline.stop`), `abort` — прерывание хода (`cline.abort`),
  `shutdown` — выход процесса (idle/`hds ui` закрывается).
* Вкладка открывает проект заранее (A5) — в A3 достаточно: `start` возвращает `hello`-ответ
  с `session_id` и списком инструментов, не дожидаясь первой реплики.

**Критерий.** Два проекта: второй `start` не убивает первый; при `max_live_projects = 1`
первый выгружается; после перезапуска `hds ui` `start` с `initialMessages` даёт модели
прежнюю историю.

### 3.10. TR-3.10 — тесты изоляции (mock-провайдер)

Стенд — `tools/parity/agent_a0_sdk/mock_provider.mjs` (форматы OpenAI/Anthropic, сценарии
ошибок, журнал запросов) + `stand.mjs`.

| # | Проверка | Ожидание |
|---|---|---|
| а | `read_files` с `..\..`, абсолютным путём вне workspace, junction наружу, UNC, `\\?\`, `file:stream`, `NUL` | отказ с причиной; путь внутри workspace — разрешён |
| б | события агента (текст, tool_use, tool_result) | ни одного пути вне `<P>\workspace` |
| в | окружение процесса | нет `USERPROFILE` реального профиля, нет ключей провайдеров вне `secrets.json` |
| г | плагин, положенный в `%USERPROFILE%\.cline\plugins` | не загружен (в событиях нет его инструментов) |
| д | все HTTP-запросы (включая повторы и суб-агентов) | только через `egressFetch` (счётчик заглушки = числу запросов mock) |
| е | MCP из `~/.cline` | в сессии отсутствуют |
| ж | (доп.) относительный путь в `read_files` | попадает в `<P>\workspace` (П-3), не в каталог SDK |
| з | (доп.) `disk-search` на маршруте A2 | `tools/list` = `search_local_files`+`index_status`; `ask_my_files` нет |

### 3.11. Шлюз реплик и событий (TR-5.1…TR-5.3)

**Реплика (TR-5.1/5.2).** `send {text}` → Rust: `Client::anonymize_text(P, text)` →
(при `agent.preview_outgoing`, по умолчанию включён) событие `preview` пользователю →
`cline.send`. Вложения: файл из папки проекта → в реплику подставляется путь **образа**
(реестр — A4; в A3 — по `<P>\workspace` + маркер `hds-agent/<P>/<artifact>` и имени файла);
файл вне папки проекта → разовая анонимизация в `<P>\workspace\_attachments\` (через
`anonymize_agent` с `artifact_id`). Абсолютные пути Windows/UNC в тексте распознаются
регуляркой и заменяются путём образа или `[PATH_n]`.

**События (TR-5.3).** Каждое событие `subscribe` → Rust деанонимизирует потоково
(`StreamDeAnonymizer`): метки → значения по снимку vault (форма по суффиксу `|дат`, TR-2.4;
неизвестная метка — как есть с пометкой), пути `<P>\workspace\…` → пути папки проекта по
реестру (A4; в A3 — оставить путь образа и пометить). Переключатель «как видит модель» (A5) —
событие без подстановки; в A3 событие содержит оба поля (`text` и `raw`), чтобы A5 не
переделывал протокол.

### 3.12. Фильтр исходящего (TR-6.1…6.7) — Rust

**Словарь.** Снимок vault (`Client::snapshot`, `rev`) пересобирается при смене `rev`
(`vault_rev`) и не реже `agent.filter.max_stale_sec`; Aho-Corasick (добавить
`aho-corasick = "1"` в workspace deps — 1.1.5 уже в `Cargo.lock`):

* (а) формы сущностей проекта → метка проекта с суффиксом;
* (б) прочие сущности/`others` → `agent.filter.unknown: label | block` (по умолчанию `label`) —
  новая метка проекта через `label_for_value` или блокировка;
* (в) системные строки (имя пользователя Windows, компьютер, профиль, путь папки проекта и
  подпапок, имена проектов из `projects.json`) → `[SYS_n]`;
* (г) **секреты** (ключи провайдеров/MCP, токены `/mcp/agent`) — в точках `model` и `mcp_out`
  запрос **блокируется** (ошибка агенту + событие пользователю).

Значения из `keep_terms` в словарь не входят; короче 3 символов — не входят; сравнение без
регистра, по границам слова, с нормализацией `ё→е`, неразрывных пробелов, кавычек, дефисов.

**Точки:** `model` — тело каждого запроса провайдеру (`messages`, system, `tools`, аргументы
`tool_calls`; ответ модели не фильтруется); `mcp_out` — аргументы `beforeTool` внешних MCP;
`mcp_in` — результат `afterTool` (чистятся только **известные** базе значения — TR-6.3).

**Fail-closed (TR-6.5).** Нет снимка, фильтр упал или снимок старше `max_stale_sec` →
`egressFetch` возвращает агенту `privacy_filter_unavailable`, запрос не уходит.

**Аудит (TR-6.4).** `hds_core::audit`, событие `agent_egress_redacted`: проект, точка, тип,
метка, источник словаря (а/б/в/г) — **без значения**; срабатывание (а) в точке `model` —
повод для «Очистить историю».

**Журнал (TR-6.7).** `agent.egress_log: true` → тела запросов **после** фильтра в
`<P>\agent-state\egress\` (по умолчанию выключен).

**«Очистить историю» (TR-6.6).** Остановить сессию → прогнать `<id>.messages.json` и `<id>.json`
тем же словарём (значения → метки) с бэкапом файла → `start` с тем же `sessionId` и
`initialMessages`. `sessions.db` текстов не содержит — чистить не нужно.

### 3.13. Протокол stdio §4.3

NDJSON, JSON-RPC 2.0, двунаправленный (у `hds_extract::Worker` только «запрос → ответ» —
для агента нужен свой цикл чтения со встречными запросами):

| Направление | Сообщение |
|---|---|
| hds → агент | `hello` (версия протокола, capabilities), `start {project, session_id?, …}`, `send {text}`, `abort`, `stop`, `shutdown` |
| агент → hds | нотификация `event {…}` (текст, рассуждения, `tool_use`, `tool_result`, `ask_user`, `ended`) |
| агент → hds | запрос `host_call` с `id` с префиксом `h`: `egress_check {kind: model\|mcp_in\|mcp_out, body}`, `office_read\|office_edit\|office_create {path, …}`, `ask_user {question}` |

Идентификаторы `host_call` — `h…`, чтобы Rust не путал их с `id` своих запросов. Ошибки —
JSON-RPC `error` с `code`/`message` (без значений PII в `message`). Успех прогона определять по
событиям `agent_event.done`/`ended`, **не** по возвращаемому значению `send` (в локальном
runtime обычно `undefined`), и проверять, что текст/usage не пусты (риск «успех при пустом
ответе» — A0 §5.1).

## 4. Порядок работ и стоп-точки

1. **Стоп-точка 0.** `git status`, ветка `agent-privacy-zone`, HEAD `3bfd1089`, дерево чистое.
   Чужие незакоммиченные правки — не трогать, согласовать. Проверить: `node --version`,
   `tools/parity/out/agent_a0/sdk/node_modules` на месте (иначе `npm ci` в копии).
2. **Каркас `hds-agent` (Rust).** Процесс-менеджер (spawn с окружением TR-3.2), NDJSON-цикл с
   `host_call`, реестр живых проектов, события в канал. Вход без вкладки UI — временная команда
   `hds agent run --project P0001` (вопрос В-16).
3. **`sidecar/hds_agent`.** `hello`/`start`/`send`/`stop`, провайдер + `toolPolicies` +
   `hds-guard` → первый e2e на mock-провайдере (`read_files` внутри workspace).
4. **Office + MCP.** `extraTools`/`host_call`, `cline_mcp_settings.json` с `disk-search`
   (маршрут A2), `tavily`, `1c-buddy`; проверить `tools/list` и отсутствие `ask_my_files`.
5. **Шлюз + фильтр.** TR-5.1…5.3, TR-6.1…6.7, «Очистить историю».
6. **Жизненный цикл** (TR-3.9) и суб-агенты (TR-3.7).
7. **TR-3.10** (а–з) + e2e-приёмка: агент читает образ, ищет через `disk-search`, вызывает
   заглушку `tavily`, пишет файл; I2/I7 на заглушках.
8. **Установщик и документация:** `installers/build_agent_sidecar.ps1`, `sidecar/README.md`
   (Node-воркер), раздел в `README.md`; `RELEASE_NOTES` — раздел «Фаза A3». `README_AGENT.md`,
   `NOTICE.md`, `hds check` — A6.

**Стоп-точки (остановиться и согласовать):**

* `disk-search` с маршрута A2 не отвечает из SDK (401/транспорт) — блокер TR-3.6;
* guard вынужден бросать исключение (валит прогон) — пересмотреть набор инструментов/политику;
* фильтр не может гарантировать fail-closed (нет снимка vault, воркер недоступен) — решение
  владельца: агент не отвечает вообще или отвечает с предупреждением;
* ключ провайдера/MCP попадает в файл вне `secrets.json` (в т.ч. `cline_mcp_settings.json`) —
  остановиться: это В-1 и I7;
* `1c-buddy`/`tavily` не стартуют под подменённым `HOME` — согласовать способ (явные пути,
  копия программы в поставку).

## 5. Критерии готовности A3 (план §6 + уточнения)

- [ ] `sidecar/hds_agent` собирается: `npm ci --omit=dev` + `npm run build` → `dist/worker.mjs`;
      версии `@cline/sdk` и `tavily-mcp` закреплены lock-файлом; `npm test` зелёный.
- [ ] `hds-agent` (Rust) запускает процесс с очищенным окружением, `cwd = <P>\workspace`,
      `HOME`/`APPDATA` → `<P>\agent-config`, каталог данных SDK → `<P>\agent-state`.
- [ ] Прогон на mock-провайдере: реплика → `anonymize_text` → `send` → текст в событиях
      деанонимизирован; в запросе к модели — только метки.
- [ ] `hds-guard`: матрица путей TR-3.10 (а) отклонена, путь внутри workspace разрешён;
      хук не бросает исключений (включая «путь не существует»).
- [ ] `run_commands` и `fetch_web_content` отсутствуют в `tools`; `editor` по `docx`/`xlsx` и по
      `*.pdf.md`/`*.pptx.md`/`*.mpp.md` отклонён; `office_*` работают через `host_call`.
- [ ] MCP: `disk-search` (маршрут `/mcp/agent/<P>`, токен проекта) и заглушки
      `tavily`/`1c-buddy` видны в `tools`; `ask_my_files` нет.
- [ ] Фильтр: значение сущности в реплике/инструменте заменено меткой; ключ провайдера в тексте
      → блокировка с `privacy_filter_unavailable`; аудит `agent_egress_redacted` без значения.
- [ ] `agent.subagents: false` по умолчанию; при `true` суб-агент проходит через `egressFetch`
      и хук.
- [ ] Жизненный цикл: `max_live_projects`, `idle_unload_min`, возобновление через
      `initialMessages`.
- [ ] Тесты: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace -- --test-threads=4`, python-тесты
      (`sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar`),
      `npm test` в `sidecar/hds_agent` — зелёные.
- [ ] **Приёмка A3 (e2e, mock-провайдер):** образ docx из стенда A2 → агент читает его
      `office_read`, ищет через `disk-search`, вызывает заглушку `tavily`, пишет файл в
      workspace; I2/I7 на заглушках; профиль `%USERPROFILE%\.cline` до/после не изменён.

## 6. Грабли (из A0/A1/A2 — проверено фактом)

1. **«Успех» при пустом ответе.** 429/500 с исчерпанными повторами → `ended.reason =
   "completed"` и пустой текст. Проверять наличие текста и `usage`, иначе UI покажет
   «готово» без ответа.
2. **Исключение в хуке валит прогон** (в обоих `hookErrorMode`). Guard обязан быть
   безотказным: все ошибки → `{skip: true, reason}`.
3. **Таймаута хука нет.** Если guard может зависнуть (`realpath` на сетевом пути) — свой
   таймаут внутри хука.
4. **Относительные пути резолвятся от `cwd` процесса**, а не от `config.cwd`: процесс надо
   запускать из `<P>\workspace`.
5. **`command` в MCP-настройках не квотируется** («C:\Program Files\…» → код 1). Путь без
   пробелов, кавычки или `node` из PATH.
6. **MCP задаётся только файлом** (`<HOME>/.cline/data/settings/cline_mcp_settings.json`);
   программной опции нет.
7. **`toolPolicies` не добавляют инструменты**, а только выключают/подтверждают;
   `apply_patch`, `ask_question`, `submit_and_exit` в headless-наборе недоступны — вопросы
   только через `host_call ask_user`.
8. **История:** `start` с одним `sessionId` историю не грузит — нужен `initialMessages`;
   тексты лежат в `<id>.messages.json`, в `sessions.db` их нет; бэкап перед перезаписью —
   наша ответственность.
9. **SDK запускает `git`** (метаданные сессии) — учесть в замере старта; в `workspace` без
   `.git` вызов безвреден.
10. **Кэш обработчика `anonymize`** по содержимому: повтор одного текста — 3–4 мс. Для замеров
    брать уникальные тексты, для реплик кэш допустим.
11. **Warmup анонимизации 11–30 с** (GPU, холодный кэш ОС) — прогревать воркер заранее
    (вкладка открывает проект до первой реплики).
12. **Снимок vault содержит исходные значения** — держать только в памяти Rust, не писать в
    журналы/события/конфиги (TR-1.6, I8).
13. **Токен `/mcp/agent` в `cline_mcp_settings.json`** лежит открытым текстом: ACL каталога
    настроек — В-1/A6; в A3 не логировать файл и не отдавать его в события.
14. **`hds agent init`** удаляет выход анонимизации только внутри `<state_dir>\data`/`<agent.root>`
    (В-10); на стендах с «чужим» `anonymize.output_dir` нужен `--force`.
15. **Метки** — `[PERSON_3|дат]` и `[PERSON_3]` один токен FTS (`xperson3x`), но **разные**
    формы при деанонимизации; разбор — только `hds_core::labels`.
16. **Async в Rust нет** — только потоки и блокирующий I/O; события агента передавать через
    `std::sync::mpsc` (как в `hds_index::watch`), не тащить `tokio`.

## 7. Правила коммита

* Перед правками — `git status` и ветка: ожидается `agent-privacy-zone`, HEAD `3bfd1089`,
  дерево чистое. Чужие незакоммиченные правки **не откатывать, не стейджить, не переписывать**;
  появились и мешают — остановиться и согласовать.
* Коммиты — явным списком путей, по смыслу: **sidecar (`npm`/TS) → crate `hds-agent` →
  фильтр → шлюз → установщик → docs**. Один коммит — одна причина.
* **Не коммитить** `sidecar/hds_agent/node_modules/`, `dist/` (уже в `.gitignore` — добавить
  `node_modules`), `sidecar/node/` (портативный Node — в `.gitignore`), скриншоты и логи.
  `package-lock.json` — **коммитить**.
* Push — только по просьбе владельца.
* Боевые `config.yaml`, `index.db`, `data/anonymize.db` не трогать; все стенды и временные
  базы — в `tools/parity/out/` (гитигнорится).
* Контракты A1/A2 не ломать: RPC аддитивны, `vault.py` — единственный доступ к базе меток,
  маршрут `/mcp/agent/<P>` и метки в FTS остаются как в A2.

## 8. Вопросы владельцу (В-12…В-16)

| # | Вопрос | Предложение исполнителя |
|---|---|---|
| В-12 | какой провайдер и модель ставить в `projects.json` при создании проекта | OpenRouter + `~deepseek/deepseek-flash-latest` (как в дымовом прогоне A0); вкладка A5 даст выбор |
| В-13 | `agent.filter.unknown`: `label` (новая метка проекта) или `block` | `label` (не терять смысл документа); `block` — для параноидального режима |
| В-14 | `agent.filter.max_stale_sec` по умолчанию и что видит пользователь при недоступном воркере | 300 с; при недоступном воркере — fail-closed: агент не отвечает, событие «фильтр недоступен» с подсказкой «запустите `llm_host load anon`» |
| В-15 | `1c-buddy` брать из `%USERPROFILE%\.cline\mcp\1c-buddy\` (каталог программирующего Cline) или ставить копию в поставку | копия в `sidecar/` (иначе зависимость от чужого каталога и его обновлений); путь — ключ `agent.mcp.onec_buddy.command` |
| В-16 | значения жизненного цикла и журнала: `max_live_projects = 2`, `idle_unload_min = 30`, `egress_log = false`; нужна ли временная точка входа `hds agent run --project <P>` до вкладки A5 | подтвердить значения; временную команду — да (иначе A3 нечего запускать без UI) |

## 9. Промт для нового чата (копировать целиком)

Ты — ведущий разработчик проекта hermes-disk-search (рабочая папка
C:\Users\Sasha\hermes-disk-search, Windows, PowerShell 5.1). Отвечай по-русски.

ЗАДАЧА: выполнить фазу A3 плана PLAN_AGENT_PRIVACY_ZONE.md — агент `hds_agent` в анонимной
зоне: Node/TypeScript-воркер на `@cline/sdk` 0.2.0 (TR-3.1…TR-3.10), crate `hds-agent`
(процесс, `host_call`, шлюз реплик TR-5.1…5.3, словарный фильтр исходящего TR-6.1…6.7,
деанонимизация событий), установщик `installers/build_agent_sidecar.ps1`. Фазы A0, A1 и A2
закрыты — их результаты обязательны к учёту.

СНАЧАЛА ПРОЧИТАЙ ЦЕЛИКОМ:
1. AGENT_A3_HANDOFF_PROMPT.md — карта фазы: состояние репозитория (§2), задачи с методикой
   (§3), порядок работ и стоп-точки (§4), критерии готовности (§5), грабли (§6), правила
   коммита (§7), вопросы владельцу (§8).
2. PLAN_AGENT_PRIVACY_ZONE.md — §3 (D1, D8, D10, D11, D12, D16, D17, D20, D22, D23), §4.2,
   §4.3 (канал hds ui ↔ hds_agent), §4.5, §4.6, §5.3 (TR-3.1…TR-3.10), §5.5 (TR-5.1…5.3),
   §5.6 (TR-6.1…6.7), §5.8 (TR-8.1/8.3/8.5), §5.9 (I1–I8), §6 (строка A3), §9.
3. tools/parity/AGENT_SPIKES.md — §3.1–3.8, §3.11 (П-1…П-14), §4 (точные имена API), §5.
4. Код: crates/hds-anonymize/src/{client,protocol,settings}.rs;
   sidecar/hds_anonymize/{vault,handlers,file_parser}.py; crates/hds-mcp/src/{agent,http,
   server,schema,tools}.rs; crates/hds-core/src/{labels,roots,access}.rs;
   crates/hds-extract/src/worker.rs; crates/hds-ui/src/lib.rs (стиль: потоки, без async);
   crates/hds-cli/src/{main,support}.rs; .gitignore.
5. Стенд A0: tools/parity/agent_a0_sdk/ (mock_provider.mjs, stand.mjs, s1…s7) и рабочая копия
   tools/parity/out/agent_a0/sdk/ (node_modules).
Затем составь и покажи короткий план (что, в каком порядке, стоп-точки) и сразу начинай.

ГЛАВНОЕ:
* Перед правками проверь git status и ветку: ожидается agent-privacy-zone, HEAD 3bfd1089,
  дерево чистое. Чужие незакоммиченные правки не откатывать, не стейджить, не переписывать;
  если появились и мешают — остановись и согласуй с владельцем.
* Node — только с очищенным окружением (TR-3.2): HOME/USERPROFILE/APPDATA/LOCALAPPDATA/TEMP →
  <P>\agent-config, cwd процесса = <P>\workspace, каталог данных SDK = <P>\agent-state, прокси —
  только из agent.network.proxy. Ключи провайдеров/MCP — только в data\agent\secrets.json.
* Хук hds-guard обязан быть безотказным (исключение валит прогон) и с собственным таймаутом;
  область — строго <P>\workspace (матрица опасных путей TR-3.4/TR-3.10а).
* run_commands и fetch_web_content выключены; вопросы пользователю — только host_call ask_user;
  apply_patch/ask_question/submit_and_exit политикой не включаются.
* docx/xlsx — только office_* через host_call (Rust → Python); editor по docx/xlsx и по
  *.pdf.md/*.pptx.md/*.mpp.md — отказ с причиной (TR-3.5, TR-7.3a).
* MCP — файлом <P>\agent-config\.cline\data\settings\cline_mcp_settings.json: disk-search на
  маршрут http://127.0.0.1:8787/mcp/agent/<P> с токеном проекта (hds agent token), tavily,
  1c-buddy. Имя инструмента для модели — <server>__<tool>. ask_my_files на маршруте нет.
* Фильтр исходящего — fail-closed: в точках model и mcp_out запрос с секретом или при
  недоступном снимке vault не уходит (privacy_filter_unavailable); аудит без значений.
* Деанонимизация событий — только метки → значения (hds_core::labels, суффикс |дат) и пути
  workspace → пути проекта; исходные значения не писать в журналы, события и конфиги.

ПРОВЕРКИ И ЗАВЕРШЕНИЕ:
* cargo fmt --check, cargo clippy --workspace --all-targets -- -D warnings,
  cargo test --workspace -- --test-threads=4; npm ci --omit=dev + npm run build + npm test
  в sidecar/hds_agent; python-тесты —
  sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar.
* TR-3.10 (a–з): матрица путей, окружение без профиля, плагин из ~/.cline не загружен, все
  HTTP-запросы через egressFetch, MCP владельца отсутствуют, относительный путь в workspace,
  tools/list disk-search = search_local_files + index_status.
* e2e на mock-провайдере: агент читает образ docx (office_read), ищет через disk-search,
  вызывает заглушку tavily, пишет файл в workspace; I2/I7 на заглушках; профиль
  %USERPROFILE%\.cline до/после не изменён.
* Обнови sidecar/README.md (Node-воркер), раздел в README.md, RELEASE_NOTES (раздел «Фаза A3»)
  и, при необходимости, §2–§5 этого файла.
* В конце дай сводку: что сделано, какие тесты/замеры, какие решения приняты, какие вопросы
  к владельцу (В-12…В-16).
