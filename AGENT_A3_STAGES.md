# AGENT_A3_STAGES.md — промежуточный план фазы A3 (под-этапы)

> **Зачем.** Исходная карта `AGENT_A3_HANDOFF_PROMPT.md` вела к результату одним длинным
> участком: Rust-крейт → Node-воркер → CLI → установщик → приёмка. Первый проверяемый итог
> появлялся только в конце. Этот файл режет остаток A3 на **короткие под-этапы**. Каждый
> занимает **один чат** и заканчивается **зелёным тестом, который виден снаружи**, и
> коммитом.
>
> Карта фазы (требования, грабли, критерии §5) остаётся в `AGENT_A3_HANDOFF_PROMPT.md`,
> здесь — только порядок, границы под-этапов и состояние.
>
> **Дата среза:** 19.11.2026. **Ветка:** `agent-privacy-zone`. Состояние: A3.0 — `f10a92e8`,
> `a8dc5b51`; A3.1 — `2efe8043` (fix), `8e26d0f3` (test); A3.2 — `6ef028af` (feat),
> `8204e384` (test); A3.3 — `be72b01d` (feat), `74d1aacb` (test); A3.4 — `55abc33b` (feat),
> `1b80fc3f` (test); A3.5 — `babb2223` (feat), `3e9d61f7` (test); A3.6 — `9db20e55` (test) плюс
> docs-коммиты. Дерево чистое, push не делался.

## 1. Что уже сделано и проверено (A3.0…A3.6)

| Коммит | Содержание |
|---|---|
| `f10a92e8` | крейт `crates/hds-agent`: `settings`, `projects`, `secrets`, `paths` (матрица TR-3.10а), `env` (HOME=`agent-config`, `CLINE_DATA_DIR`=`agent-state\.cline\data`, `CLINE_MCP_SETTINGS_PATH`), `filter` (Aho-Corasick, fail-closed, аудит), `deanon`, `protocol` (§4.3), `worker` (NDJSON + `host_call`), `gateway`, `mcp`, `rules`, `history`; `hds-core::labels::find_labels` |
| `a8dc5b51` | Node-воркер `sidecar/hds_agent` (TypeScript + esbuild → `dist/worker.mjs`, `@cline/sdk` 0.2.0, `tavily-mcp` 0.2.22, lock-файл); тесты `node --test` |

**Проверки на срезе (все зелёные):**

* `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`;
* `cargo test --workspace -- --test-threads=4`: 728 тестов, 0 падений (`hds-agent` — 51);
* python: `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests -t sidecar`:
  307 OK, 1 пропущен;
* `sidecar/hds_agent`: `npm run typecheck` (tsc), `npm run build`, `npm test` — 8/8;
* чистая копия: `npm ci` + `npm run build` собирают `dist/worker.mjs`.

**Подтверждено фактом — первый e2e** (`test/e2e_basic.test.mjs`, mock-провайдер):

* `hello` → `ready {protocol: 1, sdk: "0.2.0"}`; SDK принимает конфигурацию `start` воркера
  (`toolPolicies`, `extraTools`, `hooks`, `plugins`, `checkpoint`);
* ход `send` → `read_files` внутри workspace → ответ модели. 2 запроса к модели, оба прошли
  `host_call egress_check {kind: model}`. Во втором запросе (с результатом инструмента) ушла
  метка `[PERSON_1]`, исходное значение — нет;
* в `tools` запроса к модели: `read_files, search_codebase, editor, office_read, office_edit,
  office_create, ask_user`. `run_commands` и `fetch_web_content` отсутствуют;
* события: `iteration, usage, tool_use, tool_result, text_delta, text, agent_done, status,
  ended, turn_end`;
* сессии SDK лежат в `<P>\agent-state\.cline\data\sessions`, в `agent-config` их нет.

**Исправлено попутно:**

* нумерация `[SYS_n]`: фильтр и деанонимизатор берут один список
  `filter::sys_values(system, projects)`;
* профиль агента: раньше `HOME` указывал на `agent-config\home`, а `cline_mcp_settings.json`
  писался в `agent-config\.cline\…` — MCP не нашёлся бы.

**На срезе A3.0 НЕ было проверено фактом** (закрыто в A3.1 — список ниже):

* `beforeModel` с заменой `tools` (вырезание `*__ask_my_files`);
* `plugins: {'*': {enabled: false}}` — плагин из `<HOME>\.cline\plugins` не грузится;
* MCP через файл по `CLINE_MCP_SETTINGS_PATH`, фильтр `mcp_out`/`mcp_in`;
* `office_*` и `ask_user` в живом прогоне;
* fail-closed (отказ `egress_check` → 0 запросов к модели);
* `abort`, суб-агенты (`subagents: true`), возобновление через `history`
  (`initialMessages`);
* поле `tools` в ответе `start` — **статический список**, а не фактический. Фактический
  список приходит в `turn_end.tools` (из `beforeModel`).

**Закрыто в A3.1** (всё перечисленное выше проверено тестами, см. §1.1 и §3):

* `npm test` = 23 теста, 0 падений, ≈ 39 с (`node --test`, mock-провайдер, MCP-заглушка);
* `beforeModel`: в `tools` запроса и в `turn_end.tools` есть `stub__search_local_files`,
  `stub__leak`, нет `stub__ask_my_files`; имя с хеш-суффиксом
  (`…__ask_my_files_3deb181b`, так SDK называет MCP-инструменты Agent Plugin) тоже режется;
* MCP из файла по `CLINE_MCP_SETTINGS_PATH`: `mcp_out` заменил значение меткой **до** вызова
  заглушки (`[PERSON_1]` в журнале заглушки), `mcp_in` заменил значение в ответе; оба вызова
  видны в журнале стенда; секрет в аргументах блокирует вызов, секрет в реплике — 0 запросов к
  модели, `turn_end.ok=false`, `error` содержит `privacy_filter_unavailable`, следующая
  реплика снова работает;
* `read_files` по `..\outside`, UNC, `NUL` → «hds-guard: …» в следующем запросе к модели,
  содержимое outside не ушло; относительный путь читается из workspace;
  `report.docx` → «используй office_read»; `editor` по `*.pdf.md` → отказ;
* `office_read/edit/create` и `ask_user` → `host_call` с путём как дал агент, `ops` массивом,
  результат дошёл до модели; путь вне workspace — `host_call` не вызван;
* fail-closed: отказ `egress_check {kind:model}` → mock получил 0 запросов, `turn_end.ok=false`,
  в `error` есть `privacy_filter_unavailable`;
* `abort` во время хода → `turn_end {ok:false, reason:"aborted"}`, процесс жив;
* `subagents: false` → нет `spawn_agent`; `true` → есть, запрос суб-агента прошёл
  `egress_check` (число запросов mock = числу `egress_check` kind=model);
* `history` + тот же `session_id` → первый запрос к модели содержит прошлую реплику;
* `start.result` теперь `declared_tools` (старое `tools` снято, см. §4);
* ключ провайдера (`start.provider.api_key`) и ключ из env родителя не появляются ни в одном
  файле под `agent-state`/`agent-config`, реальный `<домашний каталог>\.cline` не упоминается
  ни в запросах, ни в событиях;

### 1.1. Тесты A3.1 (карта: файл → проверяемый факт)

Все — `node --test`, сценарии — `test/fixtures/` (mock-провайдер, MCP-заглушка, стенд
`harness.mjs` в роли Rust). Провайдер — `127.0.0.1:0`, каталоги проектов — `os.tmpdir()`,
удаляются в `t.after`.

| Файл | Факт |
|---|---|
| `test/e2e_basic.test.mjs` | базовый ход + `start.result.declared_tools` (старое `tools` снято) |
| `test/mcp.test.mjs` | инструменты заглушки в запросе и `turn_end.tools`, `ask_my_files` вырезан; `mcp_out`/`mcp_in` через `egress_check` |
| `test/secret.test.mjs` | секрет в аргументах MCP — вызова нет; секрет в реплике — 0 запросов, `turn_end.ok=false`, воркер жив |
| `test/failclosed.test.mjs` | отказ `egress_check {kind:model}` → 0 запросов к mock, ошибка с `privacy_filter_unavailable` |
| `test/guard_live.test.mjs` | отказы guard (пути, docx, `*.pdf.md`) доходят до модели текстом; outside не утёк |
| `test/office.test.mjs` | `office_*`/`ask_user` → `host_call` с верным payload; путь вне workspace — вызова нет |
| `test/env.test.mjs` | ключ из env родителя не передан; реальный профиль не упоминается; auth — только из `start` |
| `test/isolation.test.mjs` | плагин профиля найден и отклонён политикой (его инструмента нет), MCP по «умолчанию» и Agent Plugin не загружены; фикстуры валидны (положительный контроль) |
| `test/retry.test.mjs` | 429 → повтор, число запросов mock = числу `egress_check kind=model` |
| `test/abort.test.mjs` | `abort` → `turn_end {ok:false, reason:"aborted"}`, процесс жив |
| `test/subagents.test.mjs` | `subagents` false/true; запрос суб-агента через `egress_check` |
| `test/history.test.mjs` | `history` + `session_id` → прошлая реплика в первом запросе; api_key нет в файлах |
| `test/guard.test.mjs` | матрица путей, политика форматов, `collectPaths`, `withTimeout`, `isForbiddenToolName` (все формы `ask_my_files`) |

### 1.2. A3.2: Rust ↔ настоящий воркер (подтверждено фактом)

`cargo test -p hds-agent` = **56** тестов (54 unit + 2 интеграционных в
`crates/hds-agent/tests/gateway_worker.rs`), ≈ 3 с. Предпосылки (node + `dist/worker.mjs`)
проверяются в тесте: нет — печатается причина и тест выходит (как тесты `worker.rs`).

* `Gateway::spawn` → `hello` → `open` → `send` → события → `turn_end` поверх живого
  `dist/worker.mjs` (не заглушки);
* `open` читает **`declared_tools`** (после переименования в A3.1) — ровно 7 объявленных имён;
* `egress_check` проходит **настоящий `Filter`** со снимком vault, собранным без Python
  (структура `Snapshot` вручную, `GatewayHost::without_client`): «Иванов Иван» в теле запроса
  заменён на `[PERSON_1]`, mock-провайдер значения **не получил**;
* `GatewayEvent::Redacted` содержит только `Hit { label, source }` — значения нет
  (проверяется по `Debug` и по структуре);
* события деанонимизированы: `[PERSON_1]` → «Иванов Иван», `[SYS_1]` → `DESKTOP-A32`,
  путь workspace → путь папки проекта; `TurnEnd` отдаёт `raw` (метки) и `text` (значения)
  и фактический `tools` (есть `read_files`/`office_read`/`ask_user`, нет `run_commands`);
