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
> `1b80fc3f` (test) плюс docs-коммиты. Дерево чистое, push не делался.

## 1. Что уже сделано и проверено (A3.0…A3.4)

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

**Ещё НЕ сделано в фазе A3** (после A3.4):

* `installers/build_agent_sidecar.ps1` — установщик воркера (A3.5);
* приёмка e2e на стенде A2 (A3.6) — включая `run --session` и `history clear` на копии
  `agent-state`;
* документация A3 (`sidecar/README.md`, раздел `README.md`, `RELEASE_NOTES`; A3.7);
* долг A3.4: реестр `AgentRegistry` не подключён к CLI-прогону (один проект на процесс) и
  не имеет фонового reaper — это вкладка A5/служба (§4 п.23).

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
| A3.5 | Установщик | `installers/build_agent_sidecar.ps1` (портативный Node без пробелов в пути → `sidecar/node/`, `npm ci --omit=dev` по lock, сборка `dist/`, `-SelfTest` = `hello`); образец — `build_anonymize_sidecar.ps1`, `sidecar_common.ps1` | ~0,5 чата |
| A3.6 | Приёмка A3 (e2e) | стенд из A2: образ docx → `office_read` (настоящий Python-воркер), `disk-search__search_local_files` с маршрута `/mcp/agent/P0001` (живой `hds mcp-http`), заглушка `tavily` (`mcp_out`), запись файла в workspace; I2/I7 на заглушках; профиль `%USERPROFILE%\.cline` до/после не изменён; плюс `run --session` (прошлая реплика модели) и `history clear` на **копии** `agent-state`. **Здесь стоп-точки** «disk-search не отвечает из SDK» и «1c-buddy/tavily под подменённым HOME» | ~1 чат |
| A3.7 | Документация и закрытие | `sidecar/README.md`, раздел в `README.md`, `RELEASE_NOTES` «Фаза A3», статус в `AGENT_A3_HANDOFF_PROMPT.md`; полный набор проверок (§5 карты); вопросы В-12…В-16 владельцу | ~0,5 чата |

Зависимости: A3.1 и A3.2 независимы (A3.1 первым — он проверяет допущения о SDK, на которые
опирается всё остальное). A3.3 ← A3.2. A3.4 ← A3.3. A3.5 независим (можно в любой момент).
A3.6 ← A3.3 + A3.5. A3.7 — последним.

## 3. Журнал под-этапов

| Под-этап | Состояние | Коммиты | Заметки |
|---|---|---|---|
| A3.0 | закрыт 19.11.2026 | `f10a92e8`, `a8dc5b51` | см. §1 |
| A3.1 | закрыт 19.11.2026 | `2efe8043` (fix), `8e26d0f3` (test) | `npm test` 23/23, ≈ 39 с; карта тестов — §1.1; находки — §4 п.7–12; `Gateway::open` поправить в A3.2 (§4 п.1) |
| A3.2 | закрыт 19.11.2026 | `6ef028af` (feat), `8204e384` (test) | `cargo test -p hds-agent` = 56 тестов (54 unit + 2 интеграционных), `cargo fmt --check` и `clippy --workspace -D warnings` чистые; факты — §1.2, находки — §4 п.1–3 и 13–16 |
| A3.3 | закрыт 19.11.2026 | `be72b01d` (feat), `74d1aacb` (test) | `cargo test -p hds-cli --test agent_run` = 3 теста, весь воркспейс 737/737; факты — §1.3, находки — §4 п.17–22 |
| A3.4 | закрыт 19.11.2026 | `55abc33b` (feat), `1b80fc3f` (test) | `cargo test -p hds-agent` = 68 тестов (8 — `registry`, 3 — `lifecycle_worker`), ≈ 11 с; `cargo test -p hds-cli --test agent_run` = 5, весь воркспейс 750/750; `npm test` 23/23; факты — §1.4, находки — §4 п.23–28. Долг: реестр готов, но CLI-прогон по-прежнему один проект на процесс (подключение — вкладка A5) |

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

