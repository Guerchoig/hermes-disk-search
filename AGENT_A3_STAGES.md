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
> **Дата среза:** 19.11.2026. **Ветка:** `agent-privacy-zone`. Код A3.0 — до `a8dc5b51`
> включительно; поверх него один docs-коммит с этим файлом. Дерево чистое, push не делался.

## 1. Что уже сделано и проверено (A3.0)

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

**Ещё НЕ проверено фактом** (код есть, теста нет — это работа A3.1):

* `beforeModel` с заменой `tools` (вырезание `*__ask_my_files`);
* `plugins: {'*': {enabled: false}}` — плагин из `<HOME>\.cline\plugins` не грузится;
* MCP через файл по `CLINE_MCP_SETTINGS_PATH`, фильтр `mcp_out`/`mcp_in`;
* `office_*` и `ask_user` в живом прогоне;
* fail-closed (отказ `egress_check` → 0 запросов к модели);
* `abort`, суб-агенты (`subagents: true`), возобновление через `history`
  (`initialMessages`);
* поле `tools` в ответе `start` — **статический список**, а не фактический. Фактический
  список приходит в `turn_end.tools` (из `beforeModel`).

**Ещё НЕ написано:**

* подключение `hds-agent` в `hds-cli` и сборка сессии из конфига;
* менеджер жизненного цикла;
* `installers/build_agent_sidecar.ps1`;
* документация A3.

## 2. Под-этапы (остаток A3)

Правило нарезки: под-этап = один чат = один-два коммита = **видимый зелёный результат**. Если
под-этап разрастается — остановиться на ближайшем зелёном тесте, закоммитить и записать
остаток в этот файл (§3).

| # | Под-этап | Результат «снаружи» (критерий) | Объём |
|---|---|---|---|
| **A3.1** | **Воркер: TR-3.10 на Node-стенде** | `npm test` покрывает TR-3.10 а, в, г, д, е, ж и ветки воркера: MCP-заглушка через файл настроек (`<server>__<tool>`, `ask_my_files` вырезан, `mcp_out`/`mcp_in` через `egress_check`), `office_*`/`ask_user` через `host_call`, отказ guard доходит до модели текстом, fail-closed = 0 запросов к mock, секрет блокируется, `abort`, `subagents` false/true, возобновление через `history` | ~1 чат |
| A3.2 | Rust ↔ настоящий воркер | `cargo test -p hds-agent` (интеграционный, `#[ignore]` при отсутствии `node`/`dist`): `Gateway::spawn` → `hello` → `open` → `send` на mock-провайдере. `egress_check` проходит **настоящий** `Filter` (снимок без Python, `GatewayHost::without_client`); события деанонимизированы (`[PERSON_1]` → значение, пути workspace → пути проекта); `Redacted` без значений | ~1 чат |
| A3.3 | Точка входа CLI | `hds agent project add|list` (`projects.json`), `hds agent run --project P [--prompt …]` (В-16) собирает сессию из конфига: поиск Node (`agent.node.path` → `sidecar/node` → PATH), `env::build`, `mcp::write_settings`, `rules::write_rules`, `Secrets`, снимок vault, `Gateway`; события печатаются в консоль. Тест CLI на mock-провайдере | ~1 чат |
| A3.4 | Жизненный цикл (TR-3.9) | реестр живых проектов в `hds-agent`: `max_live_projects`, `idle_unload_min`, переиспользование процесса на повторный `start`, возобновление `session_id + initialMessages` из `history::read_messages`, «Очистить историю» end-to-end. Тесты на заглушке воркера (быстрые) + 1 на настоящем | ~1 чат |
| A3.5 | Установщик | `installers/build_agent_sidecar.ps1` (портативный Node без пробелов в пути → `sidecar/node/`, `npm ci --omit=dev` по lock, сборка `dist/`, `-SelfTest` = `hello`); образец — `build_anonymize_sidecar.ps1`, `sidecar_common.ps1` | ~0,5 чата |
| A3.6 | Приёмка A3 (e2e) | стенд из A2: образ docx → `office_read` (настоящий Python-воркер), `disk-search__search_local_files` с маршрута `/mcp/agent/P0001` (живой `hds mcp-http`), заглушка `tavily` (`mcp_out`), запись файла в workspace; I2/I7 на заглушках; профиль `%USERPROFILE%\.cline` до/после не изменён. **Здесь стоп-точки** «disk-search не отвечает из SDK» и «1c-buddy/tavily под подменённым HOME» | ~1 чат |
| A3.7 | Документация и закрытие | `sidecar/README.md`, раздел в `README.md`, `RELEASE_NOTES` «Фаза A3», статус в `AGENT_A3_HANDOFF_PROMPT.md`; полный набор проверок (§5 карты); вопросы В-12…В-16 владельцу | ~0,5 чата |

Зависимости: A3.1 и A3.2 независимы (A3.1 первым — он проверяет допущения о SDK, на которые
опирается всё остальное). A3.3 ← A3.2. A3.4 ← A3.3. A3.5 независим (можно в любой момент).
A3.6 ← A3.3 + A3.5. A3.7 — последним.

## 3. Журнал под-этапов

| Под-этап | Состояние | Коммиты | Заметки |
|---|---|---|---|
| A3.0 | закрыт 19.11.2026 | `f10a92e8`, `a8dc5b51` | см. §1 |
| A3.1 | следующий | — | промт — §5 |

## 4. Известные долги и находки (передаются по под-этапам)

1. `start.result.tools` в воркере — статический список; фактический — из `beforeModel`
   (`turn_end.tools`). В A3.1 либо снять поле, либо явно назвать его `declared_tools`
   и поправить `Gateway::open` (сейчас читает `result.tools`).
2. `GatewayEvent::Agent` при `Up::HostCall` — мёртвая ветка: `host_call` перехватывает
   поток чтения. Убрать в A3.2.
3. `turn_end` — событие воркера, а не `done`. Rust (`Gateway`) об этом пока не знает. В A3.2
   добавить `GatewayEvent::TurnEnd {ok, reason, text, error}` и проверку «успех при пустом
   ответе» (грабли §6.1 карты).
4. Скрипт `npm test` — `node --test "test/*.test.mjs"`. Каталог `test/` Node 24 сам не
   раскрывает.
5. `npm install` предупреждает `allow-scripts` про `esbuild` и `protobufjs`. Бинарник esbuild
   для win32-x64 приходит как optional-зависимость, сборка работает без postinstall. Учесть
   в A3.5.
6. Терминал агента: `git` без `--no-pager` вешает команду (пейджер). Длинные прогоны
   (`cargo test --workspace`) — через `Start-Job` + `Wait-Job -Timeout` и лог в
   `tools/parity/out/`.



## 5. Промт для A3.1 (копировать целиком в новый чат, режим Act)

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