* fail-closed на живом воркере: без снимка `egress_check` отвечает
  `privacy_filter_unavailable`, mock получил **0** запросов, `TurnEnd { ok: false,
  reason: "error" }`, процесс жив;
* `send` без Python-клиента отказывает (`GatewayError::Anonymize`), `send_anonymized`
  отправляет уже обезличенный текст — этот режим (egress-only) нужен стендам и тестам.

### 1.3. A3.3: точка входа CLI (подтверждено фактом)

`cargo test -p hds-cli --test agent_run` — 3 теста, ≈ 3 с (весь воркспейс — 737 тестов,
было 733; 0 падений). Стенд: `HDS_ROOT` = временный каталог, mock-провайдер, mock-воркер
анонимизации `crates/hds-anonymize/tests/mock_worker.py`, настоящий `dist/worker.mjs`.

* `hds agent project add P0001 --name … --path DIR [--provider …] [--model …]` пишет
  `data\agent\projects.json` (путь — из `state_dir`, в тестах `HDS_ROOT`); `project list`
  и `list --json` печатают реестр (проверено через сам бинарь `hds`);
* `hds agent run --project P --dry-run` печатает план (JSON) и не запускает ни Node, ни
  Python: каталоги `workspace|agent-config|agent-state`, node (`agent.node.path` →
  `sidecar/node` → PATH), `dist/worker.mjs`, провайдер/модель/base_url, файл MCP, фильтр;
  ключа провайдера и токена проекта в плане нет; `plan()` ничего не создаёт на диске;
* `agent run` собирает сессию: `env::prepare_dirs`, `rules::write_rules` (privacy.md,
  1c-domain.md), `mcp::write_settings` (маршрут `/mcp/agent/P0001`), снимок vault через
  `Client::snapshot`, `Filter` со снимком и `secret_strings`, `env::build`,
  `Gateway::spawn` → `hello` → `open` → `send`;
* на mock-провайдере: реплика с «Иванов Иван» ушла модели меткой `[PERSON_1]` (провайдер
  значения не получил), ответ деанонимизирован («Готово: Иванов Иван прочитан.»),
  срабатывание фильтра на теле запроса с содержимым файла видно событием `redacted`,
  успех — по `turn_end {ok: true}`, `tools` содержит `read_files`;
* ключ провайдера не появился ни в одном файле `agent-config`/`agent-state` (только
  `secrets.json`); сессии SDK — в `agent-state\.cline\data\sessions`;
* `run` без `--project` → код 2, неизвестный проект → код 1; `--quiet` печатает только
  текст ответа, `--json` — NDJSON-события;
* срабатывания фильтра уходят в журнал `audit.enabled` (desktop-профиль — выключен).

### 1.4. A3.4: жизненный цикл (подтверждено фактом)

`cargo test -p hds-agent` = **68** тестов (63 unit, из них 8 — `registry`; 2 —
`gateway_worker`; 3 — `lifecycle_worker`), ≈ 11 с. `cargo test -p hds-cli --test
agent_run` = **5** тестов, ≈ 7 с. Воркспейс — **750** тестов, 0 падений; `npm test`
в `sidecar/hds_agent` — 23/23 (не менялся).

| Файл | Факт |
|---|---|
| `crates/hds-agent/src/registry.rs` (unit, Node-заглушка) | 8 тестов: дефолты из `AgentSettings` (2 проекта, 30 мин); переиспользование (PID тот же, второго `start` нет); `max_live = 1` выгружает первый процесс (заглушка пишет файл выхода — процесс завершён); `max_live = 2` — оба живы; жертва — LRU; `reap_idle` только просроченные + переоткрытие; упавший воркер переоткрывается; no-op на неизвестном проекте |
| `crates/hds-agent/tests/lifecycle_worker.rs` (настоящий `dist/worker.mjs` + mock-провайдер) | 3 теста: реестр (2 проекта, ход после второго `start`, переиспользование без `spawn`, выгрузка LRU); возобновление после «перезапуска hds»; «Очистить историю» end-to-end |
| `crates/hds-cli/tests/agent_run.rs` | +2: `run --session` (прошлая реплика в первом запросе, ошибка при отсутствующей истории); `history clear` через бинарь (0/1/2, бэкапы, последняя сессия) |

**Что проверено:**

* реестр `AgentRegistry` (`hds-agent`): `max_live` и простой берутся из
  `AgentSettings` (`agent.max_live_projects` = 2, `agent.idle_unload_min` = 30);
  время — параметр (`now`), фоновых потоков и «спящих» тестов нет; выгрузка =
  `stop` + `shutdown` (ожидание выхода ≤ 5 с) + удаление записи;
* при `max_live = 1` открытие второго проекта завершает процесс первого
  (заглушка успевает записать свой PID в файл выхода), запись уходит; при 2 —
  оба живы, второй `start` первого не убивает; жертва переполнения — проект с
  самым старым `last_used` (LRU), а не первый по порядку;
* переиспользование: повторный `get_or_open` не вызывает `spawn`, PID тот же,
  второго `start` в журнале заглушки нет (значит и `initialMessages` не
  передаются повторно); упавший процесс (`is_closed`) переоткрывается;
* `reap_idle(now)` выгружает только проекты, бездействовавшие дольше `idle`, и не
  трогает свежие; после выгрузки проект открывается заново (новый PID, новый
  `start`);
* возобновление на **настоящем** воркере: `start` с тем же `session_id` **и**
  `initialMessages` из `history::read_messages` — первый запрос к модели содержит
  прошлую реплику; `session_id` **без** `initialMessages` историю не грузит
  (регресс A0 §3.6 зафиксирован тестом);
* «Очистить историю» end-to-end: `Gateway::clear_history` переписывает
  `<id>.messages.json` и манифест, рядом остаются бэкапы `.json.bak`; значения в
  файлах нет, метка на месте; следующий `start` с очищенной историей — в первом
  запросе к модели метка, значения нет;
* `Gateway::session_id()` отдаёт сессию, выданную воркером
  (`start.result.session_id`) — её принимает `history clear --session`;
  `history::read_messages` принимает и `{messages: […]}` и массив верхнего уровня;
* CLI: `hds agent run --project P --session <id>` возобновляет сессию (история из
  `agent-state`, событие `{"kind":"session","resumed":true}`), `TurnReport.session_id`
  несёт идентификатор; `hds agent history clear --project P [--session ID]`
  печатает число замен и пути бэкапов, без `--session` берёт последнюю сессию из
  `<state>\.cline\data\sessions`; коды: 0 — успех, 1 — нет истории /
  незарегистрированный проект / сбой, 2 — использование.

### 1.5. A3.5: установщик воркера (подтверждено фактом)

`installers/build_agent_sidecar.ps1` + `runtime-manifests/node-manifest.json` (ASCII-only,
как соседние установщики). Проверки на срезе: `cargo test --workspace` — **751** тест
(добавлен `plan_prefers_portable_node`), `npm test` — 23/23, `fmt`/`clippy` чисто. Логи
прогонов — `tools/parity/out/a35-*.log` (каталог гитигнорится).

**Что делает скрипт:**

* **Node**: `runtime-manifests/node-manifest.json` (v24.18.0, официальные `url` + `sha256`
  из `SHASUMS256.txt`) → скачивание → проверка sha256 → распаковка в `sidecar\node`
  (macOS — `sidecar/node/bin/node`); `-NodeSource <dir>` копирует существующую установку
  (`C:\Program Files\nodejs` → путь без пробелов), `-Force` переустанавливает; маркер
  `.hds-node-version`;
* **зависимости**: `npm ci` по `package-lock.json`; маркер
  `node_modules\.hds-agent-lock` = хеш lock + режим `dev|prod`. В дереве поставки после
  сборки — `npm prune --omit=dev` (в репозитории dev-зависимости остаются; `-Prune`
  включает чистку на месте, `-KeepDev` выключает в поставке);
* **сборка**: `npm run build` → `dist\worker.mjs` (163 KiB, esbuild ESM,
  `--packages=external`);
* **самопроверка `-SelfTest`**: путь Node без пробелов, `import('@cline/sdk')` (не только
  наличие пакета), `node_modules\tavily-mcp\build\index.js` (путь, который hds передаёт
  MCP-клиенту), `hello` → `ready {protocol:1}` с непустым `sdk`, затем `shutdown` и выход;
* **идемпотентность**: повторный запуск ничего не делает (маркеры), `-SkipInstall` /
  `-SkipBuild` — офлайн и диагностика.

**Подтверждено фактом** (четыре прогона):

1. в репозитории `-SelfTest`: скачивание zip (sha256 совпал), `npm ci` (341 пакет, 14 с),
   сборка, `hello: ready {protocol=1, node=v24.18.0, sdk=0.2.0}`;
2. дерево поставки (`-OutDir tools\parity\out\a35-dist\sidecar`): источники скопированы,
   `npm prune --omit=dev` убрал 3 dev-пакета, в `node_modules` остались `@cline/sdk` и
   `tavily-mcp`, `esbuild`/`typescript` удалены, `-SelfTest` зелёный; повторный прогон —
   no-op (Node и `node_modules` переиспользованы, сборка не повторяется);
3. офлайн-ветка `-NodeSource "C:\Program Files\nodejs"` → `sidecar\node` без пробелов,
   `-SelfTest` зелёный;
4. **продукт**: `hds agent run` с `HDS_ROOT` = установленное дерево (портативный Node +
   pruned `node_modules`) и mock-провайдером провёл полный ход: событие `session`,
   анонимизированный prompt, ответ модели, `turn_end {ok:true, reason:"completed"}`, код 0,
   ровно 1 запрос к модели.

**Тест:** `crates/hds-cli/tests/agent_run.rs::plan_prefers_portable_node` — план берёт
`<root>\sidecar\node\node(.exe)` (и в пути нет пробелов), т.е. установщик кладёт Node туда,
откуда его действительно берёт `hds-agent`.

### 1.6. A3.6: приёмка A3 (e2e) — подтверждено фактом

Стенд — `crates/hds-cli/tests/agent_acceptance.rs` (+ `tests/fixtures/tavily_stub.mjs`), один
тест `acceptance_a3_end_to_end`. Два режима: по умолчанию mock-воркер анонимизации
(28,6 с), `HDS_A36_REAL_ANON=1` — **настоящий** `sidecar\venv-anonymize` с GLiNER-моделью
(41,7 с). Логи — `tools/parity/out/a36-accept.log`, `a36-accept-real.log`.

**Что входит в прогон** (стенд из A2: образ `tools/parity/fixtures/записка_ртк_полная.docx`,
индекс и `hds mcp --http`):