## 6. Контекст для нового чата (A3.5 и далее)

> «Памятка входа»: что сделано, где лежит, какие грабли уже оплачены. Промт
> следующего подэтапа — §7. Требования и критерии фазы — `AGENT_A3_HANDOFF_PROMPT.md`;
> нарезка фазы A3 на подэтапы — §2 этого файла.

### 6.1. Где мы

* Верхний уровень: фаза **A3** плана `PLAN_AGENT_PRIVACY_ZONE.md` (§5.3, §5.5, §5.6) —
  «агент в анонимной зоне». Нарезана на подэтапы **A3.1…A3.7** (§2): A3.1 — воркер на
  Node-стенде, A3.2 — Rust ↔ настоящий воркер, A3.3 — точка входа CLI, **A3.4 — жизненный
  цикл (TR-3.9) — закрыт**, дальше A3.5 — установщик, A3.6 — приёмка (e2e), A3.7 —
  документация.
* Ветка `agent-privacy-zone`, HEAD ≥ `1b80fc3f` (test A3.4), дерево чистое, push не делался.
* Коммиты: A3.0 — `f10a92e8`, `a8dc5b51`; A3.1 — `2efe8043` (fix), `8e26d0f3` (test);
  A3.2 — `6ef028af` (feat), `8204e384` (test); A3.3 — `be72b01d` (feat), `74d1aacb` (test);
  A3.4 — `55abc33b` (feat), `1b80fc3f` (test); docs — `7779343b`, `c5c1f57d`, `76661a7a`
  и docs-коммит A3.4.
* Проверки на срезе: `cargo fmt --all --check` и
  `cargo clippy --workspace --all-targets -- -D warnings` — чисто;
  `cargo test --workspace -- --test-threads=4` — **750** тестов, 0 падений;
  `npm test` в `sidecar/hds_agent` — 23/23, ≈ 39 с; чистая копия (`npm ci` + build) собирается.

### 6.2. Что уже работает (точки входа)