* **office_read** по образу: mock-режим — ответ заглушки `[1] mock` дошёл до модели;
  настоящий Python-воркер — в запросе к модели текст docx («Пояснительная записка»);
* **disk-search** через живой `hds mcp --http`: маршрут `/mcp/agent/P0001` отвечает
  `tools/list` = `search_local_files` + `index_status` (без `ask_my_files`), агент вызывает
  `disk-search__search_local_files` и получает фрагмент своего проекта (`договор.md`);
* **заглушка `tavily`** (MCP stdio, `crates/hds-cli/tests/fixtures/tavily_stub.mjs`): вызов
  `tavily__search` проходит `mcp_out` — заглушка получила метку, а не значение; ответ
  заглушки (в нём значение есть) дошёл до модели уже меткой (`mcp_in`);
* **запись файла**: `editor` создал `ответ.md` в workspace агента;
* **инструменты**: `office_read`, `disk-search__search_local_files`, `tavily__search`,
  `editor`, `ask_user`; нет `run_commands`, `fetch_web_content`, `ask_my_files` (в т.ч. с
  префиксом сервера: `tavily__ask_my_files` вырезан);
* **I2**: ни в одном теле запроса к провайдеру нет формы значения («Иванов Иван»), метка
  есть; ни во внешний MCP (`mcp_out`), ни в файлы `agent-config`/`agent-state` не попал ключ
  провайдера;
* **I7**: реплика с ключом провайдера → ход неуспешен (`privacy_filter_unavailable`), к
  провайдеру не ушло ни одного запроса;
* **возобновление**: `run --session` — первый запрос содержал прошлый ответ модели;
* **«Очистить историю»** на копии `agent-state`: значения в файле сессии заменены меткой,
  бэкапы `.bak` на месте;
* **профиль `%USERPROFILE%\.cline`** до и после прогона совпадает (путь, размер, mtime).

**Ещё НЕ сделано в фазе A3** (после A3.6) — всё это подэтап A3.7, чеклист в §6.6:

* документация: `sidecar/README.md` (Node-воркер), раздел в `README.md`, `RELEASE_NOTES_0.25.0.md`
  «Фаза A3», статус в `AGENT_A3_HANDOFF_PROMPT.md`;
* подключение `build_agent_sidecar.ps1` к релизной сборке (`build_rust_release.ps1` и macOS),
  решение по `install_server.ps1` и `hds check` (§4 п.37);
* стоп-точка «1c-buddy под подменённым HOME» не проверялась: в приёмке сервер выключен
  (`agent.mcp.onec_buddy.enabled: false`), `tavily`-заглушка под подменённым HOME работает;
* долг A3.4 (вне A3): реестр `AgentRegistry` не подключён к CLI-прогону (один проект на
  процесс) и не имеет фонового reaper — это вкладка A5/служба (§4 п.23).

## 2. Под-этапы (остаток A3)

Правило нарезки: под-этап = один чат = один-два коммита = **видимый зелёный результат**. Если
под-этап разрастается — остановиться на ближайшем зелёном тесте, закоммитить и записать
остаток в этот файл (§3).

| # | Под-этап | Результат «снаружи» (критерий) | Объём |
|---|---|---|---|
| **A3.1** | **Воркер: TR-3.10 на Node-стенде** | **Закрыт.** `npm test` покрывает TR-3.10 а, в, г, д, е, ж и ветки воркера: MCP-заглушка через файл настроек (`<server>__<tool>`, `ask_my_files` вырезан, `mcp_out`/`mcp_in` через `egress_check`), `office_*`/`ask_user` через `host_call`, отказ guard доходит до модели текстом, fail-closed = 0 запросов к mock, секрет блокируется, `abort`, `subagents` false/true, возобновление через `history` (карта — §1.1) | закрыт |
| A3.2 | Rust ↔ настоящий воркер | **Закрыт.** `cargo test -p hds-agent`: интеграционный `tests/gateway_worker.rs` (56 тестов всего): `Gateway::spawn` → `hello` → `open` → `send` на mock-провайдере. `egress_check` проходит **настоящий** `Filter` (снимок без Python, `GatewayHost::without_client`); события деанонимизированы (`[PERSON_1]` → значение, пути workspace → пути проекта); `Redacted` без значений; fail-closed = 0 запросов к провайдеру | закрыт |
| A3.3 | Точка входа CLI | **Закрыт.** `hds agent project add|list` (`projects.json`), `hds agent run --project P [--prompt …] [--dry-run]` (В-16) собирает сессию из конфига: поиск Node (`agent.node.path` → `sidecar/node` → PATH), `env::build`, `mcp::write_settings`, `rules::write_rules`, `Secrets`, снимок vault, `Gateway`; события печатаются в консоль (human / `--json` / `--quiet`). Тесты на mock-провайдере — `crates/hds-cli/tests/agent_run.rs` | закрыт |
| A3.4 | Жизненный цикл (TR-3.9) | **Закрыт.** Реестр `AgentRegistry` в `hds-agent`: `max_live`/`idle` из `AgentSettings` (В-16: 2 и 30 мин), LRU-выгрузка (процесс завершён), переиспользование процесса на повторный `get_or_open`, переоткрытие упавшего воркера, `reap_idle` по параметру времени. Возобновление `session_id + initialMessages` (`history::read_messages`) — прошлая реплика в первом запросе модели; `session_id` без истории её не грузит. «Очистить историю» end-to-end (`Gateway::clear_history`, бэкапы `.json.bak`). CLI: `run --session`, `history clear [--session]`. Факты — §1.4, находки — §4 п.23–28 | закрыт |
| A3.5 | Установщик | **Закрыт.** `installers/build_agent_sidecar.ps1`: портативный Node из `runtime-manifests/node-manifest.json` (v24.18.0, официальные sha256) → `sidecar\node` без пробелов (`-NodeSource` — копия существующей установки, офлайн), `npm ci` по lock, сборка `dist\worker.mjs`, `npm prune --omit=dev` в дереве поставки, `-SelfTest` = `hello` + импорт `@cline/sdk` + `tavily-mcp`. Факты — §1.5, находки — §4 п.29–32 | закрыт |
| A3.6 | Приёмка A3 (e2e) | **Закрыт.** `crates/hds-cli/tests/agent_acceptance.rs`: образ → `office_read` (настоящий Python-воркер в режиме `HDS_A36_REAL_ANON=1`), `disk-search__search_local_files` через живой `hds mcp --http` (`/mcp/agent/P0001`), заглушка `tavily` (`mcp_out`/`mcp_in`), запись `ответ.md` в workspace; I2/I7, профиль `%USERPROFILE%\.cline` не изменён, `run --session`, `history clear`. Факты — §1.6, находки — §4 п.33–37 | закрыт |
| A3.7 | Документация и закрытие | `sidecar/README.md` (Node-воркер: сборка, протокол, установщик, тесты), раздел в `README.md` (включение, команды, что уходит/не уходит наружу, ограничения), `RELEASE_NOTES_0.25.0.md` «Фаза A3», статус в `AGENT_A3_HANDOFF_PROMPT.md` (§5 критерии, §8 В-12…В-16), подключение `build_agent_sidecar.ps1` к `build_rust_release.ps1` (+ macos) и решение по `install_server.ps1`/`hds check`; полный набор проверок §5 карты. **Промт — §7**, чеклист — §6.6 | ~0,5 чата |

Зависимости: A3.1 и A3.2 независимы (A3.1 первым — он проверяет допущения о SDK, на которые
опирается всё остальное). A3.3 ← A3.2; A3.4 ← A3.3; A3.5 независим и закрыт; **A3.6 ← A3.3 +
A3.5 — следующий**; A3.7 — последним.

## 3. Журнал под-этапов

| Под-этап | Состояние | Коммиты | Заметки |
|---|---|---|---|
| A3.0 | закрыт 19.11.2026 | `f10a92e8`, `a8dc5b51` | см. §1 |
| A3.1 | закрыт 19.11.2026 | `2efe8043` (fix), `8e26d0f3` (test) | `npm test` 23/23, ≈ 39 с; карта тестов — §1.1; находки — §4 п.7–12; `Gateway::open` поправить в A3.2 (§4 п.1) |
| A3.2 | закрыт 19.11.2026 | `6ef028af` (feat), `8204e384` (test) | `cargo test -p hds-agent` = 56 тестов (54 unit + 2 интеграционных), `cargo fmt --check` и `clippy --workspace -D warnings` чистые; факты — §1.2, находки — §4 п.1–3 и 13–16 |
| A3.3 | закрыт 19.11.2026 | `be72b01d` (feat), `74d1aacb` (test) | `cargo test -p hds-cli --test agent_run` = 3 теста, весь воркспейс 737/737; факты — §1.3, находки — §4 п.17–22 |
| A3.4 | закрыт 19.11.2026 | `55abc33b` (feat), `1b80fc3f` (test) | `cargo test -p hds-agent` = 68 тестов (8 — `registry`, 3 — `lifecycle_worker`), ≈ 11 с; `cargo test -p hds-cli --test agent_run` = 5, весь воркспейс 750/750; `npm test` 23/23; факты — §1.4, находки — §4 п.23–28. Долг: реестр готов, но CLI-прогон по-прежнему один проект на процесс (подключение — вкладка A5) |
| A3.5 | закрыт 19.11.2026 | `babb2223` (feat), `3e9d61f7` (test) | `installers/build_agent_sidecar.ps1` + `runtime-manifests/node-manifest.json`; 4 прогона (`tools/parity/out/a35-*.log`): репозиторий, дерево поставки с `npm prune`, офлайн `-NodeSource`, `hds agent run` на установленном дереве; воркспейс 751 тест, `npm test` 23/23; факты — §1.5, находки — §4 п.29–32 |
| A3.6 | закрыт 19.11.2026 | `9db20e55` (test) | `crates/hds-cli/tests/agent_acceptance.rs`; два режима: mock-воркер (28,6 с) и настоящий NER (`HDS_A36_REAL_ANON=1`, 41,7 с); логи — `tools/parity/out/a36-accept*.log`; факты — §1.6, находки — §4 п.33–37 |

## 4. Известные долги и находки (передаются по под-этапам)

1. **Сделано в A3.1/A3.2:** `start` возвращает `declared_tools`, `Gateway::open` его читает
   (интеграционный тест A3.2 это фиксирует). `Up::Ready { tools }` (ответ на `hello`) остаётся
   как есть — воркер отдаёт там пустой список.
2. **Сделано в A3.2:** мёртвая ветка `GatewayEvent::Agent` при `Up::HostCall` убрана: `map_up`
   теперь возвращает `GatewayError::Worker("host_call … попал в поток событий")`.
3. **Сделано в A3.2:** `turn_end` приходит отдельным `GatewayEvent::TurnEnd {ok, reason, text,
   error, tools, raw}`; добавлена проверка «успех при пустом ответе» (Rust-сторона, в дополнение
   к проверке воркера). **A3.3/UI:** успех хода определять по `TurnEnd`, а не по `Done` —
   `Done` после `send` означает только «принято» (`{accepted: true}`).
4. Скрипт `npm test` — `node --test "test/*.test.mjs"`. Каталог `test/` Node 24 сам не
   раскрывает.
5. `npm install` предупреждает `allow-scripts` про `esbuild` и `protobufjs`. Бинарник esbuild
   для win32-x64 приходит как optional-зависимость, сборка работает без postinstall. Учесть
   в A3.5.
6. Терминал агента: `git` без `--no-pager` вешает команду (пейджер). Длинные прогоны
   (`cargo test --workspace`) — через `Start-Job` + `Wait-Job -Timeout` и лог в
   `tools/parity/out/`.
7. **A3.1: SDK на фатальной ошибке прогона резолвит `send` и уничтожает сессию.** Было:
   `turn_end {ok:false, reason:"unknown", error:"unknown"}`; следующий `send` падал
   `session not found: <id>`. Исправлено в воркере: `turn_end` собирается из событий
   (`done`/`ended`/`error`, причина — текст ошибки, например `privacy_filter_unavailable`),
   а после фатальной ошибки (`ended.reason == "error"`) воркер перезапускает сессию с **тем
   же** `session_id` перед следующим ходом (`revive()`; `initialMessages` при этом не
   передаются, чтобы история не удвоилась). В A3.2 учесть: `start` после ошибки возвращает
   тот же `session_id`, историю не грузит.
8. **A3.1: плагины выключаются политикой `plugins: {'*': {enabled: false}}`, а не пустыми
   `pluginPaths: []`.** Проверено экспериментом: без политики плагин из `<HOME>\.cline\plugins`
   загружается, и в `tools` появляется его инструмент. Обнаружение идёт по
   `<workspace>\.cline\plugins`, `<HOME>\.cline\plugins`, `<HOME>\Documents\Cline\Plugins`;
   **только `.js`/`.ts`** (`PLUGIN_FILE_EXTENSIONS`; `.mjs` не рассматривается). Модуль
   плагина — `export default {name, manifest: {capabilities: [...]}, setup(api, ctx)}` либо
   именованный экспорт `plugin`. Отклонённый политикой плагин виден в `start.result.plugin_issues`
   (сейчас — список имён; A3.3/UI может показать «плагин отключён»).
9. **A3.1: Agent Plugin (`<HOME>\.agents\plugins`) грузит свои MCP-серверы в сессию по
   умолчанию** — та же политика `plugins` его отсекает. Формат: `plugin.json`
   (`$schema: https://agent-plugins.org/schemas/1.0.0/plugin.schema.json`, `name`, `version`) +
   `mcp.json` (`$schema: .../mcp.schema.json`, `mcpServers: {name: {type: "stdio", command, args}}`
   — **без** обёртки `transport`). Имена инструментов таких серверов получают хеш-суффикс
   (`hds-agent-plugin_agentdecoy__ask_my_files_3deb181b`) — прежняя проверка
   `endsWith("__ask_my_files")` их пропускала; в A3.1 добавлен `isForbiddenToolName`
   (суффикс + хеш) и тест на все формы.
10. **A3.1: `CLINE_MCP_SETTINGS_PATH` действительно читается SDK** (`resolveMcpSettingsPath`
   в `@cline/shared/storage`), путь по умолчанию — `<CLINE_DATA_DIR>\settings\cline_mcp_settings.json`.
    Файл-обманка в этом месте не загружается (тест изоляции), т.е. Rust достаточно задавать
    переменную, как сейчас. MCP-инструменты готовы уже к первому запросу — `start` ждать
    ничего не нужно (проверено: заглушка в первом же `tools`).
11. **A3.1: 429 повторяется SDK через `egressFetch`** (в тесте — 2 запроса, оба с
    `egress_check`; пауза ~1–2 с). `abort` даёт `ended {reason:"aborted"}`, сессия остаётся
    рабочей. Суб-агент на mock-провайдере запускается, его запрос идёт через `egressFetch`.
12. **A3.1: в стенд добавлено, в продукт — нет.** `mock_provider.mjs`: шаги `{status}` и
    `{delayMs}`, `authorization` в журнале. `mcp_stub.mjs`: журнал вызовов в файл по
    `HDS_STUB_LOG`. `harness.mjs`: `writeStubMcp`, `readStubLog`, `filesUnder`, третий
    аргумент `startWorker(dirs, host, extraEnv)`. Диагностических RPC и env-переключателей в
    воркер не добавляли — все проверки сделаны через поведение.
13. **A3.2: `canonicalize()` на Windows даёт verbatim-путь `\\?\C:\…`, а Node его не
    переваривает** — `node \\?\C:\…\worker.mjs` падает на `realpathSync` → `lstat 'C:'`
    (EISDIR), воркер закрывает канал, `hello` не отвечает. Перед передачей пути в Node
    снимать префикс `\\?\` (в тесте — `worker_script()`). **A3.3/A3.5:** так же чистить путь
    к `dist/worker.mjs`, портативному `node` и каталогу проекта в `projects.json`.
14. **A3.2: у `Gateway` был обязательный Python-клиент.** Добавлен egress-only режим:
    `Gateway::spawn_egress_only` (без клиента, `send` → `GatewayError::Anonymize`),
    `Gateway::send_anonymized` (текст уже обезличен) и `Gateway::refresh_from_snapshot`
    (снимок применяется без Python). **A3.3:** CLI всегда поднимает `Gateway::spawn` с
    клиентом; egress-only — стенды/тесты и режим «анонимизатор недоступен → fail-closed».
15. **A3.2: mock-провайдер для Rust-теста — внутри теста** (`MOCK_JS` + `node`, порт
    `127.0.0.1:0`, тела запросов в `mock-requests.jsonl`); от `sidecar/hds_agent/test/fixtures`
    не зависит, интерпретатор переопределяется `HDS_NODE`. Тест сам решает, доступны ли
    предпосылки (нет `node`/`dist` → печатает причину и выходит).
16. **A3.2: интеграционные тесты идут параллельно в одном процессе** — `process::id()` у них
    общий, общий временный каталог приводил к падению (один тест удалял каталог другого).
    Каталог должен включать счётчик (`AtomicU32`), не только pid.
17. **A3.3: границы CLI-прогона.** `hds agent run` — один проект и один ход на процесс;
    реестр живых процессов, `max_live_projects`, `idle_unload_min` и возобновление
    (`initialMessages`) — A3.4. `ask_user` пока `NoAskUser` (модель получает понятный отказ):
    вопросы пользователю появятся с вкладкой (A5). События печатаются в консоль, `--json` —
    NDJSON (тот же поток пойдёт в UI).
18. **A3.3: `HDS_ROOT` переопределяет и `state_dir`, и `project_root()`** (корень поставки:
    `sidecar/`, портативный Node, Python-воркер). Для изоляции тестов это конфликт: данные
    хочется в temp, а `dist/worker.mjs` — из репозитория. Поэтому добавлен
    `agent_run::plan_with(cfg, project, opts, sidecar_root)`; `plan()` = `plan_with(..., &project_root())`.
    **A3.4/A3.6:** если понадобится развести их в бою — отдельный ключ конфига, а не
    перегрузка `HDS_ROOT`.
19. **A3.3: `open_client` создаёт `agent-state` до старта воркера** — иначе `stderr_log`
    внутри ещё не созданного каталога валит spawn (`os error 3`). `HDS_ANONYMIZE_WORKER`
    подменяет скрипт воркера анонимизации: диагностика и тесты (стенд проверяет проводку
    на `crates/hds-anonymize/tests/mock_worker.py`, не поднимая NER-модели). В продукте
    переменная не задаётся.
20. **A3.3: `Secrets::provider_for` требует `projects.<P>.provider` в `secrets.json`**, а
    провайдер/модель A3.3 читает из реестра `projects.json` (TR-8.1). Добавлен
    `Secrets::provider(name)` — ключ по имени провайдера; `provider_for` оставлен для A2-схемы.
21. **A3.3: аудит срабатываний фильтра** — `Audit::from_config` (`audit.enabled`; desktop —
    выключен, desktop-стенды журнал не пишут; на сервере включён по умолчанию). При приёмке
    A3.6 учитывать профиль: «аудит есть» проверять с `audit.enabled: true`.
22. **A3.3: `--dry-run` не создаёт каталоги и не запускает процессы** (план печатается до
    `prepare`): это же точка для A5 — вкладка показывает план до старта воркера. Ключ
    провайдера и токен проекта в план не попадают (проверяется тестом).
23. **A3.4: реестр живых проектов — `crates/hds-agent/src/registry.rs`.** `AgentRegistry`
    (`new(max_live, idle)`, `from_settings`, `defaults`) хранит по проекту `Gateway` и
    `last_used`; время приходит параметром, **фонового потока нет** — тик
    `reap_idle(Instant::now())` будет звать вкладка A5/служба. Выгрузка = `stop` +
    `shutdown` (≤ 5 с ожидания, затем kill) + удаление записи; `max_live == 0`
    трактуется как 1. `Drop` реестра выгружает всё (CLI/тесты не оставляют процессов).
    **Не подключён к CLI-прогону:** `hds agent run` по-прежнему один проект на процесс
    (§4 п.17) — реестр нужен вкладке, где несколько проектов живут одновременно.
24. **A3.4: реестр переоткрывает упавший воркер.** `get_or_open` смотрит на
    `Gateway::is_closed()`: мёртвая запись выгружается (без ожидания — процесс уже
    вышел) и проект открывается заново. Без этого UI получил бы закрытый канал, а
    «перезапустить» пользователю было бы нечем. Тест гоняет это на заглушке:
    `send {text:"__crash__"}` → `process.exit(1)`.
25. **A3.4: `Gateway::open` запоминает сессию, выданную воркером** (аддитивно):
    раньше `session_id` брался только из аргумента, поэтому новая сессия не знала
    своего id, и «Очистить историю» без `--session` была невозможна. Теперь при
    `start` без id читается `start.result.session_id`, добавлен
    `Gateway::session_id()`. Публичные сигнатуры A3.2/A3.3 не менялись.
26. **A3.4: форматы истории.** `history::read_messages` принимает и `{ "messages": […] }`,
    и массив верхнего уровня (SDK писал по-разному). Добавлен
    `history::latest_session(state_dir)` — последняя сессия по времени изменения
    `.messages.json` (нужна `history clear` без `--session`).
27. **A3.4: очистка истории помечает форму граммемой.** Значение «Иванов Иван» в
    файле заменяется на `[PERSON_1|nomn]`, а не на `[PERSON_1]` (в снимке mock-воркера
    у формы есть граммема). Проверять в тестах префикс метки, а не точное совпадение:
    разбор — только `hds_core::labels`.
28. **A3.4: `hds agent history clear` не поднимает Node и не требует провайдера.**
    Нужен только снимок vault (воркер анонимизации) и `Filter`; `secrets.json`
    читается best-effort (без него очистка работает, ключи просто не входят в
    словарь). Тестам CLI нужен `HDS_ANONYMIZE_PYTHON` (при `HDS_ROOT` = temp venv
    лежит в репозитории) — `HDS_ANONYMIZE_WORKER` подменяет скрипт на
    `crates/hds-anonymize/tests/mock_worker.py`. **A3.6:** «Очистить историю»
    проверять на копии профиля — реальные файлы сессии трогать нельзя.

29. **A3.5: портативный Node берётся из манифеста, а не из кода скрипта.**
    `runtime-manifests/node-manifest.json` (`tag`, `assets[{platform, archive, file_name,
    url, sha256, root, exe, npm}]`) — единственное место с версией и хешами; sha256 взяты из
    официального `https://nodejs.org/dist/<tag>/SHASUMS256.txt`. Смена версии Node = правка
    манифеста, скрипт не переписывается. Скачанный архив проверяется по sha256 **до**
    распаковки; несовпадение — остановка. `-NodeSource <dir>` копирует существующую установку
    (`C:\Program Files\nodejs`) — это и есть офлайн-путь, копия убирает пробел из пути.