**Node-воркер `sidecar/hds_agent`** (TS → `dist/worker.mjs`, `@cline/sdk` 0.2.0): протокол
§4.3 (`hello`/`start`/`send`/`abort`/`stop`/`shutdown` + события + `host_call`), политики
(без `run_commands`/`fetch_web_content`), `hds-guard`, `office_*`/`ask_user` через
`host_call`, `egressFetch` (каждый запрос и повтор — `egress_check`), MCP из
`CLINE_MCP_SETTINGS_PATH`, плагины профиля выключены политикой, `turn_end {ok, reason, text,
error, tools}`, восстановление сессии после фатальной ошибки (`revive` с тем же id).
Тесты: `npm test` (карта — §1.1).

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
cd sidecar\hds_agent; npm test
```

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
| настройки, пути, реестр проектов, секреты | `settings.rs`, `projects.rs`, `secrets.rs` |
| окружение процесса, MCP, правила | `env.rs`, `mcp.rs`, `rules.rs` |
| сборка сессии в CLI | `crates/hds-cli/src/cmd/agent_run.rs` |
| образцы тестов | `crates/hds-agent/tests/gateway_worker.rs`, `crates/hds-agent/tests/lifecycle_worker.rs`, `crates/hds-cli/tests/agent_run.rs` |
| Node-воркер и его тесты | `sidecar/hds_agent/src/*.ts`, `sidecar/hds_agent/test/*.test.mjs` |

## 7. Промт для A3.4 (выполнен; оставлен для истории)

```text
Ты продолжаешь этап A3 («агент в анонимной зоне») проекта hermes-disk-search (hds).
A3 нарезан на подэтапы A3.1…A3.7 (AGENT_A3_STAGES.md §2): A3.1 (Node-воркер на стенде),
A3.2 (Rust ↔ настоящий воркер), A3.3 (точка входа CLI) закрыты; ты делаешь
ПОДЭТАП A3.4 — «Жизненный цикл (TR-3.9)». Он разблокирует A3.6 (приёмка e2e) и вкладку A5.

ГРАНИЦЫ
* Работаешь в crates/hds-agent (src/, tests/) и в AGENT_A3_STAGES.md. В crates/hds-cli —
  только то, что нужно для возобновления и очистки истории в `hds agent` (agent.rs,
  agent_run.rs); новых подкоманд сверх заявленных не добавляешь.
* Не трогаешь: другие крейты, установщик (A3.5), UI/A5, документацию (A3.7), боевые
  config.yaml, index.db, data/. Новых зависимостей не добавляешь (node --test/cargo — как есть).
* Контракты A1/A2 (RPC, маршрут /mcp/agent/<P>, метки в FTS) не ломаешь.

СНАЧАЛА ПРОЧИТАЙ (файлы репо читай через PowerShell
[System.IO.File]::ReadAllLines(path, [Text.Encoding]::UTF8): read_files отдаёт пустоту)
1. AGENT_A3_STAGES.md: §1.1–1.3 (что подтверждено фактом), §2 (нарезка A3), §3 (журнал),
   §4 (долги и грабли — особенно п.4–7, 13, 16–22), §6 (контекст нового чата), этот промт.
2. AGENT_A3_HANDOFF_PROMPT.md: §3.9 (TR-3.9 — требования и критерий), §3.13 (протокол §4.3),
   §6 (грабли), §8 (вопрос В-16).
3. Код: crates/hds-agent/src/{gateway,worker,history,settings,projects,secrets,lib}.rs,
   crates/hds-cli/src/cmd/agent_run.rs; образцы тестов —
   crates/hds-agent/tests/gateway_worker.rs, crates/hds-cli/tests/agent_run.rs.
Затем покажи короткий план (список тестов и что меняется в src) и начинай.

СТАРТ
* git --no-pager status: ветка agent-privacy-zone, HEAD — docs-коммит A3.3 (76661a7a) или
  новее, дерево чистое. Чужие правки не трогать; если мешают — остановись и спроси.
* Прогони до работы: cargo test -p hds-agent; cargo test -p hds-cli --test agent_run;
  (cd sidecar/hds_agent; npm test) — всё зелёное (737 тестов в воркспейсе на срезе).

ЗАДАЧА: тесты на `node --test`/`cargo test` без новых зависимостей. Каждый тест — отдельный
проверяемый факт. Сначала тест; красный — правишь src.
 1. Реестр живых проектов. Новый модуль crates/hds-agent/src/registry.rs (+ экспорт в lib.rs).
    `AgentRegistry::new(max_live: usize, idle: Duration)`; значения по умолчанию берутся из
    AgentSettings (`agent.max_live_projects` = 2, `agent.idle_unload_min` = 30 мин). Реестр
    хранит по проекту (`P0001`) живой `Gateway` (один процесс = один проект) и метку
    последнего использования. API: `get_or_open(project, now, spawn_fn)`,
    `get_mut(project)`, `touch(project, now)`, `unload(project)`, `reap_idle(now) ->
    Vec<String>`, `live() -> Vec<(String, Instant)>`. Выгрузка = `Gateway::stop()` +
    `Gateway::shutdown()` (дождаться выхода, у Worker это ≤ 5 с), запись реестра удаляется.
    Время передаётся параметром — без фоновых потоков и без «спящих» тестов.
 2. `max_live`: при `max_live = 1` открытие второго проекта выгружает первый (его процесс
    завершён — `Gateway::is_closed()`/PID недоступен, запись ушла); при `max_live = 2` оба живы.
    Выбор жертвы — LRU по `last_used`.
 3. Переиспользование: повторный `get_or_open` того же проекта НЕ перезапускает процесс
    (PID тот же, второго `start` нет); повторного `initialMessages` при переиспользовании
    не передаётся. Параллельные ходы — домен UI: реестр отдаёт `&mut Gateway`.
 4. Простой: `reap_idle(now)` выгружает проекты, бездействовавшие дольше `idle`, и не трогает
    свежие; после выгрузки проект открывается заново.
 5. Возобновление после перезапуска hds: `start` с тем же `session_id` И `initialMessages`
    (из `history::read_messages(state_dir, id)`) — на настоящем воркере первый запрос к модели
    содержит прошлую реплику; `session_id` без `initialMessages` историю не грузит (регресс A0
    §3.6 зафиксировать тестом).
 6. «Очистить историю» end-to-end: остановить сессию → `Gateway::clear_history()` (перезапись
    файлов сессии словарём, бэкапы `.json.bak`) → `start` с тем же id и `initialMessages` из
    очищенных файлов → маркера значения в файлах нет, метка на месте, в первом запросе к модели
    — метка. Ошибки: нет файла истории → понятная ошибка (код 1 в CLI).
 7. CLI (минимум, по образцу agent_run.rs): `hds agent run --project P --session <id>`
    возобновляет сессию (session_id + read_messages; история читается из `agent-state`);
    `hds agent history clear --project P [--session <id>]` — чистка истории (печатает число
    замен и пути бэкапов; `--session` не задан — берётся последняя сессия из
    `<state>\.cline\data\sessions`, иначе понятная ошибка). Коды возврата: 0 — успех,
    1 — нет истории/сбой, 2 — использование.
 8. Документация: §1.4 (что подтверждено фактом), §3 (A3.4 закрыт + коммиты), §4 (находки;
    что осталось для UI/сервиса), при необходимости уточни A3.5/A3.6.

ПРАВИЛА
* Guard/фильтр/fail-closed не ослаблять; хуки не бросают исключений; таймаут guard (2 с) не менять.
* Тесты быстрые: реестр — на Node-заглушке (крошечный скрипт в тесте, как ECHO_WORKER в
  worker.rs; пишет свой PID и полученные команды в файл), плюс ОДИН тест на настоящем
  `dist/worker.mjs` с mock-провайдером (образец — gateway_worker.rs). Весь
  `cargo test -p hds-agent` ≤ 60 с, `cargo test --workspace` ≤ 3 мин.
* Временные каталоги — `os.tmpdir()` + pid + счётчик (AtomicU32): тесты идут параллельно.
* Env в тестах CLI (`HDS_ROOT`, `HDS_CONFIG`, `HDS_AGENT_SECRETS`, `HDS_ANONYMIZE_WORKER`) —
  под мьютекс и с восстановлением.
* Боевые config.yaml, index.db, data/ не трогать; артефакты — tools/parity/out/ (гитигнорится).
* Пути к node/скрипту чистить от `\\?\` перед передачей в Node (§4 п.13).
* git — всегда с `--no-pager`; коммиты явным списком путей и с identity:
  git -c user.name='Alex Guerchoig' -c user.email='Guerchoig@gmail.com' commit -m '…'
* Длинные прогоны — Start-Job + Wait-Job -Timeout, лог в tools/parity/out/.
* Редактор: ≤ 6000 символов на вызов, большие файлы — кусками.

ЗАВЕРШЕНИЕ
* cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings;
  cargo test --workspace -- --test-threads=4; npm test в sidecar/hds_agent — зелёные.
* Коммиты: «feat(agent): A3.4 — …», «test(agent): A3.4 — …» (и отдельный docs-коммит с
  обновлением AGENT_A3_STAGES.md: §1.4, §3, §4). Push не делать.
* Сводка: какие пункты 1–8 зелёные, что отложено и почему (например, фоновый reaper для
  service/UI), что меняется для A3.5, A3.6 и A5.
* СТОП-ТОЧКИ (остановиться и спросить владельца): реестр требует менять публичный API
  `Gateway` так, что ломаются A3.2/A3.3; выгрузка не укладывается в 5 с и требует нового
  контракта протокола; ключ провайдера попадает в файл вне secrets.json; нужны значения
  `max_live_projects`/`idle_unload_min`, отличные от В-16 (2 и 30 мин).
```