30. **A3.5: `npm ci` нужен полный, «только runtime» делается `npm prune --omit=dev`.**
    `esbuild`/`typescript` — devDependencies, без них `npm run build` не работает; поэтому
    порядок: `npm ci` → `npm run build` → `npm prune --omit=dev` (в дереве поставки). В
    репозитории dev-зависимости остаются (иначе после установщика не собрать и не прогнать
    `npm test`); переключатели — `-Prune` (почистить на месте) и `-KeepDev` (не чистить в
    поставке). Идемпотентность — маркер `node_modules\.hds-agent-lock` = хеш lock + `dev|prod`.
31. **A3.5: `-SelfTest` проверяет импорт, а не наличие файла.** `ready.sdk` берётся из
    `node_modules\@cline\sdk\package.json` рядом с `dist\worker.mjs`, но сам SDK при `hello`
    не импортируется — поэтому самопроверка дополнительно делает
    `node -e "import('@cline/sdk')…"` в каталоге воркера и проверяет
    `node_modules\tavily-mcp\build\index.js` (ровно этот путь `hds-agent` передаёт MCP-клиенту).
    Иначе `npm prune`, срезавший зависимость, прошёл бы «зелёным».
32. **A3.5: PowerShell 5.1 и кириллица.** Установщики остаются ASCII-only (BOM-less `.ps1`
    читается как ANSI). Проверочный стенд с русскими строками нужно сохранять как UTF-8 **с
    BOM**, иначе парсер падает на «непредвиденной лексеме» (наступил на этом при прогоне
    `a35-e2e.ps1`). То же и с правкой файлов: `Get-Content -Raw | Set-Content` в PS 5.1
    читает UTF-8 как ANSI и **портит кириллицу** (наступил на этом с `agent_acceptance.rs` —
    восстанавливал обратным перекодированием). Файлы править только
    `[IO.File]::ReadAllText/WriteAllText` с явной `UTF8Encoding`. Осталось для A3.7/A6:
    подключить установщик к `build_rust_release.ps1` и `install_server.ps1`, добавить
    проверку портативного Node в `hds check` и раздел про `sidecar\node` в `README.md`.
33. **A3.6: приёмка — Rust-тест, а не скрипт.** `crates/hds-cli/tests/agent_acceptance.rs`
    поднимает живой `hds mcp --http` (маршрут A2 с токеном проекта), индекс — строками через
    `hds_core::db` (FTS, без эмбеддера: `Embedder` недоступен → ключевой поиск), и копирует
    python-часть воркера извлечения в `<HDS_ROOT>/sidecar`. Без этой копии
    `search_local_files` отвечает «не найден интерпретатор воркера» (в поставке её кладёт
    `build_sidecar.ps1`); `HDS_EXTRACT_PYTHON` указывает на venv анонимизации — pymorphy3 там
    нет, лемматизатор деградирует, выдача FTS от этого не страдает.
34. **A3.6: журнал заглушки `tavily` — через ключ.** `McpSpec` передаёт в env сервера только
    `TAVILY_API_KEY`, поэтому стенд кодирует путь журнала в самом ключе
    (`tvly-a36-stand|<путь>`), а заглушка разбирает его. Это приём стенда, в продукте ключ
    обычный; если SDK перестанет передавать env, журнал пропадёт и тест это заметит
    (`mcp_out` проверяется по журналу).
35. **A3.6: mcp_in не спасает незнакомые значения.** `run` берёт снимок фильтра **до** хода,
    а настоящий воркер нумерует метки по мере встречи: значение, лежащее в индексе сырым
    текстом и неизвестное снимку, уходит модели. Стенд заводит метку
    `client.label_for_value` до прогона (в продукте индекс уже анонимизирован — A2, карточка
    анонимизации). То есть mcp_in — это «известные формы → метки», а не универсальный
    детектор; опора I2 — анонимизированный индекс.
36. **A3.6: приёмочный режим с настоящим NER требует весов** `models/anonymize/hf-home`
    (в репозитории их нет: `installers/fetch_anonymize_models.ps1`). На машине разработчика
    сделан junction `models\anonymize\hf-home → %USERPROFILE%\.cache\huggingface`
    (каталог `models/` гитигнорится). В стенде `anonymize.gpu: cpu` — приёмка не отбирает
    VRAM у резидента; время: 41,7 с против 28,6 с у mock-воркера.
37. **A3.6: что осталось для A3.7.** `sidecar/README.md`, раздел в `README.md`,
    `RELEASE_NOTES` «Фаза A3», подключение `build_agent_sidecar.ps1` к
    `build_rust_release.ps1`/`install_server.ps1`, `hds check` про портативный Node.
    Стоп-точка «1c-buddy под подменённым HOME» **не проверялась**: в приёмке сервер выключен
    (`agent.mcp.onec_buddy.enabled: false`); `tavily`-заглушка под подменённым HOME работает.

## 5. Промт для A3.1 (выполнен; оставлен для истории)

```text
Ты продолжаешь фазу A3 («агент в анонимной зоне») проекта hermes-disk-search (hds), под-этап
A3.1 — «Воркер: TR-3.10 на Node-стенде». Работаешь ТОЛЬКО в sidecar/hds_agent (src/ и test/)
и в AGENT_A3_STAGES.md. Rust, CLI, установщик и документацию не трогаешь: это A3.2…A3.7.

СНАЧАЛА ПРОЧИТАЙ (файлы репо читай через PowerShell
[System.IO.File]::ReadAllLines(path, [Text.Encoding]::UTF8): read_files отдаёт пустоту):
1. AGENT_A3_STAGES.md целиком: что сделано (§1), под-этапы (§2), долги и грабли (§4).
2. AGENT_A3_HANDOFF_PROMPT.md: §3.4–3.7, §3.9, §3.10 (таблица TR-3.10 а–з), §3.12 (точки
   фильтра), §3.13 (протокол), §6 (грабли).
3. tools/parity/AGENT_SPIKES.md: §3.3 (хуки), §3.4 (toolPolicies), §3.5 (суб-агенты),
   §3.6 (история), §3.7 (MCP), §4 (имена API).
4. Код воркера: sidecar/hds_agent/src/*.ts и test/**. fixtures/harness.mjs играет роль
   Rust-стороны (env, host_call, упрощённый словарь); mock_provider.mjs, mcp_stub.mjs;
   образец e2e: e2e_basic.test.mjs.
5. Типы SDK: sidecar/hds_agent/node_modules/@cline/{core,shared}/dist/**/*.d.ts.
Затем покажи короткий план (список тестов и что меняется в src) и начинай.

СТАРТ:
* git --no-pager status: ветка agent-privacy-zone, HEAD — коммит docs A3.0 (после a8dc5b51),
  дерево чистое. Чужие правки не трогать; если мешают — остановись и спроси.
* cd sidecar/hds_agent; npm ci (если нет node_modules); npm run build; npm test → 8/8.

ЗАДАЧА: тесты на node --test, без новых зависимостей. Каждый тест — отдельный проверяемый
факт. Сначала пиши тест; если он красный, правь src.
 1. MCP: файл по CLINE_MCP_SETTINGS_PATH с заглушкой test/fixtures/mcp_stub.mjs (stdio,
    command = nodeCommand(): путь с пробелом SDK не квотирует). В tools запроса к модели есть
    <server>__search_local_files и <server>__leak, но нет <server>__ask_my_files (его
    вырезает beforeModel). Проверить и в запросе mock, и в turn_end.tools.
 2. mcp_out: аргументы <server>__search_local_files содержат «Иванов Иван» → заглушка
    получила [PERSON_1]. mcp_in: результат leak («…Иванов Иван») → во второй запрос к модели
    ушла метка. Оба egress_check видны в журнале стенда (kind mcp_out и mcp_in).
 3. Секрет: egress_check отклоняет SECRET (в аргументах MCP или в реплике) → вызов MCP не
    состоялся либо к модели не ушло ни одного запроса; turn_end ok=false; процесс жив,
    следующий send работает.
 4. Fail-closed: хост отвечает ok=false на каждый egress_check kind=model → mock получил
    0 запросов, turn_end.ok=false, в error есть privacy_filter_unavailable.
 5. Guard в живом прогоне (TR-3.10 а, ж): read_files с ..\outside\secret.txt, UNC, NUL →
    в следующем запросе к модели есть причина «hds-guard: …», содержимое outside не ушло;
    относительный путь читается из workspace. read_files по report.docx → «используй
    office_read»; editor по *.pdf.md → отказ.
 6. office_read / office_edit / office_create / ask_user: вызов модели → host_call с верным
    payload (путь как дал агент, ops массивом) → результат дошёл до модели. Путь вне
    workspace у office_* → отказ guard, host_call не вызван.
 7. TR-3.10 в: в окружении воркера нет ключей провайдеров и реального профиля. Например,
    стенд кладёт OPENROUTER_API_KEY в env родителя, harness его не передаёт — проверь это
    по поведению воркера. Отладочных команд в протокол продукта не добавлять; если нужен
    диагностический ответ — только при HDS_AGENT_TEST=1, и согласуй это в плане.
 8. TR-3.10 г, е: плагин (минимальный, формат найди в типах SDK) в <HOME>\.cline\plugins
    и MCP-сервер в файле настроек по умолчанию <HOME>\.cline\data\settings не загружены
    (их инструментов нет в tools). Если формат плагина за разумное время не выяснить —
    запиши это в AGENT_A3_STAGES.md §4 и иди дальше.
 9. TR-3.10 д: повтор после 429 от mock (добавь шаг {status: 429} в mock_provider) → число
    запросов mock = числу egress_check kind=model.
10. abort во время хода (mock с задержкой ответа) → turn_end с reason aborted, процесс жив.
11. subagents: false → в tools нет spawn_agent; true → есть, и запрос суб-агента идёт через
    egress_check (A0 §3.5). Если суб-агент на mock не запускается — запиши это, не застревай.
12. История: start с history (сообщения прошлого прогона из
    <state>\.cline\data\sessions\<id>\<id>.messages.json) и тем же session_id → первый
    запрос к модели содержит прошлую реплику. Заодно проверь, что api_key провайдера не
    попал ни в один файл под agent-state и agent-config.
13. Долг §4.1: поле start.result.tools снять или переименовать в declared_tools; в §4
    записать «поправить Gateway::open в A3.2».

ПРАВИЛА:
* Хуки не бросают исключений; таймаут guard (2 с) без причины не менять.
* stdout воркера — только протокол, диагностика — в stderr.
* Тесты быстрые: mock на 127.0.0.1:0, временные каталоги в os.tmpdir(), после теста всё
  удаляется; весь npm test ≤ 3 мин.
* Боевые config.yaml, index.db, data/ не трогать; артефакты — в tools/parity/out/.
* git — всегда с --no-pager; длинные команды — через Start-Job + Wait-Job -Timeout.
* Редактор принимает ≤ 6000 символов за вызов: большие файлы пиши частями.

ЗАВЕРШЕНИЕ:
* npm run typecheck, npm run build, npm test — зелёные; чистая копия (npm ci + build)
  собирается.
* Коммиты явным списком путей: «test(agent): A3.1 — …», «fix(agent): A3.1 — …».
* Обнови AGENT_A3_STAGES.md: §3 (A3.1 закрыт, коммиты), §1 (что подтверждено фактом),
  §4 (новые находки и долги), при необходимости уточни A3.2. Закоммить отдельным
  docs-коммитом. Push не делать.
* Дай сводку: какие из пунктов 1–13 зелёные, какие отложены и почему, что меняется для A3.2.
* СТОП-ТОЧКИ (остановиться и спросить владельца): guard вынужден бросать исключение; SDK
  обходит egressFetch (у mock есть запрос без egress_check); ключ провайдера попал в файл
  вне secrets.json (например, в сессию SDK в agent-state).
```

## 6. Контекст для нового чата (A3.7 и далее)

> «Памятка входа»: что сделано, где лежит, какие грабли уже оплачены. Промт
> следующего подэтапа — §7. Требования и критерии фазы — `AGENT_A3_HANDOFF_PROMPT.md`;
> нарезка фазы A3 на подэтапы — §2 этого файла.

### 6.1. Где мы

* Верхний уровень: фаза **A3** плана `PLAN_AGENT_PRIVACY_ZONE.md` (§5.3, §5.5, §5.6) —
  «агент в анонимной зоне». Нарезана на подэтапы **A3.1…A3.7** (§2): A3.1 — воркер на
  Node-стенде, A3.2 — Rust ↔ настоящий воркер, A3.3 — точка входа CLI, A3.4 — жизненный
  цикл (TR-3.9), A3.5 — установщик, **A3.6 — приёмка e2e — закрыт**, последний подэтап —
  **A3.7** (документация и закрытие фазы).
* Ветка `agent-privacy-zone`, HEAD ≥ `9db20e55` (test A3.6), дерево чистое, push не делался.
* Коммиты: A3.0 — `f10a92e8`, `a8dc5b51`; A3.1 — `2efe8043` (fix), `8e26d0f3` (test);
  A3.2 — `6ef028af` (feat), `8204e384` (test); A3.3 — `be72b01d` (feat), `74d1aacb` (test);
  A3.4 — `55abc33b` (feat), `1b80fc3f` (test); A3.5 — `babb2223` (feat), `3e9d61f7` (test);
  A3.6 — `9db20e55` (test); docs — `7779343b`, `c5c1f57d`, `76661a7a` и docs-коммиты
  A3.4…A3.6.
* Проверки на срезе: `cargo fmt --all --check` и
  `cargo clippy --workspace --all-targets -- -D warnings` — чисто;
  `cargo test --workspace -- --test-threads=4` — **752** теста, 0 падений;
  `npm test` в `sidecar/hds_agent` — 23/23, ≈ 39 с; чистая копия (`npm ci` + build) собирается;
  `installers/build_agent_sidecar.ps1 -SelfTest` — зелёный (портативный Node + `hello`);
  приёмка `cargo test -p hds-cli --test agent_acceptance` — зелёная (mock и настоящий NER).

### 6.2. Что уже работает (точки входа)

**Node-воркер `sidecar/hds_agent`** (TS → `dist/worker.mjs`, `@cline/sdk` 0.2.0): протокол
§4.3 (`hello`/`start`/`send`/`abort`/`stop`/`shutdown` + события + `host_call`), политики
(без `run_commands`/`fetch_web_content`), `hds-guard`, `office_*`/`ask_user` через
`host_call`, `egressFetch` (каждый запрос и повтор — `egress_check`), MCP из
`CLINE_MCP_SETTINGS_PATH`, плагины профиля выключены политикой, `turn_end {ok, reason, text,
error, tools}`, восстановление сессии после фатальной ошибки (`revive` с тем же id).
Тесты: `npm test` (карта — §1.1).

**Установщик воркера `installers/build_agent_sidecar.ps1`** (A3.5): портативный Node из
`runtime-manifests/node-manifest.json` → `sidecar\node` (без пробелов в пути; `-NodeSource`
копирует существующую установку), `npm ci` по lock, `npm run build` → `dist\worker.mjs`,
`npm prune --omit=dev` в дереве поставки, `-SelfTest` = `hello` + импорт `@cline/sdk` +
наличие `tavily-mcp`. Идемпотентен (маркеры `.hds-node-version`,
`node_modules\.hds-agent-lock`).

**Приёмка A3.6** (`crates/hds-cli/tests/agent_acceptance.rs`): один тест поднимает стенд из A2
(образ `tools/parity/fixtures/записка_ртк_полная.docx`, индекс через `hds_core::db`, живой
`hds mcp --http` на маршруте `/mcp/agent/P0001`) и проходит полный ход: `office_read` →
`disk-search__search_local_files` → `tavily__search` (заглушка MCP) → `editor`; проверяет
I2/I7, неизменность `%USERPROFILE%\.cline`, возобновление и «Очистить историю».
`HDS_A36_REAL_ANON=1` — то же с настоящим воркером анонимизации (NER на CPU).

**Крейт `crates/hds-agent`** (lib, без async):

* `worker` — `Worker::spawn(WorkerSpec, Arc<dyn HostHandler>)`, `call`, `next_event`,
  `write`, `shutdown` (5 с ожидания → kill); `WorkerSpec { program, script, cwd, env,
  stderr_log, timeout }`;
* `gateway` — `Gateway::spawn` (с Python-клиентом) / `Gateway::spawn_egress_only`, `hello`,
  `open(provider, history, session_id, max_iterations, subagents) -> declared_tools`, `send`,
  `send_anonymized`, `abort`, `stop`, `next_event` → `GatewayEvent::{Ready, Agent, TurnEnd,
  Redacted, Done}`, `refresh`/`refresh_from_snapshot`, `session_id`, `clear_history`,
  `shutdown`;
  `GatewayHost::{new, without_client}` (egress_check через `Filter`, `office_*` → Python,
  `ask_user`);
* `filter` — `Filter::new(project, keep, UnknownPolicy, max_stale, Option<Audit>)`,
  `set_snapshot`, `redact`, `redact_stored`, `sys_values`; срок снимка —
  `agent.filter.max_stale_sec` (`AgentSettings::filter_max_stale_secs`, дефолт 300 = В-14);
* `deanon` — `DeAnonymizer::new(snapshot, sys, workspace, project_root)` (метки → значения,
  пути workspace → пути проекта);
* `history` — `session_dir`, `session_files`, `clear(state, session, filter)` (бэкапы
  `.json.bak`), `read_messages(state, session) -> Vec<Value>` (для `initialMessages`),
  `latest_session(state)`;
* `registry` (**A3.4**) — `AgentRegistry::{new, from_settings, defaults}`, `get_or_open(project,
  now, spawn_fn)`, `get_mut`, `touch`, `unload`, `reap_idle(now)`, `live`, `projects`;
  выгрузка = `stop` + `shutdown`, жертва — LRU, упавший процесс переоткрывается. Фонового
  потока нет — `reap_idle` зовут с `Instant::now()` (вкладка A5/служба); CLI-прогон реестр
  пока не использует (один проект на процесс);
* `settings` — `settings(cfg)` → `AgentSettings { root, max_live_projects (2), idle_secs (1800),
  request_timeout_secs (120), subagents, unknown, keep_terms, proxy, egress_log, node_path,
  filter_max_stale_secs, mcp.* }`, `project_dir/workspace_dir/config_dir/state_dir_for`,
  `projects_file`, `secrets_file`;
* `projects` — `projects.json` (`Project {id, name, path, provider, model, created}`,
  `add/load/save/get/remove`, проверка D21); `secrets` — `Secrets::{load, provider(name),
  provider_for, project_model, project_provider, project_token, mcp_*, secret_strings}`;
* `env` — `build(cfg, config_dir, state_dir, workspace, proxy)` (HOME/USERPROFILE/APPDATA/
  TEMP → agent-config, `CLINE_DATA_DIR` → agent-state, `CLINE_MCP_SETTINGS_PATH`),
  `prepare_dirs`; `mcp` — `McpSpec::from_secrets`/`write_settings`/`settings_path`;
  `rules` — `write_rules`; `paths` — матрица путей; `protocol` — §4.3.
* Реестр живых процессов — `registry` (A3.4, см. выше); вкладке A5 остаётся тик `reap_idle`
  и переключение проектов через `get_or_open`.

**CLI `hds agent`** (`crates/hds-cli/src/cmd/agent.rs` + `agent_run.rs`): `init|token` (A2),
`project add|list`, `run --project P [--prompt …] [--session ID] [--max-iterations N]
[--subagents] [--json] [--quiet] [--dry-run]`, `history clear --project P [--session ID]`;
`agent_run::{plan, plan_with, plan_json, prepare, run, open_client, open_client_at,
clear_history, TurnReport, RunOptions}`. Успех хода — по `GatewayEvent::TurnEnd`,
`Redacted` — предупреждение, `ask_user` — `NoAskUser` (отказ). Тесты:
`crates/hds-cli/tests/agent_run.rs`.

### 6.3. Грабли, которые уже оплачены (не наступать снова)

* **SDK резолвит `send` и при фатальном сбое** — успех хода только по `GatewayEvent::TurnEnd`;
  воркер сам восстанавливает сессию с тем же `session_id` (`revive`, §4 п.7). В Rust это не
  дублировать.
* **Verbatim-путь `\\?\C:\…` ломает Node** (`realpathSync` → `lstat 'C:'`): чистить перед
  передачей процесса/скрипта в Node (§4 п.13). В `agent_run::clean_path` это уже сделано.
* **`HDS_ROOT` переопределяет и `state_dir`, и `project_root()`** (корень поставки). Изоляция
  тестов — `agent_run::plan_with(..., sidecar_root)` (§4 п.18).
* **`open_client` создаёт `agent-state` до спавна** (иначе `stderr_log` валит spawn);
  `HDS_ANONYMIZE_WORKER` подменяет скрипт воркера анонимизации — только для тестов/диагностики
  (§4 п.19).
* **Плагины профиля** выключаются политикой `plugins: {'*': {enabled:false}}`, а не пустыми
  `pluginPaths`; Agent Plugin из `<HOME>\.agents\plugins` приносит MCP-серверы, имена с
  хеш-суффиксом (`…__ask_my_files_3deb181b`) — вырезаются `isForbiddenToolName` (§4 п.8–9).
* **`CLINE_MCP_SETTINGS_PATH` читается SDK**, путь по умолчанию —
  `<CLINE_DATA_DIR>\settings\cline_mcp_settings.json`; MCP-инструменты готовы к первому запросу
  (§4 п.10).
* **Повторы и abort**: 429 повторяется через `egressFetch` (каждый повтор — `egress_check`);
  `abort` → `ended {reason:"aborted"}`, сессия остаётся рабочей (§4 п.11).
* **Параллельные интеграционные тесты делят `process::id()`** — временный каталог обязан
  включать счётчик (`AtomicU32`), иначе тесты удаляют каталоги друг друга (§4 п.16).
* `npm test` использует glob `test/*.test.mjs` (Node 24 не раскрывает каталог, §4 п.4);
  `npm ci` предупреждает про `allow-scripts` (esbuild/protobufjs) — это ожидаемо (§4 п.5).
* **История сессии (A3.4):** `session_id` без `initialMessages` историю не грузит —
  сообщения читает `history::read_messages` и передаёт в `start`; файл
  `<id>.messages.json` бывает и объектом `{messages: […]}` и массивом (§4 п.26); в
  очищенной истории форма метки может быть `[PERSON_1|nomn]` — сравнивать префикс
  (§4 п.27). Реестр не держит фоновых потоков: `reap_idle(now)` зовёт вызывающий.

### 6.4. Окружение и команды (Windows, PowerShell)

```powershell
cd c:\Users\Sasha\hermes-disk-search
git --no-pager status; git --no-pager log -3 --oneline
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=4
cargo test -p hds-agent
cargo test -p hds-cli --test agent_run
cargo test -p hds-cli --test agent_acceptance
cd sidecar\hds_agent; npm test
powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_agent_sidecar.ps1 -SelfTest
```

* Приёмка с настоящим NER: `$env:HDS_A36_REAL_ANON='1'; cargo test -p hds-cli --test
  agent_acceptance -- --nocapture` (нужны веса в `models/anonymize/hf-home`, см. §4 п.36).

* PowerShell печатает `cargo` в stderr как `NativeCommandError` — ориентироваться на
  `$LASTEXITCODE`, а не на «красный» вывод.
* Длинные прогоны — `Start-Job` + `Wait-Job -Timeout` и лог в `tools/parity/out/` (гитигнорится).
* Файлы читать через `[System.IO.File]::ReadAllLines((Resolve-Path 'path'), [Text.Encoding]::UTF8)`
  и писать большими кусками ≤ 6000 символов на вызов редактора.
* В окружении **не настроена git-identity**: коммиты —
  `git -c user.name='Alex Guerchoig' -c user.email='Guerchoig@gmail.com' commit …`, пути — явным
  списком. `git` всегда с `--no-pager`.
* Каталог `Microsoft/` в корне репозитория — кэш PowerShell (`ModuleAnalysisCache`), не наш;
  удалять, не коммитить.

### 6.5. Что читать по коду (ориентиры)

| Тема | Файлы |
|---|---|
| протокол §4.3, транспорт | `crates/hds-agent/src/protocol.rs`, `worker.rs` |
| шлюз, события, TurnEnd, clear_history | `crates/hds-agent/src/gateway.rs` |
| словарь исходящего, fail-closed | `filter.rs`, `deanon.rs` |
| история сессии | `history.rs` (`read_messages`, `clear`, `latest_session`) |
| реестр живых процессов (A3.4) | `crates/hds-agent/src/registry.rs` |
| установщик воркера и портативный Node (A3.5) | `installers/build_agent_sidecar.ps1`, `runtime-manifests/node-manifest.json` |
| приёмка e2e (A3.6) | `crates/hds-cli/tests/agent_acceptance.rs`, `crates/hds-cli/tests/fixtures/tavily_stub.mjs` |
| настройки, пути, реестр проектов, секреты | `settings.rs`, `projects.rs`, `secrets.rs` |
| окружение процесса, MCP, правила | `env.rs`, `mcp.rs`, `rules.rs` |
| сборка сессии в CLI | `crates/hds-cli/src/cmd/agent_run.rs` |
| образцы тестов | `crates/hds-agent/tests/gateway_worker.rs`, `crates/hds-agent/tests/lifecycle_worker.rs`, `crates/hds-cli/tests/agent_run.rs` |
| Node-воркер и его тесты | `sidecar/hds_agent/src/*.ts`, `sidecar/hds_agent/test/*.test.mjs` |

### 6.6. Что осталось к A3.7 (чеклист «документация и закрытие фазы»)

Код A3 закрыт (A3.1…A3.6), приёмка зелёная. A3.7 — документация и интеграция установщика;
новой логики агента не пишется, но полный набор проверок §5 карты прогоняется целиком.

| Что | Где | Что отразить (факты из §1.2–1.6) |
|---|---|---|
| Node-воркер | `sidecar/README.md` (159 строк, сейчас только про Python-sidecar) | сборка `npm ci` + `npm run build` → `dist/worker.mjs` (163 KiB, esbuild ESM, `--packages=external`), Node ≥ 22.18; протокол §4.3 (`hello/start/send/abort/stop/shutdown`, события, `host_call` с префиксом `h`); `npm test` = 23/23 (`node --test`); установка `installers/build_agent_sidecar.ps1` (манифест `runtime-manifests/node-manifest.json` v24.18.0, `-SelfTest`, `-NodeSource` для офлайна); в поставке остаются только runtime-зависимости (`npm prune --omit=dev`: `@cline/sdk`, `tavily-mcp`), dev — нет |
| Раздел README | `README.md`: новый раздел после §19 «Анонимизация документов» (перед §20 «Что ещё почитать») | что такое агент в анонимной зоне и чем отличается от чата; включение (`agent.enabled`, `agent.root`, ключ провайдера — **только** `data/agent/secrets.json`); команды `hds agent project add|list`, `run [--session] [--prompt] [--dry-run] [--json]`, `history clear`; что уходит наружу (метки, не значения; ключи блокируют запрос) и что не читается (`%USERPROFILE%\.cline`, плагины, `run_commands`, `fetch_web_content`); ограничения: `ask_user` = отказ до вкладки A5, `max_live_projects=2`/`idle_unload_min=30`, реестр процессов к CLI не подключён (один проект на процесс); куда класть `dist/worker.mjs` и портативный Node. Плюс строки в §15 «Перечень команд» и §5 «Что где лежит» (`<agent.root>\P0001\{workspace,agent-config,agent-state}`) |
| Заметки о выпуске | `RELEASE_NOTES_0.25.0.md` (0.24.0 — фазы A1/A2; номер версии уточнить у владельца) | раздел «Фаза A3»: что нового по TR-3.1…3.10, TR-5.x, TR-6.x; «что качать» (архив Windows; агентский sidecar собирается установщиком/`build_agent_sidecar.ps1`); что проверено и чем (§1.2–1.6, файлы тестов: `gateway_worker.rs`, `lifecycle_worker.rs`, `agent_acceptance.rs`, `npm test`); известные ограничения и открытые вопросы |
| Статус карты | `AGENT_A3_HANDOFF_PROMPT.md` §5 (критерии) и §8 (В-12…В-16) | отметить `[x]` выполненные пункты со ссылкой на §1.x; по В-12…В-16 зафиксировать фактически принятое: В-12 (OpenRouter + `~deepseek/deepseek-flash-latest` по умолчанию), В-13 `label`, В-14 300 с + fail-closed, В-16 (2 / 30 мин / `egress_log=false`); В-15 (`1c-buddy` копией в поставку) — **не проверено**, вопрос остаётся владельцу |
| Релизная сборка | `installers/build_rust_release.ps1` (блок `$sidecarDst`, после `build_sidecar.ps1`/копии; параметры `-WithSidecar`, `-SidecarDir`, `-TranscribeProfile`) и `build_rust_release_macos.sh` | собрать агентский воркер в `$sidecarDst` (`build_agent_sidecar.ps1 -OutDir $sidecarDst`), переключатель по образцу `-TranscribeProfile` (предложение: `-AgentSidecar none|portable`, по умолчанию `portable` для Windows-архива); на macOS — аналог или явный `[i]` в логе |
| Серверный установщик | `installers/install_server.ps1` (блок P5, рядом с `build_transcribe_sidecar.ps1`; флаг `-SkipTranscribeStack`) | решить и записать: серверный профиль агент **не** ставит (desktop-функция) либо ставит по флагу `-WithAgent` со строкой в `$summary`; решение — в этом файле (§4) и в `AGENT_A3_HANDOFF_PROMPT.md` |
| `hds check` | `crates/hds-cli/src/cmd/check.rs` | пункт «портативный Node / `dist/worker.mjs`»: короткий `warn`, если `agent.enabled`, а воркера нет — по решению владельца A3.7 или A6 (§4 п.37) |

Скрипты `installers/*.ps1` — **ASCII-only** (PowerShell 5.1 читает BOM-less `.ps1` как ANSI);
.markdown-файлы править `[IO.File]::ReadAllText/WriteAllText` с `UTF8Encoding` (§4 п.32).

## 7. Промт для A3.7 (копировать целиком в новый чат, режим Act)

```text
Ты закрываешь этап A3 («агент в анонимной зоне») проекта hermes-disk-search (hds), под-этап
A3.7 — «Документация и закрытие фазы». Код A3 готов и проверен: A3.1…A3.6 закрыты
(AGENT_A3_STAGES.md §2–§3), приёмка e2e зелёная. Твоя задача — довести фазу до выпуска:
документация, интеграция установщика в релизную сборку, статус карты и вопросы владельцу.

ГРАНИЦЫ
* Пишешь только в: sidecar/README.md, README.md, RELEASE_NOTES_0.25.0.md (новый),
  AGENT_A3_HANDOFF_PROMPT.md (разделы статуса, критериев, вопросов), AGENT_A3_STAGES.md,
  installers/build_rust_release.ps1, installers/build_rust_release_macos.sh,
  installers/install_server.ps1, crates/hds-cli/src/cmd/check.rs (последний — только если
  владелец подтвердит, что пункт про Node делаем в A3.7, а не в A6).
* Не меняешь логику агента (crates/hds-agent, sidecar/hds_agent/src, crates/hds-cli/src/cmd/
  agent*.rs), контракты A1/A2 (RPC, маршрут /mcp/agent/<P>, метки в FTS), фильтр, guard,
  fail-closed. Новых зависимостей нет. Боевые config.yaml, index.db, data/ не трогаешь.
* Коммиты — явным списком путей и с identity:
  git -c user.name='Alex Guerchoig' -c user.email='Guerchoig@gmail.com' commit -m '…'.
  Push не делать.

СНАЧАЛА ПРОЧИТАЙ (файлы репо читай через PowerShell
[System.IO.File]::ReadAllLines(path, [Text.Encoding]::UTF8): read_files отдаёт пустоту)
1. AGENT_A3_STAGES.md целиком, обязательно: §1.2–1.6 (что доказано и чем), §2 (границы
   подэтапов), §3 (журнал коммитов), §4 (все находки — там грабли PowerShell, приёмки и
   фильтра), §6 (§6.6 — твой чеклист), §7 (этот промт).
2. AGENT_A3_HANDOFF_PROMPT.md: §2.3 (контракты, которые нельзя ломать), §3.1…3.13 (требования
   TR-3.x/5.x/6.x — нужны точные формулировки для заметок о выпуске), §5 (критерии готовности
   A3), §8 (В-12…В-16).
3. RELEASING.md — как выпускается релиз: что кладётся в архив, как называются заметки, какие
   проверки обязательны; RELEASE_NOTES_0.24.0.md — образец оформления «Фазы A1/A2».
4. README.md — §5 «Что где лежит», §15 «Перечень команд», §19 «Анонимизация документов»,
   §20 «Что ещё почитать» (новый раздел встаёт между §19 и §20). sidecar/README.md — что там
   уже сказано про Python-sidecar, чтобы встроить Node-воркер в тот же тон и структуру.
5. Код и скрипты — только читать, для точности: crates/hds-cli/src/cmd/agent.rs (usage),
   agent_run.rs (plan_json, команды, TurnReport), crates/hds-agent/src/{registry,gateway,
   history}.rs, sidecar/hds_agent/src/worker.ts, installers/build_agent_sidecar.ps1,
   runtime-manifests/node-manifest.json, installers/build_rust_release.ps1 (блок $sidecarDst),
   installers/install_server.ps1 (блок P5), crates/hds-cli/src/cmd/check.rs.
Затем покажи короткий план (какие файлы и что именно меняешь) и начинай.

СТАРТ
* git --no-pager status: ветка agent-privacy-zone, HEAD ≥ ec296ee6, дерево чистое. Чужие
  правки не трогать; если мешают — остановись и спроси.
* Прогони до работы (всё должно быть зелёным): cargo test --workspace -- --test-threads=4;
  (cd sidecar/hds_agent; npm test); cargo fmt --all --check;
  cargo clippy --workspace --all-targets -- -D warnings.

ЗАДАЧА (итог — чеклист §6.6; каждый пункт заканчивается коммитом и видимой проверкой)
1. sidecar/README.md: раздел про Node-воркер `hds_agent` — сборка (`npm ci`, `npm run build` →
   `dist/worker.mjs`), протокол §4.3, установка (`installers/build_agent_sidecar.ps1`,
   `-SelfTest`, `-NodeSource` для офлайна, манифест `runtime-manifests/node-manifest.json`),
   что остаётся в поставке (`@cline/sdk`, `tavily-mcp`; dev-зависимости срезает
   `npm prune --omit=dev`), `npm test` = 23/23.
2. README.md: новый раздел «ИИ-агент в анонимной зоне» между §19 и §20 плюс строки в §15
   «Перечень команд» и §5 «Что где лежит». Обязательно: включение (`agent.enabled`,
   `agent.root`), ключ провайдера только в `data/agent/secrets.json`, команды
   `project add|list` / `run [--session] [--dry-run] [--json]` / `history clear`, что уходит
   наружу (метки, не значения; ключ в тексте блокирует запрос), что не читается (реальный
   `%USERPROFILE%\.cline`, плагины, `run_commands`, `fetch_web_content`), ограничения
   (`ask_user` — отказ до вкладки A5; реестр процессов к CLI не подключён;
   `max_live_projects=2`, `idle_unload_min=30`), где лежат файлы агента
   (`<agent.root>\P0001\{workspace,agent-config,agent-state}`).
3. RELEASE_NOTES_0.25.0.md: раздел «Фаза A3» по образцу 0.24.0 — что нового (TR-3.1…3.10,
   TR-5.x, TR-6.x), таблица «что качать», что и чем проверено (файлы тестов из §1.2–1.6),
   ограничения и открытые вопросы. Номер версии и дату уточни у владельца, если не очевидны.
4. AGENT_A3_HANDOFF_PROMPT.md: §5 — отметить выполненные критерии `[x]` со ссылкой на §1.x;
   §8 — фактически принятые ответы по В-12…В-16 и что осталось владельцу (В-15 «1c-buddy»;
   «стоп-точка не проверялась» из §4 п.37).
5. Релизная сборка: в installers/build_rust_release.ps1 собрать агентский воркер в
   `$sidecarDst` (`build_agent_sidecar.ps1 -OutDir $sidecarDst`) под переключателем по образцу
   `-TranscribeProfile` (предложение: `-AgentSidecar none|portable`); в
   build_rust_release_macos.sh — аналог или явное «[i] не собирается» в логе. Скрипты —
   ASCII-only.
6. installers/install_server.ps1: решение по серверному профилю (агент не ставим —
   desktop-функция, либо `-WithAgent`) со строкой в `$summary`; решение и причина —
   в AGENT_A3_STAGES.md §4.
7. Проверка интеграции (лог в tools/parity/out/, каталог гитигнорится):
   powershell -NoProfile -ExecutionPolicy Bypass -File installers\build_rust_release.ps1 `
       -Version 0.0.0-a37 -SkipBuild -SkipZip        # только staging, без zip
   Ожидаемо: `dist\hds-0.0.0-a37-windows-x64\sidecar\hds_agent\dist\worker.mjs`,
   `sidecar\node\node.exe`, в `node_modules` нет `esbuild`/`typescript`; затем удали
   dist-каталог. Если `-SkipBuild`/staging упрётся в занятый `llm_host.exe` — освободи
   (dev-машина: резидент можно остановить) и верни его после прогона.
8. AGENT_A3_STAGES.md: §1.7 (что сделано в A3.7), §2 (A3.7 закрыт), §3 (коммиты), §4 (находки
   релизной интеграции), §6 — контекст следующей фазы (A5, вкладка UI): что для неё готово
   (`AgentRegistry`, `Gateway::session_id`, `RunOptions.session`, `TurnReport`, NDJSON-события
   `session`/`prompt`/`redacted`/`agent`/`turn_end`).

ПРАВИЛА
* `installers/*.ps1` — ASCII-only (PowerShell 5.1 читает BOM-less `.ps1` как ANSI); `.md`
  править `[IO.File]::ReadAllText/WriteAllText` с `UTF8Encoding` — `Get-Content | Set-Content`
  в PS 5.1 портит кириллицу (§4 п.32).
* git всегда с `--no-pager`; длинные прогоны — `Start-Job` + `Wait-Job -Timeout`, лог в
  `tools/parity/out/`. Артефакты (`dist`, `sidecar/node`, деревья поставки) за собой убирать.
* В документации не обещать того, чего нет: `ask_user` = отказ до A5; реестр процессов к CLI
  не подключён; `1c-buddy` не проверен; аудит срабатываний фильтра на desktop выключен
  (`audit.enabled: false` — включать в примерах явно).
* Значения PII в документацию не попадают: примеры — только метки (`[PERSON_1]`) и
  вымышленные реквизиты.

ЗАВЕРШЕНИЕ
* Всё зелёное: `cargo fmt --all --check`;
  `cargo clippy --workspace --all-targets -- -D warnings`;
  `cargo test --workspace -- --test-threads=4` (752+ тестов);
  `(cd sidecar/hds_agent; npm test)` — 23/23;
  python: `sidecar\venv-anonymize\Scripts\python.exe -m unittest discover -s sidecar/tests
  -t sidecar`; `installers\build_agent_sidecar.ps1 -SelfTest`;
  приёмка `cargo test -p hds-cli --test agent_acceptance`.
* Коммиты: `docs(agent): A3.7 — …`, при необходимости `feat(installer): A3.7 — …`,
  закрывающий `docs(agent): A3.7 закрыт — фаза A3`.
* Сводка: что задокументировано, что изменено в установщиках, какие проверки прогнаны (числа),
  что осталось владельцу (В-15, номер версии, серверный профиль, `hds check`) и что из этого
  получает вкладка A5.

СТОП-ТОЧКИ (остановиться и спросить владельца)
* номер версии и дата для `RELEASE_NOTES`, нужно ли менять версионирование;
* релизная сборка требует остановки резидента `llm_host`, а это сейчас неудобно;
* для `npm ci` в релизной сборке нет сети (или кэша) — тогда решение: собирать из уже
  установленного `node_modules` либо оставить шаг установщику;
* в документацию просится утверждение, которое ничем не подтверждено (например, `1c-buddy`
  под подменённым HOME) — не выдумывать, вынести в вопросы.
```

